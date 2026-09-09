use crate::aws::client::AwsClients;
use crate::aws::resource::{native_state_label, Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use aws_sdk_identitystore::Client as IdentitystoreClient;
use aws_sdk_ssoadmin::Client as SsoAdminClient;
use std::any::Any;
use std::collections::HashMap;
use tokio::sync::mpsc;

pub struct IdentityCenterService {
    ssoadmin: SsoAdminClient,
    identitystore: IdentitystoreClient,
}

impl IdentityCenterService {
    pub fn new(aws_clients: &AwsClients) -> Self {
        Self {
            ssoadmin: aws_clients.ssoadmin_client(),
            identitystore: aws_clients.identitystore_client(),
        }
    }
}

#[async_trait]
impl AwsService for IdentityCenterService {
    fn service_type(&self) -> ServiceType {
        ServiceType::IdentityCenter
    }

    fn name(&self) -> &str {
        "IAM Identity Center"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        self.list_resources_streaming(tx, ServiceType::IdentityCenter)
            .await?;
        Ok(vec![])
    }

    async fn list_resources_streaming(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        // Discover the Identity Center instance — there is at most one per account/region.
        let instances_resp = match self.ssoadmin.list_instances().send().await {
            Ok(r) => r,
            Err(e) => {
                let _ = event_tx.send(Event::ResourceLoadError {
                    service: service_type,
                    error: format!("Failed to list Identity Center instances: {}", e),
                });
                return Ok(());
            }
        };

        let instance = match instances_resp.instances().first() {
            Some(i) => i,
            None => {
                // No Identity Center instance configured in this account/region.
                let _ = event_tx.send(Event::ResourcesFullyLoaded {
                    service: service_type,
                    total_count: 0,
                });
                return Ok(());
            }
        };

        let instance_arn = instance.instance_arn().unwrap_or_default().to_string();
        let identity_store_id = instance.identity_store_id().unwrap_or_default().to_string();
        let mut total = 0usize;

        // ── Instance ─────────────────────────────────────────────────────────
        let mut ic_instance = IcInstance::from_sdk(instance);
        // DescribeInstance adds created date, status reason, and the KMS
        // encryption configuration — best-effort (older accounts / permission
        // gaps just keep the ListInstances fields).
        if let Ok(d) = self
            .ssoadmin
            .describe_instance()
            .instance_arn(&instance_arn)
            .send()
            .await
        {
            ic_instance.enrich_from_describe(&d);
        }
        let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
            service: service_type,
            resources: vec![Box::new(ic_instance)],
            progress: LoadProgress {
                loaded_count: 1,
                total_count: None,
                status_message: Some("Loaded instance info…".to_string()),
            },
        });
        total += 1;

        // ── Permission Sets ──────────────────────────────────────────────────
        let mut ps_arns: Vec<String> = Vec::new();
        let mut ps_paginator = self
            .ssoadmin
            .list_permission_sets()
            .instance_arn(&instance_arn)
            .into_paginator()
            .items()
            .send();

        while let Some(result) = ps_paginator.next().await {
            match result {
                Ok(arn) => ps_arns.push(arn),
                Err(e) => {
                    // Later batches can still stream — record a warning and move on.
                    let _ = event_tx.send(Event::ResourceLoadWarning {
                        service: service_type,
                        warning: format!(
                            "permission sets unavailable: {}",
                            crate::error::sdk_error_message(&e)
                        ),
                    });
                    break;
                }
            }
        }

        let mut ps_resources: Vec<Box<dyn Resource>> = Vec::new();
        for arn in &ps_arns {
            let desc = self
                .ssoadmin
                .describe_permission_set()
                .instance_arn(&instance_arn)
                .permission_set_arn(arn)
                .send()
                .await;

            if let Ok(output) = desc {
                if let Some(ps) = output.permission_set() {
                    ps_resources.push(Box::new(PermissionSet::from_sdk(
                        ps,
                        &instance_arn,
                        &identity_store_id,
                    )));
                }
            }
        }

        total += ps_resources.len();
        if !ps_resources.is_empty() {
            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                service: service_type,
                resources: ps_resources,
                progress: LoadProgress {
                    loaded_count: total,
                    total_count: None,
                    status_message: Some(format!("Loaded {} permission sets…", total)),
                },
            });
        }

        // ── Users ────────────────────────────────────────────────────────────
        let mut user_resources: Vec<Box<dyn Resource>> = Vec::new();
        let mut user_paginator = self
            .identitystore
            .list_users()
            .identity_store_id(&identity_store_id)
            .into_paginator()
            .items()
            .send();

        while let Some(result) = user_paginator.next().await {
            match result {
                Ok(user) => {
                    user_resources.push(Box::new(IcUser::from_sdk(&user, &instance_arn)));
                }
                Err(e) => {
                    let _ = event_tx.send(Event::ResourceLoadWarning {
                        service: service_type,
                        warning: format!(
                            "users unavailable: {}",
                            crate::error::sdk_error_message(&e)
                        ),
                    });
                    break;
                }
            }
        }

        total += user_resources.len();
        if !user_resources.is_empty() {
            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                service: service_type,
                resources: user_resources,
                progress: LoadProgress {
                    loaded_count: total,
                    total_count: None,
                    status_message: Some(format!("Loaded {} users…", total)),
                },
            });
        }

        // ── Groups ───────────────────────────────────────────────────────────
        let mut group_resources: Vec<Box<dyn Resource>> = Vec::new();
        let mut group_paginator = self
            .identitystore
            .list_groups()
            .identity_store_id(&identity_store_id)
            .into_paginator()
            .items()
            .send();

        while let Some(result) = group_paginator.next().await {
            match result {
                Ok(group) => {
                    group_resources.push(Box::new(IcGroup::from_sdk(&group, &instance_arn)));
                }
                Err(e) => {
                    let _ = event_tx.send(Event::ResourceLoadWarning {
                        service: service_type,
                        warning: format!(
                            "groups unavailable: {}",
                            crate::error::sdk_error_message(&e)
                        ),
                    });
                    break;
                }
            }
        }

        total += group_resources.len();
        if !group_resources.is_empty() {
            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                service: service_type,
                resources: group_resources,
                progress: LoadProgress {
                    loaded_count: total,
                    total_count: None,
                    status_message: Some(format!("Loaded {} groups…", total)),
                },
            });
        }

        // ── Applications ─────────────────────────────────────────────────────
        // ListApplications already returns the full application (provider,
        // status, portal options) — no N+1 describe needed. Error-tolerant: a
        // sso:ListApplications permission gap records a warning, never breaks
        // the core load.
        let mut app_resources: Vec<Box<dyn Resource>> = Vec::new();
        let mut app_paginator = self
            .ssoadmin
            .list_applications()
            .instance_arn(&instance_arn)
            .into_paginator()
            .items()
            .send();
        while let Some(result) = app_paginator.next().await {
            match result {
                Ok(app) => {
                    app_resources.push(Box::new(IcApplication::from_sdk(
                        &app,
                        &identity_store_id,
                    )));
                }
                Err(e) => {
                    let _ = event_tx.send(Event::ResourceLoadWarning {
                        service: service_type,
                        warning: format!(
                            "applications unavailable: {}",
                            crate::error::sdk_error_message(&e)
                        ),
                    });
                    break;
                }
            }
        }

        total += app_resources.len();
        if !app_resources.is_empty() {
            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                service: service_type,
                resources: app_resources,
                progress: LoadProgress {
                    loaded_count: total,
                    total_count: None,
                    status_message: Some(format!("Loaded {} resources…", total)),
                },
            });
        }

        let _ = event_tx.send(Event::ResourcesFullyLoaded {
            service: service_type,
            total_count: total,
        });

        Ok(())
    }

    async fn get_resource_details(&self, id: &str) -> Result<Box<dyn Resource>> {
        Err(crate::error::Error::ResourceNotFound(id.to_string()))
    }
}

// ── Permission Set ────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct PermissionSet {
    pub arn: String,
    pub name: String,
    pub description: String,
    pub session_duration: Option<String>,
    pub relay_state: Option<String>,
    pub instance_arn: String,
    pub identity_store_id: String,
    pub tags: HashMap<String, String>,
}

impl PermissionSet {
    pub fn from_sdk(
        ps: &aws_sdk_ssoadmin::types::PermissionSet,
        instance_arn: &str,
        identity_store_id: &str,
    ) -> Self {
        Self {
            arn: ps.permission_set_arn().unwrap_or_default().to_string(),
            name: ps.name().unwrap_or_default().to_string(),
            description: ps.description().unwrap_or_default().to_string(),
            session_duration: ps.session_duration().map(|s| s.to_string()),
            relay_state: ps.relay_state().map(|s| s.to_string()),
            instance_arn: instance_arn.to_string(),
            identity_store_id: identity_store_id.to_string(),
            tags: HashMap::new(),
        }
    }

    fn instance_id(&self) -> &str {
        // ARN format: arn:aws:sso:::instance/ssoins-xxxxxxxxxxx
        self.instance_arn
            .split('/')
            .next_back()
            .unwrap_or_default()
    }

    fn permission_set_id(&self) -> &str {
        // ARN format: arn:aws:sso:::permissionSet/ssoins-xxx/ps-xxx
        self.arn.split('/').next_back().unwrap_or_default()
    }
}

crate::sections! {
    pub enum PermissionSetDetailSection,
    pub static PERMISSION_SET_SECTIONS = [
        Details "Details" => crate::app::App::hook_permission_set_section,
        Policies "Policies" => crate::app::App::hook_permission_set_section,
        Assignments "Assignments" => crate::app::App::hook_permission_set_section,
        Tags "Tags" => crate::app::App::hook_permission_set_section,
    ]
}

impl Resource for PermissionSet {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&PERMISSION_SET_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.arn
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn resource_type(&self) -> &str {
        "Permission Set"
    }

    fn state(&self) -> ResourceState {
        ResourceState::stateless()
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!(
            "{} {} {} {}",
            self.arn,
            self.name,
            self.description,
            self.session_duration.as_deref().unwrap_or_default()
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        let mut d = vec![
            ("Name".to_string(), self.name.clone()),
            ("ARN".to_string(), self.arn.clone()),
        ];
        if !self.description.is_empty() {
            d.push(("Description".to_string(), self.description.clone()));
        }
        if let Some(dur) = &self.session_duration {
            d.push(("Session Duration".to_string(), dur.clone()));
        }
        if let Some(relay) = &self.relay_state {
            d.push(("Relay State".to_string(), relay.clone()));
        }
        d
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{region}.console.aws.amazon.com/singlesignon/home?region={region}#/instances/{}/permissionSets/{}",
            self.instance_id(),
            self.permission_set_id(),
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── User ──────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct IcUser {
    pub user_id: String,
    pub username: String,
    pub display_name: String,
    pub email: Option<String>,
    pub status: Option<String>, // ENABLED / DISABLED
    pub title: Option<String>,
    pub user_type: Option<String>,
    pub given_name: Option<String>,
    pub family_name: Option<String>,
    /// (issuer, id) pairs — present when the user is provisioned from an
    /// external IdP via SCIM (the closest public-API signal for identity source).
    pub external_ids: Vec<(String, String)>,
    pub created_at: Option<String>,
    pub created_by: Option<String>,
    pub updated_at: Option<String>,
    pub updated_by: Option<String>,
    pub identity_store_id: String,
    pub instance_arn: String,
    pub tags: HashMap<String, String>,
}

impl IcUser {
    pub fn from_sdk(user: &aws_sdk_identitystore::types::User, instance_arn: &str) -> Self {
        let email = user
            .emails()
            .iter()
            .find(|e| e.primary())
            .or_else(|| user.emails().first())
            .and_then(|e| e.value())
            .map(|s| s.to_string());

        Self {
            user_id: user.user_id().to_string(),
            username: user.user_name().unwrap_or_default().to_string(),
            display_name: user.display_name().unwrap_or_default().to_string(),
            email,
            status: user.user_status().map(|s| s.as_str().to_string()),
            title: user.title().map(|s| s.to_string()),
            user_type: user.user_type().map(|s| s.to_string()),
            given_name: user.name().and_then(|n| n.given_name()).map(|s| s.to_string()),
            family_name: user.name().and_then(|n| n.family_name()).map(|s| s.to_string()),
            external_ids: external_id_pairs(user.external_ids()),
            created_at: fmt_dt(user.created_at()),
            created_by: user.created_by().map(|s| s.to_string()),
            updated_at: fmt_dt(user.updated_at()),
            updated_by: user.updated_by().map(|s| s.to_string()),
            identity_store_id: user.identity_store_id().to_string(),
            instance_arn: instance_arn.to_string(),
            tags: HashMap::new(),
        }
    }
}

/// Format an SDK timestamp for display, or None when absent.
fn fmt_dt(d: Option<&aws_smithy_types::DateTime>) -> Option<String> {
    d.and_then(|d| d.fmt(aws_smithy_types::date_time::Format::DateTime).ok())
}

/// Flatten identity-store external IDs into (issuer, id) display pairs.
fn external_id_pairs(ids: &[aws_sdk_identitystore::types::ExternalId]) -> Vec<(String, String)> {
    ids.iter()
        .map(|e| (e.issuer().to_string(), e.id().to_string()))
        .collect()
}

crate::sections! {
    pub enum IcUserDetailSection,
    pub static IC_USER_SECTIONS = [
        Details "Details" => crate::app::App::hook_ic_user_section,
        Groups "Groups" => crate::app::App::hook_ic_user_section,
        Access "Access" => crate::app::App::hook_ic_user_section,
    ]
}

impl Resource for IcUser {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&IC_USER_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.user_id
    }

    fn name(&self) -> &str {
        if !self.display_name.is_empty() {
            &self.display_name
        } else {
            &self.username
        }
    }

    fn resource_type(&self) -> &str {
        "IC User"
    }

    fn state(&self) -> ResourceState {
        // Disabled users can't sign in — surface that in the list dot.
        match self.status.as_deref() {
            Some("DISABLED") => ResourceState::Unavailable,
            _ => ResourceState::Available,
        }
    }

    fn state_label(&self) -> String {
        native_state_label(self.status.as_deref().unwrap_or(""), || self.state())
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!(
            "{} {} {} {} {} {}",
            self.user_id,
            self.username,
            self.display_name,
            self.email.as_deref().unwrap_or_default(),
            self.title.as_deref().unwrap_or_default(),
            self.status.as_deref().unwrap_or_default()
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        let mut d = vec![
            ("User ID".to_string(), self.user_id.clone()),
            ("Username".to_string(), self.username.clone()),
        ];
        if !self.display_name.is_empty() {
            d.push(("Display Name".to_string(), self.display_name.clone()));
        }
        if let Some(email) = &self.email {
            d.push(("Email".to_string(), email.clone()));
        }
        if let Some(status) = &self.status {
            d.push(("Status".to_string(), status.clone()));
        }
        if let Some(title) = &self.title {
            d.push(("Title".to_string(), title.clone()));
        }
        d
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{region}.console.aws.amazon.com/singlesignon/home?region={region}#/users/{}",
            self.user_id
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── Group ─────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct IcGroup {
    pub group_id: String,
    pub display_name: String,
    pub description: String,
    /// (issuer, id) pairs — present when SCIM-provisioned from an external IdP.
    pub external_ids: Vec<(String, String)>,
    pub created_at: Option<String>,
    pub created_by: Option<String>,
    pub updated_at: Option<String>,
    pub updated_by: Option<String>,
    pub identity_store_id: String,
    pub instance_arn: String,
    pub tags: HashMap<String, String>,
}

impl IcGroup {
    pub fn from_sdk(group: &aws_sdk_identitystore::types::Group, instance_arn: &str) -> Self {
        Self {
            group_id: group.group_id().to_string(),
            display_name: group.display_name().unwrap_or_default().to_string(),
            description: group.description().unwrap_or_default().to_string(),
            external_ids: external_id_pairs(group.external_ids()),
            created_at: fmt_dt(group.created_at()),
            created_by: group.created_by().map(|s| s.to_string()),
            updated_at: fmt_dt(group.updated_at()),
            updated_by: group.updated_by().map(|s| s.to_string()),
            identity_store_id: group.identity_store_id().to_string(),
            instance_arn: instance_arn.to_string(),
            tags: HashMap::new(),
        }
    }
}

crate::sections! {
    pub enum IcGroupDetailSection,
    pub static IC_GROUP_SECTIONS = [
        Details "Details" => crate::app::App::hook_ic_group_section,
        Members "Members" => crate::app::App::hook_ic_group_section,
        Access "Access" => crate::app::App::hook_ic_group_section,
    ]
}

impl Resource for IcGroup {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&IC_GROUP_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.group_id
    }

    fn name(&self) -> &str {
        &self.display_name
    }

    fn resource_type(&self) -> &str {
        "IC Group"
    }

    fn state(&self) -> ResourceState {
        ResourceState::stateless()
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!(
            "{} {} {}",
            self.group_id, self.display_name, self.description
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        let mut d = vec![("Group ID".to_string(), self.group_id.clone())];
        if !self.display_name.is_empty() {
            d.push(("Display Name".to_string(), self.display_name.clone()));
        }
        if !self.description.is_empty() {
            d.push(("Description".to_string(), self.description.clone()));
        }
        d
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{region}.console.aws.amazon.com/singlesignon/home?region={region}#/groups/{}",
            self.group_id
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── Instance ──────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct IcInstance {
    pub instance_arn: String,
    pub identity_store_id: String,
    pub name: String,
    pub owner_account_id: String,
    pub status: String,
    // Enriched from DescribeInstance (best-effort — absent on a permission gap).
    pub created_date: Option<String>,
    pub status_reason: Option<String>,
    pub encryption_key_type: Option<String>, // AWS_OWNED_KMS_KEY / CUSTOMER_MANAGED_KEY
    pub encryption_kms_key_arn: Option<String>,
    pub encryption_status: Option<String>,
    pub encryption_status_reason: Option<String>,
    pub tags: HashMap<String, String>,
}

impl IcInstance {
    pub fn from_sdk(instance: &aws_sdk_ssoadmin::types::InstanceMetadata) -> Self {
        Self {
            instance_arn: instance.instance_arn().unwrap_or_default().to_string(),
            identity_store_id: instance.identity_store_id().unwrap_or_default().to_string(),
            name: instance.name().unwrap_or_default().to_string(),
            owner_account_id: instance.owner_account_id().unwrap_or_default().to_string(),
            status: instance
                .status()
                .map(|s| s.as_str().to_string())
                .unwrap_or_else(|| "unknown".to_string()),
            created_date: None,
            status_reason: None,
            encryption_key_type: None,
            encryption_kms_key_arn: None,
            encryption_status: None,
            encryption_status_reason: None,
            tags: HashMap::new(),
        }
    }

    /// Fold in the extra fields DescribeInstance returns beyond ListInstances.
    fn enrich_from_describe(
        &mut self,
        d: &aws_sdk_ssoadmin::operation::describe_instance::DescribeInstanceOutput,
    ) {
        self.created_date = fmt_dt(d.created_date());
        self.status_reason = d.status_reason().map(|s| s.to_string());
        if let Some(enc) = d.encryption_configuration_details() {
            self.encryption_key_type = enc.key_type().map(|t| t.as_str().to_string());
            self.encryption_kms_key_arn = enc.kms_key_arn().map(|s| s.to_string());
            self.encryption_status = enc.encryption_status().map(|s| s.as_str().to_string());
            self.encryption_status_reason = enc.encryption_status_reason().map(|s| s.to_string());
        }
    }

    fn instance_id(&self) -> &str {
        self.instance_arn.split('/').next_back().unwrap_or_default()
    }
}

crate::sections! {
    pub enum IcInstanceDetailSection,
    pub static IC_INSTANCE_SECTIONS = [
        Overview "Overview" => crate::app::App::trigger_ic_instance_section_load,
        Abac "ABAC" => crate::app::App::trigger_ic_instance_section_load,
        TrustedIssuers "Trusted Issuers" => crate::app::App::trigger_ic_instance_section_load,
        Summary "Summary" => crate::app::App::trigger_ic_instance_section_load,
    ]
}

impl Resource for IcInstance {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&IC_INSTANCE_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.instance_arn
    }

    fn name(&self) -> &str {
        if !self.name.is_empty() {
            &self.name
        } else {
            &self.instance_arn
        }
    }

    fn resource_type(&self) -> &str {
        "IC Instance"
    }

    fn state(&self) -> ResourceState {
        match self.status.as_str() {
            "ACTIVE" => ResourceState::Available,
            "CREATE_IN_PROGRESS" => ResourceState::Creating,
            "DELETE_IN_PROGRESS" => ResourceState::Deleting,
            _ => ResourceState::Unknown(self.status.clone()),
        }
    }

    fn state_label(&self) -> String {
        native_state_label(&self.status, || self.state())
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!(
            "{} {} {} {}",
            self.instance_arn, self.name, self.identity_store_id, self.owner_account_id
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        let mut d = vec![
            ("Instance ARN".to_string(), self.instance_arn.clone()),
            ("Identity Store ID".to_string(), self.identity_store_id.clone()),
            ("Status".to_string(), self.status.clone()),
        ];
        if !self.name.is_empty() {
            d.push(("Name".to_string(), self.name.clone()));
        }
        if !self.owner_account_id.is_empty() {
            d.push(("Owner Account".to_string(), self.owner_account_id.clone()));
        }
        d
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{region}.console.aws.amazon.com/singlesignon/home?region={region}#/instances/{}/dashboard",
            self.instance_id()
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── Permission Set: lazy access (policies + boundary + tags) ─────────────────

#[derive(Clone, Debug)]
pub struct PsAccess {
    pub inline_policy: Option<String>,           // pretty JSON, or None
    pub managed: Vec<(String, String)>,          // (name, arn)
    pub customer_managed: Vec<(String, String)>, // (name, path)
    pub boundary: Option<String>,                // human label of the boundary
    pub tags: Vec<(String, String)>,
}

/// Fetch a permission set's policies (inline + AWS-managed + customer-managed),
/// its permissions boundary, and its tags — for the Policies / Tags sections.
pub async fn fetch_permission_set_access(
    ssoadmin: SsoAdminClient,
    instance_arn: String,
    ps_arn: String,
) -> Result<PsAccess> {
    // Inline policy (returned as a plain JSON string; may be absent).
    let inline_policy = ssoadmin
        .get_inline_policy_for_permission_set()
        .instance_arn(&instance_arn)
        .permission_set_arn(&ps_arn)
        .send()
        .await
        .ok()
        .and_then(|o| {
            let p = o.inline_policy().unwrap_or_default().to_string();
            if p.is_empty() {
                None
            } else {
                Some(
                    serde_json::from_str::<serde_json::Value>(&p)
                        .ok()
                        .and_then(|v| serde_json::to_string_pretty(&v).ok())
                        .unwrap_or(p),
                )
            }
        });

    let mut managed = Vec::new();
    let mut mp = ssoadmin
        .list_managed_policies_in_permission_set()
        .instance_arn(&instance_arn)
        .permission_set_arn(&ps_arn)
        .into_paginator()
        .items()
        .send();
    while let Some(result) = mp.next().await {
        match result {
            Ok(p) => managed.push((
                p.name().unwrap_or_default().to_string(),
                p.arn().unwrap_or_default().to_string(),
            )),
            Err(_) => break,
        }
    }

    let mut customer_managed = Vec::new();
    let mut cp = ssoadmin
        .list_customer_managed_policy_references_in_permission_set()
        .instance_arn(&instance_arn)
        .permission_set_arn(&ps_arn)
        .into_paginator()
        .items()
        .send();
    while let Some(result) = cp.next().await {
        match result {
            Ok(c) => {
                customer_managed.push((c.name().to_string(), c.path().unwrap_or("/").to_string()))
            }
            Err(_) => break,
        }
    }

    // Permissions boundary (errors when none set → None).
    let boundary = ssoadmin
        .get_permissions_boundary_for_permission_set()
        .instance_arn(&instance_arn)
        .permission_set_arn(&ps_arn)
        .send()
        .await
        .ok()
        .and_then(|o| o.permissions_boundary().cloned())
        .map(|pb| {
            if let Some(arn) = pb.managed_policy_arn() {
                format!("AWS managed: {}", arn)
            } else if let Some(cm) = pb.customer_managed_policy_reference() {
                format!("Customer managed: {}{}", cm.path().unwrap_or("/"), cm.name())
            } else {
                "(set)".to_string()
            }
        });

    let mut tags = Vec::new();
    if let Ok(t) = ssoadmin
        .list_tags_for_resource()
        .instance_arn(&instance_arn)
        .resource_arn(&ps_arn)
        .send()
        .await
    {
        for tag in t.tags() {
            tags.push((tag.key().to_string(), tag.value().to_string()));
        }
    }

    Ok(PsAccess {
        inline_policy,
        managed,
        customer_managed,
        boundary,
        tags,
    })
}

// ── Permission Set: lazy account assignments (with name resolution) ──────────

#[derive(Clone, Debug)]
pub struct PsAssignment {
    pub account_id: String,
    pub principal_type: String, // USER / GROUP
    pub principal_name: String, // resolved (falls back to id)
    pub principal_id: String,
}

/// Fetch which accounts a permission set is provisioned to and the principals
/// assigned in each, resolving USER/GROUP principal ids to names. Sequential
/// (assignment volume per permission set is typically modest); principal name
/// lookups are deduped via a cache.
pub async fn fetch_permission_set_assignments(
    ssoadmin: SsoAdminClient,
    identitystore: IdentitystoreClient,
    instance_arn: String,
    identity_store_id: String,
    ps_arn: String,
) -> Result<Vec<PsAssignment>> {
    // Accounts this PS is provisioned to.
    let mut accounts: Vec<String> = Vec::new();
    let mut ap = ssoadmin
        .list_accounts_for_provisioned_permission_set()
        .instance_arn(&instance_arn)
        .permission_set_arn(&ps_arn)
        .into_paginator()
        .items()
        .send();
    while let Some(result) = ap.next().await {
        match result {
            Ok(a) => accounts.push(a),
            Err(e) => return Err(crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e))),
        }
    }

    // Raw assignments (account, type, id).
    let mut raw: Vec<(String, String, String)> = Vec::new();
    for account in &accounts {
        let mut asg = ssoadmin
            .list_account_assignments()
            .instance_arn(&instance_arn)
            .account_id(account)
            .permission_set_arn(&ps_arn)
            .into_paginator()
            .items()
            .send();
        while let Some(result) = asg.next().await {
            match result {
                Ok(a) => raw.push((
                    account.clone(),
                    a.principal_type()
                        .map(|t| t.as_str().to_string())
                        .unwrap_or_default(),
                    a.principal_id().unwrap_or_default().to_string(),
                )),
                Err(_) => break,
            }
        }
    }

    // Resolve principal ids → names, deduped.
    let mut names: HashMap<String, String> = HashMap::new();
    for (_, ptype, pid) in &raw {
        if pid.is_empty() || names.contains_key(pid) {
            continue;
        }
        let resolved = if ptype == "USER" {
            identitystore
                .describe_user()
                .identity_store_id(&identity_store_id)
                .user_id(pid)
                .send()
                .await
                .ok()
                .map(|u| {
                    let dn = u.display_name().unwrap_or_default().to_string();
                    if dn.is_empty() {
                        u.user_name().unwrap_or_default().to_string()
                    } else {
                        dn
                    }
                })
        } else if ptype == "GROUP" {
            identitystore
                .describe_group()
                .identity_store_id(&identity_store_id)
                .group_id(pid)
                .send()
                .await
                .ok()
                .map(|g| g.display_name().unwrap_or_default().to_string())
        } else {
            None
        };
        names.insert(
            pid.clone(),
            resolved.filter(|s| !s.is_empty()).unwrap_or_else(|| pid.clone()),
        );
    }

    let assignments = raw
        .into_iter()
        .map(|(account_id, principal_type, pid)| PsAssignment {
            account_id,
            principal_type,
            principal_name: names.get(&pid).cloned().unwrap_or_else(|| pid.clone()),
            principal_id: pid,
        })
        .collect();
    Ok(assignments)
}

// ── Group: lazy members ─────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct IcMember {
    pub user_id: String,
    pub name: String, // resolved display/user name (falls back to id)
}

/// Fetch a group's members (`ListGroupMemberships`) and resolve each member
/// user id to a name via `DescribeUser`.
pub async fn fetch_group_members(
    identitystore: IdentitystoreClient,
    identity_store_id: String,
    group_id: String,
) -> Result<Vec<IcMember>> {
    let mut user_ids: Vec<String> = Vec::new();
    let mut pager = identitystore
        .list_group_memberships()
        .identity_store_id(&identity_store_id)
        .group_id(&group_id)
        .into_paginator()
        .items()
        .send();
    while let Some(result) = pager.next().await {
        match result {
            Ok(m) => {
                if let Some(uid) = m.member_id().and_then(|mid| mid.as_user_id().ok()) {
                    user_ids.push(uid.clone());
                }
            }
            Err(e) => return Err(crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e))),
        }
    }

    let mut members = Vec::new();
    for uid in user_ids {
        let name = identitystore
            .describe_user()
            .identity_store_id(&identity_store_id)
            .user_id(&uid)
            .send()
            .await
            .ok()
            .map(|u| {
                let dn = u.display_name().unwrap_or_default().to_string();
                if dn.is_empty() {
                    u.user_name().unwrap_or_default().to_string()
                } else {
                    dn
                }
            })
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| uid.clone());
        members.push(IcMember { user_id: uid, name });
    }
    Ok(members)
}

// ── User: lazy group memberships ────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct IcGroupRef {
    pub group_id: String,
    pub name: String, // resolved group display name (falls back to id)
}

/// Fetch the groups a user belongs to (`ListGroupMembershipsForMember`) and
/// resolve each group id to its display name via `DescribeGroup`.
pub async fn fetch_user_groups(
    identitystore: IdentitystoreClient,
    identity_store_id: String,
    user_id: String,
) -> Result<Vec<IcGroupRef>> {
    let member = aws_sdk_identitystore::types::MemberId::UserId(user_id);
    let mut group_ids: Vec<String> = Vec::new();
    let mut pager = identitystore
        .list_group_memberships_for_member()
        .identity_store_id(&identity_store_id)
        .member_id(member)
        .into_paginator()
        .items()
        .send();
    while let Some(result) = pager.next().await {
        match result {
            Ok(m) => {
                if let Some(gid) = m.group_id() {
                    group_ids.push(gid.to_string());
                }
            }
            Err(e) => return Err(crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e))),
        }
    }

    let mut groups = Vec::new();
    for gid in group_ids {
        let name = identitystore
            .describe_group()
            .identity_store_id(&identity_store_id)
            .group_id(&gid)
            .send()
            .await
            .ok()
            .map(|g| g.display_name().unwrap_or_default().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| gid.clone());
        groups.push(IcGroupRef { group_id: gid, name });
    }
    Ok(groups)
}

// ── Application ───────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct IcApplication {
    pub arn: String,
    pub name: String,
    pub description: String,
    pub status: String, // ENABLED / DISABLED
    pub provider_arn: String,
    pub application_account: String,
    pub visibility: Option<String>,   // ENABLED / DISABLED (portal tile)
    pub sign_in_origin: Option<String>, // IDENTITY_CENTER / APPLICATION
    pub application_url: Option<String>,
    pub created_date: Option<String>,
    pub identity_store_id: String,
    pub tags: HashMap<String, String>,
}

impl IcApplication {
    pub fn from_sdk(app: &aws_sdk_ssoadmin::types::Application, identity_store_id: &str) -> Self {
        let portal = app.portal_options();
        Self {
            arn: app.application_arn().unwrap_or_default().to_string(),
            name: app.name().unwrap_or_default().to_string(),
            description: app.description().unwrap_or_default().to_string(),
            status: app
                .status()
                .map(|s| s.as_str().to_string())
                .unwrap_or_else(|| "unknown".to_string()),
            provider_arn: app.application_provider_arn().unwrap_or_default().to_string(),
            application_account: app.application_account().unwrap_or_default().to_string(),
            visibility: portal.map(|p| p.visibility().as_str().to_string()),
            sign_in_origin: portal
                .and_then(|p| p.sign_in_options())
                .map(|s| s.origin().as_str().to_string()),
            application_url: portal
                .and_then(|p| p.sign_in_options())
                .and_then(|s| s.application_url())
                .map(|s| s.to_string()),
            created_date: fmt_dt(app.created_date()),
            identity_store_id: identity_store_id.to_string(),
            tags: HashMap::new(),
        }
    }

    /// Friendly provider label from the provider ARN
    /// (`arn:aws:sso::aws:applicationProvider/<name>` → `<name>`).
    pub fn provider_name(&self) -> &str {
        self.provider_arn.split('/').next_back().unwrap_or_default()
    }
}

crate::sections! {
    pub enum IcApplicationDetailSection,
    pub static IC_APPLICATION_SECTIONS = [
        Overview "Overview",
        Assignments "Assignments" => crate::app::App::trigger_ic_app_assignments_load,
    ]
}

impl Resource for IcApplication {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&IC_APPLICATION_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.arn
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn resource_type(&self) -> &str {
        "IC Application"
    }

    fn state(&self) -> ResourceState {
        match self.status.as_str() {
            "ENABLED" => ResourceState::Available,
            "DISABLED" => ResourceState::Unavailable,
            other => ResourceState::Unknown(other.to_string()),
        }
    }

    fn state_label(&self) -> String {
        native_state_label(&self.status, || self.state())
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!(
            "{} {} {} {} {}",
            self.arn,
            self.name,
            self.description,
            self.provider_name(),
            self.status
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        let mut d = vec![
            ("Name".to_string(), self.name.clone()),
            ("Provider".to_string(), self.provider_name().to_string()),
            ("Status".to_string(), self.status.clone()),
            ("ARN".to_string(), self.arn.clone()),
        ];
        if !self.description.is_empty() {
            d.push(("Description".to_string(), self.description.clone()));
        }
        d
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{region}.console.aws.amazon.com/singlesignon/applications/home?region={region}"
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── Shared: resolve a USER/GROUP principal id to a display name ──────────────

async fn resolve_principal_name(
    identitystore: &IdentitystoreClient,
    identity_store_id: &str,
    principal_type: &str,
    principal_id: &str,
) -> String {
    let resolved = if principal_type == "USER" {
        identitystore
            .describe_user()
            .identity_store_id(identity_store_id)
            .user_id(principal_id)
            .send()
            .await
            .ok()
            .map(|u| {
                let dn = u.display_name().unwrap_or_default().to_string();
                if dn.is_empty() {
                    u.user_name().unwrap_or_default().to_string()
                } else {
                    dn
                }
            })
    } else if principal_type == "GROUP" {
        identitystore
            .describe_group()
            .identity_store_id(identity_store_id)
            .group_id(principal_id)
            .send()
            .await
            .ok()
            .map(|g| g.display_name().unwrap_or_default().to_string())
    } else {
        None
    };
    resolved
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| principal_id.to_string())
}

// ── Application: lazy assignments ─────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct IcAppPrincipal {
    pub principal_type: String, // USER / GROUP
    pub principal_name: String, // resolved (falls back to id)
    pub principal_id: String,
}

#[derive(Clone, Debug)]
pub struct IcAppAssignments {
    /// From GetApplicationAssignmentConfiguration — None when the call failed.
    pub assignment_required: Option<bool>,
    pub assignments: Vec<IcAppPrincipal>,
}

/// Fetch who is assigned to an Identity Center application, with principal
/// names resolved, plus whether assignment is required to access it.
pub async fn fetch_app_assignments(
    ssoadmin: SsoAdminClient,
    identitystore: IdentitystoreClient,
    identity_store_id: String,
    app_arn: String,
) -> Result<IcAppAssignments> {
    let assignment_required = ssoadmin
        .get_application_assignment_configuration()
        .application_arn(&app_arn)
        .send()
        .await
        .ok()
        .map(|o| o.assignment_required());

    let mut raw: Vec<(String, String)> = Vec::new();
    let mut pager = ssoadmin
        .list_application_assignments()
        .application_arn(&app_arn)
        .into_paginator()
        .items()
        .send();
    while let Some(result) = pager.next().await {
        match result {
            Ok(a) => raw.push((
                a.principal_type().as_str().to_string(),
                a.principal_id().to_string(),
            )),
            Err(e) => {
                return Err(crate::error::Error::AwsSdk(crate::error::sdk_error_message(
                    &e,
                )))
            }
        }
    }

    let mut names: HashMap<String, String> = HashMap::new();
    let mut assignments = Vec::new();
    for (ptype, pid) in raw {
        if !names.contains_key(&pid) {
            let name = resolve_principal_name(&identitystore, &identity_store_id, &ptype, &pid).await;
            names.insert(pid.clone(), name);
        }
        assignments.push(IcAppPrincipal {
            principal_type: ptype,
            principal_name: names.get(&pid).cloned().unwrap_or_else(|| pid.clone()),
            principal_id: pid,
        });
    }

    Ok(IcAppAssignments {
        assignment_required,
        assignments,
    })
}

// ── Principal access: user/group → (account × permission set) ────────────────

#[derive(Clone, Debug)]
pub struct IcAccessEntry {
    pub account_id: String,
    pub ps_arn: String,
    pub ps_name: String,            // resolved from loaded siblings (falls back to ARN tail)
    pub via_group: Option<String>,  // None = direct assignment; Some(name) = inherited
}

/// One page-through of `ListAccountAssignmentsForPrincipal` for a principal.
async fn list_assignments_for_principal(
    ssoadmin: &SsoAdminClient,
    instance_arn: &str,
    principal_id: &str,
    principal_type: aws_sdk_ssoadmin::types::PrincipalType,
) -> std::result::Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    let mut pager = ssoadmin
        .list_account_assignments_for_principal()
        .instance_arn(instance_arn)
        .principal_id(principal_id)
        .principal_type(principal_type)
        .into_paginator()
        .items()
        .send();
    while let Some(result) = pager.next().await {
        match result {
            Ok(a) => out.push((
                a.account_id().unwrap_or_default().to_string(),
                a.permission_set_arn().unwrap_or_default().to_string(),
            )),
            Err(e) => return Err(crate::error::sdk_error_message(&e)),
        }
    }
    Ok(out)
}

/// Answer "what access does this principal have?" — every (account, permission
/// set) pair via `ListAccountAssignmentsForPrincipal`. For users this is
/// *effective* access: direct assignments plus assignments inherited through
/// each group membership (annotated with the group name). Permission set ARNs
/// resolve to names via the `ps_names` map captured from the already-loaded
/// list (zero extra API calls); unknown ARNs fall back to the ARN tail.
pub async fn fetch_principal_access(
    ssoadmin: SsoAdminClient,
    identitystore: IdentitystoreClient,
    instance_arn: String,
    identity_store_id: String,
    principal_id: String,
    is_group: bool,
    ps_names: HashMap<String, String>,
) -> Result<Vec<IcAccessEntry>> {
    use aws_sdk_ssoadmin::types::PrincipalType;

    let ps_name = |arn: &str| {
        ps_names
            .get(arn)
            .cloned()
            .unwrap_or_else(|| arn.split('/').next_back().unwrap_or(arn).to_string())
    };

    let ptype = if is_group {
        PrincipalType::Group
    } else {
        PrincipalType::User
    };
    // The direct lookup is the meaningful permission check — propagate its error.
    let direct = list_assignments_for_principal(&ssoadmin, &instance_arn, &principal_id, ptype)
        .await
        .map_err(crate::error::Error::AwsSdk)?;

    let mut entries: Vec<IcAccessEntry> = direct
        .into_iter()
        .map(|(account_id, ps_arn)| IcAccessEntry {
            account_id,
            ps_name: ps_name(&ps_arn),
            ps_arn,
            via_group: None,
        })
        .collect();

    // Users also inherit access through their groups — fold those in,
    // best-effort per group (a gap on one group doesn't drop the rest).
    if !is_group {
        let groups = fetch_user_groups(
            identitystore.clone(),
            identity_store_id.clone(),
            principal_id.clone(),
        )
        .await
        .unwrap_or_default();
        for g in groups {
            if let Ok(list) = list_assignments_for_principal(
                &ssoadmin,
                &instance_arn,
                &g.group_id,
                PrincipalType::Group,
            )
            .await
            {
                entries.extend(list.into_iter().map(|(account_id, ps_arn)| IcAccessEntry {
                    account_id,
                    ps_name: ps_name(&ps_arn),
                    ps_arn,
                    via_group: Some(g.name.clone()),
                }));
            }
        }
    }

    // Direct first, then by group, account, permission set — stable read order.
    entries.sort_by(|a, b| {
        (a.via_group.is_some(), &a.via_group, &a.account_id, &a.ps_name)
            .cmp(&(b.via_group.is_some(), &b.via_group, &b.account_id, &b.ps_name))
    });
    Ok(entries)
}

// ── Instance: lazy ABAC configuration ─────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct IcAbac {
    pub status: String, // ENABLED / CREATION_IN_PROGRESS / CREATION_FAILED
    pub status_reason: Option<String>,
    /// (attribute key, comma-joined sources)
    pub attributes: Vec<(String, String)>,
}

/// Fetch the instance's attribute-based access control configuration.
/// A ResourceNotFound means ABAC was never configured — `Ok(None)`, not an error.
pub async fn fetch_instance_abac(
    ssoadmin: SsoAdminClient,
    instance_arn: String,
) -> std::result::Result<Option<IcAbac>, String> {
    use aws_sdk_ssoadmin::error::ProvideErrorMetadata;
    match ssoadmin
        .describe_instance_access_control_attribute_configuration()
        .instance_arn(&instance_arn)
        .send()
        .await
    {
        Ok(o) => {
            let attributes = o
                .instance_access_control_attribute_configuration()
                .map(|c| {
                    c.access_control_attributes()
                        .iter()
                        .map(|a| {
                            let sources = a
                                .value()
                                .map(|v| v.source().join(", "))
                                .unwrap_or_default();
                            (a.key().to_string(), sources)
                        })
                        .collect()
                })
                .unwrap_or_default();
            Ok(Some(IcAbac {
                status: o
                    .status()
                    .map(|s| s.as_str().to_string())
                    .unwrap_or_else(|| "unknown".to_string()),
                status_reason: o.status_reason().map(|s| s.to_string()),
                attributes,
            }))
        }
        Err(e) => {
            if e.code() == Some("ResourceNotFoundException") {
                Ok(None)
            } else {
                Err(crate::error::sdk_error_message(&e))
            }
        }
    }
}

// ── Instance: lazy trusted token issuers ─────────────────────────────────────

#[derive(Clone, Debug)]
pub struct IcTti {
    pub arn: String,
    pub name: String,
    pub issuer_type: String, // OIDC_JWT
    pub issuer_url: Option<String>,
    pub claim_attribute_path: Option<String>,
    pub identity_store_attribute_path: Option<String>,
    pub jwks_retrieval: Option<String>,
}

/// Fetch the instance's trusted token issuers (trusted identity propagation) —
/// the list is small, so each is deepened with a DescribeTrustedTokenIssuer.
pub async fn fetch_trusted_token_issuers(
    ssoadmin: SsoAdminClient,
    instance_arn: String,
) -> Result<Vec<IcTti>> {
    let mut metas: Vec<(String, String, String)> = Vec::new();
    let mut pager = ssoadmin
        .list_trusted_token_issuers()
        .instance_arn(&instance_arn)
        .into_paginator()
        .items()
        .send();
    while let Some(result) = pager.next().await {
        match result {
            Ok(t) => metas.push((
                t.trusted_token_issuer_arn().unwrap_or_default().to_string(),
                t.name().unwrap_or_default().to_string(),
                t.trusted_token_issuer_type()
                    .map(|t| t.as_str().to_string())
                    .unwrap_or_default(),
            )),
            Err(e) => {
                return Err(crate::error::Error::AwsSdk(crate::error::sdk_error_message(
                    &e,
                )))
            }
        }
    }

    let mut issuers = Vec::new();
    for (arn, name, issuer_type) in metas {
        let mut tti = IcTti {
            arn: arn.clone(),
            name,
            issuer_type,
            issuer_url: None,
            claim_attribute_path: None,
            identity_store_attribute_path: None,
            jwks_retrieval: None,
        };
        if let Ok(d) = ssoadmin
            .describe_trusted_token_issuer()
            .trusted_token_issuer_arn(&arn)
            .send()
            .await
        {
            if let Some(cfg) = d
                .trusted_token_issuer_configuration()
                .and_then(|c| c.as_oidc_jwt_configuration().ok())
            {
                tti.issuer_url = Some(cfg.issuer_url().to_string());
                tti.claim_attribute_path = Some(cfg.claim_attribute_path().to_string());
                tti.identity_store_attribute_path =
                    Some(cfg.identity_store_attribute_path().to_string());
                tti.jwks_retrieval = Some(cfg.jwks_retrieval_option().as_str().to_string());
            }
        }
        issuers.push(tti);
    }
    Ok(issuers)
}

// ── Account-name map (Organizations, best-effort) ─────────────────────────────

/// Resolve account ids to names via `organizations:ListAccounts` so assignment
/// rows read `123456789012 (prod)`. Best-effort: a non-management account or a
/// permission gap yields an empty map and rows fall back to bare ids.
pub async fn fetch_ic_account_names(
    client: aws_sdk_organizations::Client,
) -> HashMap<String, String> {
    let mut names = HashMap::new();
    let mut pager = client.list_accounts().into_paginator().send();
    while let Some(result) = pager.next().await {
        match result {
            Ok(page) => {
                for a in page.accounts() {
                    if let (Some(id), Some(name)) = (a.id(), a.name()) {
                        names.insert(id.to_string(), name.to_string());
                    }
                }
            }
            Err(_) => break,
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aws::resource::ResourceState;

    fn sample_app() -> IcApplication {
        IcApplication {
            arn: "arn:aws:sso::123456789012:application/ssoins-abc/apl-def".to_string(),
            name: "Athena".to_string(),
            description: String::new(),
            status: "ENABLED".to_string(),
            provider_arn: "arn:aws:sso::aws:applicationProvider/athena".to_string(),
            application_account: "123456789012".to_string(),
            visibility: Some("ENABLED".to_string()),
            sign_in_origin: Some("IDENTITY_CENTER".to_string()),
            application_url: None,
            created_date: None,
            identity_store_id: "d-123".to_string(),
            tags: HashMap::new(),
        }
    }

    #[test]
    fn application_provider_name_is_arn_tail() {
        assert_eq!(sample_app().provider_name(), "athena");
    }

    #[test]
    fn application_state_maps_status() {
        let mut app = sample_app();
        assert_eq!(app.state(), ResourceState::Available);
        app.status = "DISABLED".to_string();
        assert_eq!(app.state(), ResourceState::Unavailable);
    }

    #[test]
    fn disabled_user_reads_unavailable() {
        let user = IcUser {
            user_id: "u-1".to_string(),
            username: "jdoe".to_string(),
            display_name: "Jane Doe".to_string(),
            email: None,
            status: Some("DISABLED".to_string()),
            title: None,
            user_type: None,
            given_name: None,
            family_name: None,
            external_ids: vec![],
            created_at: None,
            created_by: None,
            updated_at: None,
            updated_by: None,
            identity_store_id: "d-123".to_string(),
            instance_arn: "arn:aws:sso:::instance/ssoins-abc".to_string(),
            tags: HashMap::new(),
        };
        assert_eq!(user.state(), ResourceState::Unavailable);
        let enabled = IcUser {
            status: Some("ENABLED".to_string()),
            ..user
        };
        assert_eq!(enabled.state(), ResourceState::Available);
    }
}
