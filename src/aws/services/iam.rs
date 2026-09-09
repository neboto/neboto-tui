use crate::aws::client::AwsClients;
use crate::aws::resource::{native_state_label, Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use aws_sdk_iam::Client as IamClient;
use std::any::Any;
use std::collections::HashMap;
use tokio::sync::mpsc;

pub struct IamService {
    client: IamClient,
    analyzer_client: aws_sdk_accessanalyzer::Client,
}

impl IamService {
    pub fn new(aws_clients: &AwsClients) -> Self {
        Self {
            client: aws_clients.iam_client(),
            analyzer_client: aws_clients.accessanalyzer_client(),
        }
    }
}

#[async_trait]
impl AwsService for IamService {
    fn service_type(&self) -> ServiceType {
        ServiceType::IAM
    }

    fn name(&self) -> &str {
        "IAM"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        self.list_resources_streaming(tx, ServiceType::IAM).await?;
        Ok(vec![])
    }

    async fn list_resources_streaming(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        let mut total = 0usize;

        // Phase failures are non-fatal: a permission gap on one list call must
        // not abort the phases behind it. Each failure records a warning
        // (surfaced when the load completes); only all four core phases
        // failing together is a hard error.
        let mut phase_failures: Vec<String> = Vec::new();

        // Phase 1: Roles
        let mut paginator = self.client.list_roles().into_paginator().send();
        while let Some(result) = paginator.next().await {
            match result {
                Ok(page) => {
                    let batch: Vec<Box<dyn Resource>> = page
                        .roles()
                        .iter()
                        .map(|r| Box::new(IamRole::from_sdk(r)) as Box<dyn Resource>)
                        .collect();
                    let count = batch.len();
                    if count == 0 {
                        continue;
                    }
                    total += count;
                    let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                        service: service_type,
                        resources: batch,
                        progress: LoadProgress {
                            loaded_count: total,
                            total_count: None,
                            status_message: Some("Loading roles...".to_string()),
                        },
                    });
                }
                Err(e) => {
                    let msg = format!("roles: {}", crate::error::sdk_error_message(&e));
                    let _ = event_tx.send(Event::ResourceLoadWarning {
                        service: service_type,
                        warning: msg.clone(),
                    });
                    phase_failures.push(msg);
                    break;
                }
            }
        }

        // Phase 2: Policies (customer-managed only, to skip AWS-managed noise)
        let mut paginator = self
            .client
            .list_policies()
            .scope(aws_sdk_iam::types::PolicyScopeType::Local)
            .into_paginator()
            .send();
        while let Some(result) = paginator.next().await {
            match result {
                Ok(page) => {
                    let batch: Vec<Box<dyn Resource>> = page
                        .policies()
                        .iter()
                        .map(|p| Box::new(IamPolicy::from_sdk(p)) as Box<dyn Resource>)
                        .collect();
                    let count = batch.len();
                    if count == 0 {
                        continue;
                    }
                    total += count;
                    let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                        service: service_type,
                        resources: batch,
                        progress: LoadProgress {
                            loaded_count: total,
                            total_count: None,
                            status_message: Some("Loading policies...".to_string()),
                        },
                    });
                }
                Err(e) => {
                    let msg = format!("policies: {}", crate::error::sdk_error_message(&e));
                    let _ = event_tx.send(Event::ResourceLoadWarning {
                        service: service_type,
                        warning: msg.clone(),
                    });
                    phase_failures.push(msg);
                    break;
                }
            }
        }

        // Phase 3: Users
        let mut paginator = self.client.list_users().into_paginator().send();
        while let Some(result) = paginator.next().await {
            match result {
                Ok(page) => {
                    let batch: Vec<Box<dyn Resource>> = page
                        .users()
                        .iter()
                        .map(|u| Box::new(IamUser::from_sdk(u)) as Box<dyn Resource>)
                        .collect();
                    let count = batch.len();
                    if count == 0 {
                        continue;
                    }
                    total += count;
                    let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                        service: service_type,
                        resources: batch,
                        progress: LoadProgress {
                            loaded_count: total,
                            total_count: None,
                            status_message: Some("Loading users...".to_string()),
                        },
                    });
                }
                Err(e) => {
                    let msg = format!("users: {}", crate::error::sdk_error_message(&e));
                    let _ = event_tx.send(Event::ResourceLoadWarning {
                        service: service_type,
                        warning: msg.clone(),
                    });
                    phase_failures.push(msg);
                    break;
                }
            }
        }

        // Phase 4: Groups
        let mut paginator = self.client.list_groups().into_paginator().send();
        while let Some(result) = paginator.next().await {
            match result {
                Ok(page) => {
                    let batch: Vec<Box<dyn Resource>> = page
                        .groups()
                        .iter()
                        .map(|g| Box::new(IamGroup::from_sdk(g)) as Box<dyn Resource>)
                        .collect();
                    let count = batch.len();
                    if count == 0 {
                        continue;
                    }
                    total += count;
                    let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                        service: service_type,
                        resources: batch,
                        progress: LoadProgress {
                            loaded_count: total,
                            total_count: None,
                            status_message: Some("Loading groups...".to_string()),
                        },
                    });
                }
                Err(e) => {
                    let msg = format!("groups: {}", crate::error::sdk_error_message(&e));
                    let _ = event_tx.send(Event::ResourceLoadWarning {
                        service: service_type,
                        warning: msg.clone(),
                    });
                    phase_failures.push(msg);
                    break;
                }
            }
        }

        // All four core phases failed: the load as a whole failed. A hard
        // error reads better than an empty list with a partial-load warning.
        if phase_failures.len() == 4 {
            let _ = event_tx.send(Event::ResourceLoadError {
                service: service_type,
                error: format!("Failed to load IAM — {}", phase_failures.join("; ")),
            });
            return Ok(());
        }

        // Phase 5: Identity providers (SAML + OIDC — both unpaginated). A
        // failure warns like the core phases (these normally succeed) but
        // doesn't count toward the all-failed hard error above.
        let mut idp_batch: Vec<Box<dyn Resource>> = Vec::new();
        match self.client.list_saml_providers().send().await {
            Ok(resp) => {
                for p in resp.saml_provider_list() {
                    idp_batch.push(Box::new(IamIdentityProvider::from_saml(p)) as Box<dyn Resource>);
                }
            }
            Err(e) => {
                let _ = event_tx.send(Event::ResourceLoadWarning {
                    service: service_type,
                    warning: format!("SAML providers: {}", crate::error::sdk_error_message(&e)),
                });
            }
        }
        match self.client.list_open_id_connect_providers().send().await {
            Ok(resp) => {
                for p in resp.open_id_connect_provider_list() {
                    idp_batch.push(Box::new(IamIdentityProvider::from_oidc(p)) as Box<dyn Resource>);
                }
            }
            Err(e) => {
                let _ = event_tx.send(Event::ResourceLoadWarning {
                    service: service_type,
                    warning: format!("OIDC providers: {}", crate::error::sdk_error_message(&e)),
                });
            }
        }
        if !idp_batch.is_empty() {
            total += idp_batch.len();
            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                service: service_type,
                resources: idp_batch,
                progress: LoadProgress {
                    loaded_count: total,
                    total_count: None,
                    status_message: Some("Loading identity providers...".to_string()),
                },
            });
        }

        // Phase 6: account-level settings — always one synthetic row; call
        // failures render inline in its sections rather than warning here.
        let settings = fetch_account_settings(&self.client).await;
        total += 1;
        let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
            service: service_type,
            resources: vec![Box::new(settings) as Box<dyn Resource>],
            progress: LoadProgress {
                loaded_count: total,
                total_count: None,
                status_message: Some("Loading account settings...".to_string()),
            },
        });

        // Phase 7: Access Analyzer findings (external-access). Entirely
        // error-tolerant — no analyzer / a permission gap must not break the
        // core IAM load, which has already streamed above.
        if let Ok(analyzers) = self.analyzer_client.list_analyzers().send().await {
            let arns: Vec<String> = analyzers
                .analyzers()
                .iter()
                .map(|a| a.arn().to_string())
                .collect();
            for arn in arns {
                let mut batch: Vec<Box<dyn Resource>> = Vec::new();
                let mut paginator = self
                    .analyzer_client
                    .list_findings()
                    .analyzer_arn(&arn)
                    .into_paginator()
                    .send();
                while let Some(result) = paginator.next().await {
                    let page = match result {
                        Ok(p) => p,
                        Err(_) => break, // unused-access analyzers reject list_findings; skip
                    };
                    for f in page.findings() {
                        // Only surface ACTIVE findings (archived/resolved = noise).
                        if f.status().as_str() != "ACTIVE" {
                            continue;
                        }
                        batch.push(
                            Box::new(AccessAnalyzerFinding::from_sdk(f, &arn)) as Box<dyn Resource>
                        );
                    }
                }
                if batch.is_empty() {
                    continue;
                }
                total += batch.len();
                let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                    service: service_type,
                    resources: batch,
                    progress: LoadProgress {
                        loaded_count: total,
                        total_count: None,
                        status_message: Some("Loading Access Analyzer findings...".to_string()),
                    },
                });
            }
        }

        let _ = event_tx.send(Event::ResourcesFullyLoaded {
            service: service_type,
            total_count: total,
        });

        Ok(())
    }

    async fn get_resource_details(&self, _id: &str) -> Result<Box<dyn Resource>> {
        Err(crate::error::Error::NotImplemented)
    }
}

// ── Shared helpers ──────────────────────────────────────────────────────────

fn tags_from_sdk(tags: &[aws_sdk_iam::types::Tag]) -> HashMap<String, String> {
    tags.iter()
        .map(|t| (t.key().to_string(), t.value().to_string()))
        .collect()
}

fn fmt_datetime(dt: Option<&aws_sdk_iam::primitives::DateTime>) -> Option<String> {
    dt.map(|d| {
        let secs = d.secs();
        fmt_epoch_secs(secs)
    })
}

/// Percent-decode a URL-encoded string (IAM returns policy documents this way).
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
                if let Some(h) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    out.push(h);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Decode and pretty-print a (possibly percent-encoded) JSON policy document.
pub fn pretty_policy_document(raw: &str) -> String {
    let decoded = percent_decode(raw);
    match serde_json::from_str::<serde_json::Value>(&decoded) {
        Ok(v) => serde_json::to_string_pretty(&v).unwrap_or(decoded),
        Err(_) => decoded,
    }
}

// ── IamRole ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct IamRole {
    pub role_name: String,
    pub arn: String,
    pub role_id: String,
    pub path: String,
    pub create_date: Option<String>,
    pub max_session_duration: Option<i32>,
    pub description: Option<String>,
    pub trust_policy_raw: Option<String>,
    pub tags: HashMap<String, String>,
}

impl IamRole {
    pub fn from_sdk(r: &aws_sdk_iam::types::Role) -> Self {
        Self {
            role_name: r.role_name().to_string(),
            arn: r.arn().to_string(),
            role_id: r.role_id().to_string(),
            path: r.path().to_string(),
            create_date: fmt_datetime(Some(r.create_date())),
            max_session_duration: r.max_session_duration(),
            description: r.description().map(|s| s.to_string()),
            trust_policy_raw: r.assume_role_policy_document().map(|s| s.to_string()),
            tags: tags_from_sdk(r.tags()),
        }
    }
}

crate::sections! {
    pub enum IamRoleDetailSection,
    pub static IAM_ROLE_SECTIONS = [
        // Overview and Tags need the lazy GetRole too: ListRoles omits
        // RoleLastUsed, PermissionsBoundary and Tags (documented API behavior),
        // so those rows only exist on the lazy bundle.
        Overview "Overview" => crate::app::App::trigger_iam_role_details_load,
        TrustPolicy "Trust Policy",
        Permissions "Permissions" => crate::app::App::trigger_iam_role_details_load,
        Tags "Tags" => crate::app::App::trigger_iam_role_details_load,
    ]
}

impl Resource for IamRole {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&IAM_ROLE_SECTIONS)
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws iam get-role --role-name {}",
            crate::aws::resource::shell_quote(self.name())
        ))
    }

    fn id(&self) -> &str {
        &self.arn
    }

    fn name(&self) -> &str {
        &self.role_name
    }

    fn resource_type(&self) -> &str {
        "IAM Role"
    }

    fn state(&self) -> ResourceState {
        ResourceState::stateless()
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!("{} {} {}", self.role_name, self.arn, self.path)
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Role Name".to_string(), self.role_name.clone()),
            ("ARN".to_string(), self.arn.clone()),
            ("Role ID".to_string(), self.role_id.clone()),
            ("Path".to_string(), self.path.clone()),
            (
                "Created".to_string(),
                self.create_date.clone().unwrap_or_else(|| "—".to_string()),
            ),
        ]
    }

    fn console_url(&self, _region: &str) -> Option<String> {
        Some(format!(
            "https://us-east-1.console.aws.amazon.com/iam/home#/roles/details/{}",
            self.role_name
        ))
    }

    fn trail_lookup_keys(&self) -> Vec<String> {
        // CloudTrail records IAM role events by name (and sometimes ARN).
        vec![self.role_name.clone(), self.arn.clone()]
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── IamPolicy ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct IamPolicy {
    pub policy_arn: String,
    pub policy_name: String,
    pub policy_id: String,
    pub path: String,
    pub default_version_id: String,
    pub attachment_count: i32,
    pub description: Option<String>,
    pub create_date: Option<String>,
    pub update_date: Option<String>,
    pub tags: HashMap<String, String>,
}

impl IamPolicy {
    pub fn from_sdk(p: &aws_sdk_iam::types::Policy) -> Self {
        Self {
            policy_arn: p.arn().unwrap_or_default().to_string(),
            policy_name: p.policy_name().unwrap_or_default().to_string(),
            policy_id: p.policy_id().unwrap_or_default().to_string(),
            path: p.path().unwrap_or_default().to_string(),
            default_version_id: p.default_version_id().unwrap_or_default().to_string(),
            attachment_count: p.attachment_count().unwrap_or(0),
            description: p.description().map(|s| s.to_string()),
            create_date: fmt_datetime(p.create_date()),
            update_date: fmt_datetime(p.update_date()),
            tags: tags_from_sdk(p.tags()),
        }
    }
}

crate::sections! {
    pub enum IamPolicyDetailSection,
    pub static IAM_POLICY_SECTIONS = [
        Overview "Overview",
        Document "Document" => crate::app::App::trigger_iam_policy_details_load,
        AttachedTo "Attached To" => crate::app::App::trigger_iam_policy_details_load,
        // ListPolicies omits Tags — they ride the lazy bundle (ListPolicyTags).
        Tags "Tags" => crate::app::App::trigger_iam_policy_details_load,
    ]
}

impl Resource for IamPolicy {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&IAM_POLICY_SECTIONS)
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws iam get-policy --policy-arn {}",
            crate::aws::resource::shell_quote(self.id())
        ))
    }

    fn id(&self) -> &str {
        &self.policy_arn
    }

    fn name(&self) -> &str {
        &self.policy_name
    }

    fn resource_type(&self) -> &str {
        "IAM Policy"
    }

    fn state(&self) -> ResourceState {
        ResourceState::stateless()
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!("{} {}", self.policy_name, self.policy_arn)
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Policy Name".to_string(), self.policy_name.clone()),
            ("ARN".to_string(), self.policy_arn.clone()),
            ("Path".to_string(), self.path.clone()),
            (
                "Attachments".to_string(),
                self.attachment_count.to_string(),
            ),
            (
                "Description".to_string(),
                self.description.clone().unwrap_or_else(|| "—".to_string()),
            ),
        ]
    }

    fn console_url(&self, _region: &str) -> Option<String> {
        Some(format!(
            "https://us-east-1.console.aws.amazon.com/iam/home#/policies/{}",
            self.policy_arn
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── IamUser ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct IamUser {
    pub user_name: String,
    pub arn: String,
    pub user_id: String,
    pub path: String,
    pub create_date: Option<String>,
    pub password_last_used: Option<String>,
    pub tags: HashMap<String, String>,
}

impl IamUser {
    pub fn from_sdk(u: &aws_sdk_iam::types::User) -> Self {
        Self {
            user_name: u.user_name().to_string(),
            arn: u.arn().to_string(),
            user_id: u.user_id().to_string(),
            path: u.path().to_string(),
            create_date: fmt_datetime(Some(u.create_date())),
            password_last_used: fmt_datetime(u.password_last_used()),
            tags: tags_from_sdk(u.tags()),
        }
    }
}

crate::sections! {
    pub enum IamUserDetailSection,
    pub static IAM_USER_SECTIONS = [
        Access "Access" => crate::app::App::trigger_iam_user_details_load,
        Permissions "Permissions" => crate::app::App::trigger_iam_user_details_load,
        Groups "Groups" => crate::app::App::trigger_iam_user_details_load,
        Tags "Tags" => crate::app::App::trigger_iam_user_details_load,
    ]
}

impl Resource for IamUser {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&IAM_USER_SECTIONS)
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws iam get-user --user-name {}",
            crate::aws::resource::shell_quote(self.name())
        ))
    }

    fn id(&self) -> &str {
        &self.arn
    }

    fn name(&self) -> &str {
        &self.user_name
    }

    fn resource_type(&self) -> &str {
        "IAM User"
    }

    fn state(&self) -> ResourceState {
        ResourceState::stateless()
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!("{} {}", self.user_name, self.arn)
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("User Name".to_string(), self.user_name.clone()),
            ("ARN".to_string(), self.arn.clone()),
            ("User ID".to_string(), self.user_id.clone()),
            ("Path".to_string(), self.path.clone()),
            (
                "Created".to_string(),
                self.create_date.clone().unwrap_or_else(|| "—".to_string()),
            ),
            (
                "Password Last Used".to_string(),
                self.password_last_used
                    .clone()
                    .unwrap_or_else(|| "Never".to_string()),
            ),
        ]
    }

    fn console_url(&self, _region: &str) -> Option<String> {
        Some(format!(
            "https://us-east-1.console.aws.amazon.com/iam/home#/users/details/{}",
            self.user_name
        ))
    }

    fn trail_lookup_keys(&self) -> Vec<String> {
        vec![self.user_name.clone(), self.arn.clone()]
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── IamGroup ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct IamGroup {
    pub group_name: String,
    pub arn: String,
    pub group_id: String,
    pub path: String,
    pub create_date: Option<String>,
    pub tags: HashMap<String, String>,
}

impl IamGroup {
    pub fn from_sdk(g: &aws_sdk_iam::types::Group) -> Self {
        Self {
            group_name: g.group_name().to_string(),
            arn: g.arn().to_string(),
            group_id: g.group_id().to_string(),
            path: g.path().to_string(),
            create_date: fmt_datetime(Some(g.create_date())),
            tags: HashMap::new(),
        }
    }
}

crate::sections! {
    pub enum IamGroupDetailSection,
    pub static IAM_GROUP_SECTIONS = [
        Overview "Overview",
        Members "Members" => crate::app::App::trigger_iam_group_details_load,
        Permissions "Permissions" => crate::app::App::trigger_iam_group_details_load,
    ]
}

impl Resource for IamGroup {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&IAM_GROUP_SECTIONS)
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws iam get-group --group-name {}",
            crate::aws::resource::shell_quote(self.name())
        ))
    }

    fn id(&self) -> &str {
        &self.arn
    }

    fn name(&self) -> &str {
        &self.group_name
    }

    fn resource_type(&self) -> &str {
        "IAM Group"
    }

    fn state(&self) -> ResourceState {
        ResourceState::stateless()
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!("{} {}", self.group_name, self.arn)
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Group Name".to_string(), self.group_name.clone()),
            ("ARN".to_string(), self.arn.clone()),
            ("Group ID".to_string(), self.group_id.clone()),
            ("Path".to_string(), self.path.clone()),
            (
                "Created".to_string(),
                self.create_date.clone().unwrap_or_else(|| "—".to_string()),
            ),
        ]
    }

    fn console_url(&self, _region: &str) -> Option<String> {
        Some(format!(
            "https://us-east-1.console.aws.amazon.com/iam/home#/groups/details/{}",
            self.group_name
        ))
    }

    fn trail_lookup_keys(&self) -> Vec<String> {
        vec![self.group_name.clone(), self.arn.clone()]
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── Lazy-loaded role details (permissions) ──────────────────────────────────

#[derive(Debug, Clone)]
pub struct IamRoleDetails {
    pub managed_policies: Vec<(String, String)>, // (name, arn)
    pub inline_policy_names: Vec<String>,
    // From GetRole — ListRoles omits these attributes entirely.
    pub last_used_date: Option<String>,
    pub last_used_region: Option<String>,
    pub permissions_boundary: Option<String>,
    pub tags: HashMap<String, String>,
}

pub async fn fetch_iam_role_details(
    client: IamClient,
    role_name: String,
) -> Result<IamRoleDetails> {
    // Best-effort: the policy lists below are the section's core; a denied
    // GetRole just leaves last-used/boundary/tags blank.
    let (mut last_used_date, mut last_used_region, mut permissions_boundary) = (None, None, None);
    let mut tags = HashMap::new();
    if let Ok(resp) = client.get_role().role_name(&role_name).send().await {
        if let Some(role) = resp.role() {
            if let Some(lu) = role.role_last_used() {
                last_used_date = fmt_datetime(lu.last_used_date());
                last_used_region = lu.region().map(|s| s.to_string());
            }
            permissions_boundary = role
                .permissions_boundary()
                .and_then(|b| b.permissions_boundary_arn())
                .map(|s| s.to_string());
            tags = tags_from_sdk(role.tags());
        }
    }

    let mut managed_policies = Vec::new();
    let mut paginator = client
        .list_attached_role_policies()
        .role_name(&role_name)
        .into_paginator()
        .send();
    while let Some(Ok(page)) = paginator.next().await {
        for p in page.attached_policies() {
            managed_policies.push((
                p.policy_name().unwrap_or_default().to_string(),
                p.policy_arn().unwrap_or_default().to_string(),
            ));
        }
    }

    let mut inline_policy_names = Vec::new();
    let mut paginator = client
        .list_role_policies()
        .role_name(&role_name)
        .into_paginator()
        .send();
    while let Some(Ok(page)) = paginator.next().await {
        for name in page.policy_names() {
            inline_policy_names.push(name.to_string());
        }
    }

    Ok(IamRoleDetails {
        managed_policies,
        inline_policy_names,
        last_used_date,
        last_used_region,
        permissions_boundary,
        tags,
    })
}

// ── Lazy-loaded policy details (document + attachments) ─────────────────────

#[derive(Debug, Clone)]
pub struct IamPolicyDetails {
    pub document_json: String,
    pub attached_roles: Vec<String>,
    pub attached_users: Vec<String>,
    pub attached_groups: Vec<String>,
    // From ListPolicyTags — ListPolicies omits Tags.
    pub tags: HashMap<String, String>,
}

pub async fn fetch_iam_policy_details(
    client: IamClient,
    policy_arn: String,
    default_version_id: String,
) -> Result<IamPolicyDetails> {
    let version_resp = client
        .get_policy_version()
        .policy_arn(&policy_arn)
        .version_id(&default_version_id)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;

    let document_json = version_resp
        .policy_version()
        .and_then(|v| v.document())
        .map(pretty_policy_document)
        .unwrap_or_else(|| "No policy document".to_string());

    let mut attached_roles = Vec::new();
    let mut attached_users = Vec::new();
    let mut attached_groups = Vec::new();
    let mut paginator = client
        .list_entities_for_policy()
        .policy_arn(&policy_arn)
        .into_paginator()
        .send();
    while let Some(Ok(page)) = paginator.next().await {
        for r in page.policy_roles() {
            if let Some(name) = r.role_name() {
                attached_roles.push(name.to_string());
            }
        }
        for u in page.policy_users() {
            if let Some(name) = u.user_name() {
                attached_users.push(name.to_string());
            }
        }
        for g in page.policy_groups() {
            if let Some(name) = g.group_name() {
                attached_groups.push(name.to_string());
            }
        }
    }

    // Best-effort — a denied ListPolicyTags leaves the Tags section empty
    // rather than failing the document + attachments above.
    let tags = match client.list_policy_tags().policy_arn(&policy_arn).send().await {
        Ok(r) => tags_from_sdk(r.tags()),
        Err(_) => HashMap::new(),
    };

    Ok(IamPolicyDetails {
        document_json,
        attached_roles,
        attached_users,
        attached_groups,
        tags,
    })
}

// ── Lazy-loaded IAM user details (access keys / MFA / groups / policies) ────

#[derive(Debug, Clone)]
pub struct IamAccessKey {
    pub id: String,
    pub status: String, // Active / Inactive
    pub created: Option<String>,
    pub last_used: Option<String>, // date, or None = never used
}

#[derive(Debug, Clone)]
pub struct IamUserDetails {
    pub access_keys: Vec<IamAccessKey>,
    pub mfa_devices: Vec<String>,
    pub has_console_login: bool,
    pub groups: Vec<String>,
    pub managed_policies: Vec<(String, String)>, // (name, arn)
    pub inline_policy_names: Vec<String>,
    // From GetUser — ListUsers omits PermissionsBoundary and Tags.
    pub permissions_boundary: Option<String>,
    pub tags: HashMap<String, String>,
}

/// Resolve everything a permissions ticket needs about a user: access keys
/// (status/age/last-used), MFA devices, whether a console login exists, group
/// memberships, and attached managed + inline policies. Each call is
/// independently error-tolerant so one missing permission doesn't blank the rest.
pub async fn fetch_iam_user_details(
    client: IamClient,
    user_name: String,
) -> Result<IamUserDetails> {
    let (keys_r, mfa_r, login_r, groups_r, managed_r, inline_r, user_r) = tokio::join!(
        client.list_access_keys().user_name(&user_name).send(),
        client.list_mfa_devices().user_name(&user_name).send(),
        client.get_login_profile().user_name(&user_name).send(),
        client.list_groups_for_user().user_name(&user_name).send(),
        client.list_attached_user_policies().user_name(&user_name).send(),
        client.list_user_policies().user_name(&user_name).send(),
        client.get_user().user_name(&user_name).send(),
    );

    let mut access_keys: Vec<IamAccessKey> = match keys_r {
        Ok(r) => r
            .access_key_metadata()
            .iter()
            .map(|k| IamAccessKey {
                id: k.access_key_id().unwrap_or("").to_string(),
                status: k.status().map(|s| s.as_str().to_string()).unwrap_or_default(),
                created: fmt_datetime(k.create_date()),
                last_used: None,
            })
            .collect(),
        Err(_) => vec![],
    };
    // Resolve last-used per key (best-effort).
    for key in &mut access_keys {
        if key.id.is_empty() {
            continue;
        }
        if let Ok(resp) = client.get_access_key_last_used().access_key_id(&key.id).send().await {
            key.last_used =
                fmt_datetime(resp.access_key_last_used().and_then(|u| u.last_used_date()));
        }
    }

    let mfa_devices = match mfa_r {
        Ok(r) => r
            .mfa_devices()
            .iter()
            .map(|d| d.serial_number().to_string())
            .collect(),
        Err(_) => vec![],
    };

    // get_login_profile errors with NoSuchEntity when there's no console password.
    let has_console_login = login_r.is_ok();

    let groups = match groups_r {
        Ok(r) => r
            .groups()
            .iter()
            .map(|g| g.group_name().to_string())
            .collect(),
        Err(_) => vec![],
    };

    let managed_policies = match managed_r {
        Ok(r) => r
            .attached_policies()
            .iter()
            .map(|p| {
                (
                    p.policy_name().unwrap_or_default().to_string(),
                    p.policy_arn().unwrap_or_default().to_string(),
                )
            })
            .collect(),
        Err(_) => vec![],
    };

    let inline_policy_names = match inline_r {
        Ok(r) => r.policy_names().iter().map(|n| n.to_string()).collect(),
        Err(_) => vec![],
    };

    let (permissions_boundary, tags) = match user_r {
        Ok(r) => match r.user() {
            Some(u) => (
                u.permissions_boundary()
                    .and_then(|b| b.permissions_boundary_arn())
                    .map(|s| s.to_string()),
                tags_from_sdk(u.tags()),
            ),
            None => (None, HashMap::new()),
        },
        Err(_) => (None, HashMap::new()),
    };

    Ok(IamUserDetails {
        access_keys,
        mfa_devices,
        has_console_login,
        groups,
        managed_policies,
        inline_policy_names,
        permissions_boundary,
        tags,
    })
}

// ── Lazy-loaded IAM group details (members + policies) ──────────────────────

#[derive(Debug, Clone)]
pub struct IamGroupDetails {
    pub members: Vec<String>,
    pub managed_policies: Vec<(String, String)>,
    pub inline_policy_names: Vec<String>,
}

/// Resolve a group's member list (`get_group`) plus its attached managed +
/// inline policies.
pub async fn fetch_iam_group_details(
    client: IamClient,
    group_name: String,
) -> Result<IamGroupDetails> {
    let mut members = Vec::new();
    let mut paginator = client.get_group().group_name(&group_name).into_paginator().send();
    while let Some(Ok(page)) = paginator.next().await {
        for u in page.users() {
            members.push(u.user_name().to_string());
        }
    }

    let (managed_r, inline_r) = tokio::join!(
        client.list_attached_group_policies().group_name(&group_name).send(),
        client.list_group_policies().group_name(&group_name).send(),
    );

    let managed_policies = match managed_r {
        Ok(r) => r
            .attached_policies()
            .iter()
            .map(|p| {
                (
                    p.policy_name().unwrap_or_default().to_string(),
                    p.policy_arn().unwrap_or_default().to_string(),
                )
            })
            .collect(),
        Err(_) => vec![],
    };
    let inline_policy_names = match inline_r {
        Ok(r) => r.policy_names().iter().map(|n| n.to_string()).collect(),
        Err(_) => vec![],
    };

    Ok(IamGroupDetails {
        members,
        managed_policies,
        inline_policy_names,
    })
}

// ── Per-row policy document fetch (Permissions section) ────────────────────

/// Fetch a single managed policy's document body by ARN (used when a managed
/// policy row is selected — `get_policy` resolves the default version, then
/// `get_policy_version` fetches the document).
pub async fn fetch_managed_policy_document(client: IamClient, policy_arn: String) -> Result<String> {
    let policy_resp = client
        .get_policy()
        .policy_arn(&policy_arn)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;

    let version_id = policy_resp
        .policy()
        .and_then(|p| p.default_version_id())
        .unwrap_or("v1")
        .to_string();

    let version_resp = client
        .get_policy_version()
        .policy_arn(&policy_arn)
        .version_id(&version_id)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;

    Ok(version_resp
        .policy_version()
        .and_then(|v| v.document())
        .map(pretty_policy_document)
        .unwrap_or_else(|| "No policy document".to_string()))
}

/// Fetch a single inline policy's document body for a role.
pub async fn fetch_inline_policy_document(
    client: IamClient,
    role_name: String,
    policy_name: String,
) -> Result<String> {
    let resp = client
        .get_role_policy()
        .role_name(&role_name)
        .policy_name(&policy_name)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;

    Ok(pretty_policy_document(resp.policy_document()))
}

pub async fn fetch_user_inline_policy_document(
    client: IamClient,
    user_name: String,
    policy_name: String,
) -> Result<String> {
    let resp = client
        .get_user_policy()
        .user_name(&user_name)
        .policy_name(&policy_name)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;

    Ok(pretty_policy_document(resp.policy_document()))
}

pub async fn fetch_group_inline_policy_document(
    client: IamClient,
    group_name: String,
    policy_name: String,
) -> Result<String> {
    let resp = client
        .get_group_policy()
        .group_name(&group_name)
        .policy_name(&policy_name)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;

    Ok(pretty_policy_document(resp.policy_document()))
}

// ── Timestamp helpers ─────────────────────────────────────────────────────────

fn fmt_epoch_secs(secs: i64) -> String {
    let days = secs / 86400;
    let time_rem = secs % 86400;
    let h = time_rem / 3600;
    let m = (time_rem % 3600) / 60;
    let (y, mo, d) = epoch_days_to_ymd(days);
    format!("{:04}-{:02}-{:02} {:02}:{:02} UTC", y, mo, d, h, m)
}

fn epoch_days_to_ymd(mut days: i64) -> (i32, u8, u8) {
    let mut year = 1970i32;
    loop {
        let dy = if is_leap(year) { 366 } else { 365 };
        if days < dy {
            break;
        }
        days -= dy;
        year += 1;
    }
    let dm = [
        31u8,
        if is_leap(year) { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut month = 1u8;
    for &d in &dm {
        if days < d as i64 {
            break;
        }
        days -= d as i64;
        month += 1;
    }
    (year, month, (days + 1) as u8)
}

fn is_leap(y: i32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

// ── Identity providers (IamView::IdentityProviders) ─────────────────────────

/// A SAML or OIDC identity provider. The list APIs return little (OIDC lists
/// only ARNs), so the split pane enriches lazily via GetSAMLProvider /
/// GetOpenIDConnectProvider.
#[derive(Debug, Clone)]
pub struct IamIdentityProvider {
    pub arn: String,
    /// Everything after "…provider/" in the ARN — for OIDC this is the full
    /// issuer host (+ path, e.g. an EKS issuer's "…/id/HEX"), not a segment.
    pub provider_name: String,
    pub kind: &'static str, // "SAML" / "OIDC"
    pub create_date: Option<String>, // SAML list entry only; OIDC gets it lazily
    pub valid_until: Option<String>, // SAML only
    pub tags: HashMap<String, String>,
}

fn idp_name_from_arn(arn: &str) -> String {
    arn.split_once("provider/")
        .map(|(_, rest)| rest.to_string())
        .unwrap_or_else(|| arn.to_string())
}

impl IamIdentityProvider {
    pub fn from_saml(e: &aws_sdk_iam::types::SamlProviderListEntry) -> Self {
        let arn = e.arn().unwrap_or_default().to_string();
        Self {
            provider_name: idp_name_from_arn(&arn),
            arn,
            kind: "SAML",
            create_date: fmt_datetime(e.create_date()),
            valid_until: fmt_datetime(e.valid_until()),
            tags: HashMap::new(),
        }
    }

    pub fn from_oidc(e: &aws_sdk_iam::types::OpenIdConnectProviderListEntry) -> Self {
        let arn = e.arn().unwrap_or_default().to_string();
        Self {
            provider_name: idp_name_from_arn(&arn),
            arn,
            kind: "OIDC",
            create_date: None,
            valid_until: None,
            tags: HashMap::new(),
        }
    }
}

crate::sections! {
    pub enum IamIdpDetailSection,
    pub static IAM_IDP_SECTIONS = [
        Overview "Overview" => crate::app::App::trigger_iam_idp_details_load,
        Tags "Tags" => crate::app::App::trigger_iam_idp_details_load,
    ]
}

impl Resource for IamIdentityProvider {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&IAM_IDP_SECTIONS)
    }

    fn cli_command(&self) -> Option<String> {
        let quoted = crate::aws::resource::shell_quote(&self.arn);
        Some(if self.kind == "SAML" {
            format!("aws iam get-saml-provider --saml-provider-arn {}", quoted)
        } else {
            format!(
                "aws iam get-open-id-connect-provider --open-id-connect-provider-arn {}",
                quoted
            )
        })
    }

    fn id(&self) -> &str {
        &self.arn
    }

    fn name(&self) -> &str {
        &self.provider_name
    }

    fn resource_type(&self) -> &str {
        "Identity Provider"
    }

    fn state(&self) -> ResourceState {
        ResourceState::stateless()
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!("{} {} {}", self.provider_name, self.arn, self.kind)
    }

    fn details(&self) -> Vec<(String, String)> {
        let mut rows = vec![
            ("Provider".to_string(), self.provider_name.clone()),
            ("Type".to_string(), self.kind.to_string()),
            ("ARN".to_string(), self.arn.clone()),
        ];
        if let Some(c) = &self.create_date {
            rows.push(("Created".to_string(), c.clone()));
        }
        if let Some(v) = &self.valid_until {
            rows.push(("Valid Until".to_string(), v.clone()));
        }
        rows
    }

    fn console_url(&self, _region: &str) -> Option<String> {
        Some(
            "https://us-east-1.console.aws.amazon.com/iam/home#/identity_providers".to_string(),
        )
    }

    fn trail_lookup_keys(&self) -> Vec<String> {
        vec![self.provider_name.clone(), self.arn.clone()]
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── Lazy-loaded identity-provider details ───────────────────────────────────

#[derive(Debug, Clone)]
pub struct IamIdpDetails {
    pub create_date: Option<String>,
    pub valid_until: Option<String>,   // SAML
    pub metadata_len: Option<usize>,   // SAML — document size, never the body
    pub url: Option<String>,           // OIDC
    pub client_ids: Vec<String>,       // OIDC
    pub thumbprints: Vec<String>,      // OIDC
    pub tags: HashMap<String, String>,
}

pub async fn fetch_iam_idp_details(
    client: IamClient,
    arn: String,
    kind: &'static str,
) -> Result<IamIdpDetails> {
    if kind == "SAML" {
        let r = client
            .get_saml_provider()
            .saml_provider_arn(&arn)
            .send()
            .await
            .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
        Ok(IamIdpDetails {
            create_date: fmt_datetime(r.create_date()),
            valid_until: fmt_datetime(r.valid_until()),
            metadata_len: r.saml_metadata_document().map(|d| d.len()),
            url: None,
            client_ids: vec![],
            thumbprints: vec![],
            tags: tags_from_sdk(r.tags()),
        })
    } else {
        let r = client
            .get_open_id_connect_provider()
            .open_id_connect_provider_arn(&arn)
            .send()
            .await
            .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
        Ok(IamIdpDetails {
            create_date: fmt_datetime(r.create_date()),
            valid_until: None,
            metadata_len: None,
            url: r.url().map(|s| s.to_string()),
            client_ids: r.client_id_list().to_vec(),
            thumbprints: r.thumbprint_list().to_vec(),
            tags: tags_from_sdk(r.tags()),
        })
    }
}

// ── Account settings (IamView::Account) ─────────────────────────────────────

#[derive(Debug, Clone)]
pub struct IamPasswordPolicy {
    pub minimum_length: Option<i32>,
    pub require_symbols: bool,
    pub require_numbers: bool,
    pub require_uppercase: bool,
    pub require_lowercase: bool,
    pub allow_users_to_change: bool,
    pub expire_passwords: bool,
    pub max_age: Option<i32>,
    pub reuse_prevention: Option<i32>,
    pub hard_expiry: bool,
}

/// One synthetic row: account alias, GetAccountSummary counts/quotas, and the
/// password policy. Fetched eagerly at load time (three cheap calls); call
/// failures render inline in the pane's sections.
#[derive(Debug, Clone)]
pub struct IamAccountSettings {
    pub display_name: String,
    pub aliases: Vec<String>,
    pub summary: Vec<(String, i32)>, // sorted by key
    pub summary_error: Option<String>,
    /// None with no error = the account uses the AWS default policy.
    pub password_policy: Option<IamPasswordPolicy>,
    pub pw_policy_error: Option<String>,
    pub tags: HashMap<String, String>,
}

impl IamAccountSettings {
    pub fn summary_value(&self, key: &str) -> Option<i32> {
        self.summary.iter().find(|(k, _)| k == key).map(|(_, v)| *v)
    }
}

pub async fn fetch_account_settings(client: &IamClient) -> IamAccountSettings {
    let (summary_r, pw_r, alias_r) = tokio::join!(
        client.get_account_summary().send(),
        client.get_account_password_policy().send(),
        client.list_account_aliases().send(),
    );

    let (summary, summary_error) = match summary_r {
        Ok(r) => {
            let mut v: Vec<(String, i32)> = r
                .summary_map()
                .map(|m| m.iter().map(|(k, n)| (k.as_str().to_string(), *n)).collect())
                .unwrap_or_default();
            v.sort_by(|a, b| a.0.cmp(&b.0));
            (v, None)
        }
        Err(e) => (vec![], Some(crate::error::sdk_error_message(&e))),
    };

    let (password_policy, pw_policy_error) = match pw_r {
        Ok(r) => (
            r.password_policy().map(|p| IamPasswordPolicy {
                minimum_length: p.minimum_password_length(),
                require_symbols: p.require_symbols(),
                require_numbers: p.require_numbers(),
                require_uppercase: p.require_uppercase_characters(),
                require_lowercase: p.require_lowercase_characters(),
                allow_users_to_change: p.allow_users_to_change_password(),
                expire_passwords: p.expire_passwords(),
                max_age: p.max_password_age(),
                reuse_prevention: p.password_reuse_prevention(),
                hard_expiry: p.hard_expiry().unwrap_or(false),
            }),
            None,
        ),
        Err(e) => {
            // No custom policy = the AWS default, not an error.
            let is_default = e
                .as_service_error()
                .map(|se| se.is_no_such_entity_exception())
                .unwrap_or(false);
            if is_default {
                (None, None)
            } else {
                (None, Some(crate::error::sdk_error_message(&e)))
            }
        }
    };

    let aliases = match alias_r {
        Ok(r) => r.account_aliases().to_vec(),
        Err(_) => vec![],
    };

    IamAccountSettings {
        display_name: aliases
            .first()
            .cloned()
            .unwrap_or_else(|| "Account settings".to_string()),
        aliases,
        summary,
        summary_error,
        password_policy,
        pw_policy_error,
        tags: HashMap::new(),
    }
}

crate::sections! {
    pub enum IamAccountDetailSection,
    pub static IAM_ACCOUNT_SECTIONS = [
        Overview "Overview",
        PasswordPolicy "Password Policy",
        Usage "Usage",
    ]
}

impl Resource for IamAccountSettings {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&IAM_ACCOUNT_SECTIONS)
    }

    fn cli_command(&self) -> Option<String> {
        Some("aws iam get-account-summary".to_string())
    }

    fn id(&self) -> &str {
        "iam-account-settings"
    }

    fn name(&self) -> &str {
        &self.display_name
    }

    fn resource_type(&self) -> &str {
        "IAM Account Settings"
    }

    fn state(&self) -> ResourceState {
        // Root credentials are the account's loudest signal: access keys on
        // the root user render red, root without MFA renders yellow.
        if self.summary_value("AccountAccessKeysPresent").unwrap_or(0) > 0 {
            return ResourceState::Unavailable;
        }
        if self.summary_value("AccountMFAEnabled") == Some(0) {
            return ResourceState::Unknown("root MFA off".to_string());
        }
        ResourceState::Available
    }

    fn state_label(&self) -> String {
        if self.summary_value("AccountAccessKeysPresent").unwrap_or(0) > 0 {
            "root access keys".to_string()
        } else if self.summary_value("AccountMFAEnabled") == Some(0) {
            "root mfa off".to_string()
        } else {
            "ok".to_string()
        }
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!(
            "account settings password policy {}",
            self.aliases.join(" ")
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        let mut rows = vec![(
            "Alias".to_string(),
            if self.aliases.is_empty() {
                "—".to_string()
            } else {
                self.aliases.join(", ")
            },
        )];
        for key in ["Users", "Roles", "Policies", "Groups"] {
            if let Some(v) = self.summary_value(key) {
                rows.push((key.to_string(), v.to_string()));
            }
        }
        rows
    }

    fn console_url(&self, _region: &str) -> Option<String> {
        Some("https://us-east-1.console.aws.amazon.com/iam/home#/account_settings".to_string())
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── Access Analyzer finding (IamView::AccessAnalyzer) ─────────────────────────

#[derive(Debug, Clone)]
pub struct AccessAnalyzerFinding {
    pub id: String,
    pub analyzer_arn: String,
    pub finding_type: String, // "Public access" / "External access"
    pub resource: String,
    pub resource_type: String,
    pub status: String,
    pub is_public: bool,
    pub external_principal: String,
    pub actions: Vec<String>,
    pub condition: Vec<(String, String)>,
    pub analyzed_at: Option<String>,
    pub updated_at: Option<String>,
    pub tags: HashMap<String, String>,
}

impl AccessAnalyzerFinding {
    pub fn from_sdk(f: &aws_sdk_accessanalyzer::types::FindingSummary, analyzer_arn: &str) -> Self {
        let is_public = f.is_public().unwrap_or(false);
        let external_principal = f
            .principal()
            .map(|p| {
                p.iter()
                    .map(|(k, v)| format!("{}={}", k, v))
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        let condition = f
            .condition()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        Self {
            id: f.id().to_string(),
            analyzer_arn: analyzer_arn.to_string(),
            finding_type: if is_public {
                "Public access".to_string()
            } else {
                "External access".to_string()
            },
            resource: f.resource().unwrap_or("").to_string(),
            resource_type: f.resource_type().as_str().to_string(),
            status: f.status().as_str().to_string(),
            is_public,
            external_principal,
            actions: f.action().to_vec(),
            condition,
            analyzed_at: Some(fmt_epoch_secs(f.analyzed_at().secs())),
            updated_at: Some(fmt_epoch_secs(f.updated_at().secs())),
            tags: HashMap::new(),
        }
    }
}

crate::sections! {
    pub enum AccessAnalyzerDetailSection,
    pub static ACCESS_ANALYZER_SECTIONS = [
        Details "Details",
        Access "Access",
    ]
}

impl Resource for AccessAnalyzerFinding {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&ACCESS_ANALYZER_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        // The resource the finding is about reads better in the list than a UUID.
        if self.resource.is_empty() {
            &self.id
        } else {
            &self.resource
        }
    }

    fn resource_type(&self) -> &str {
        "Access Analyzer Finding"
    }

    fn is_noise(&self) -> bool {
        self.status != "ACTIVE"
    }

    fn state(&self) -> ResourceState {
        match self.status.as_str() {
            "ACTIVE" if self.is_public => ResourceState::Unavailable, // public = loudest (red)
            "ACTIVE" => ResourceState::Unknown("ACTIVE".to_string()), // warning (yellow)
            _ => ResourceState::Available,                            // archived/resolved
        }
    }

    fn state_label(&self) -> String {
        // Public exposure is the red signal; otherwise the finding's own status.
        if self.status == "ACTIVE" && self.is_public {
            "public".to_string()
        } else {
            native_state_label(&self.status, || self.state())
        }
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!(
            "{} {} {} {} {} {}",
            self.resource,
            self.resource_type,
            self.external_principal,
            self.actions.join(" "),
            self.status,
            if self.is_public { "public" } else { "external" }
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Resource".to_string(), self.resource.clone()),
            ("Type".to_string(), self.resource_type.clone()),
            ("Status".to_string(), self.status.clone()),
            (
                "Public".to_string(),
                if self.is_public { "yes" } else { "no" }.to_string(),
            ),
        ]
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/access-analyzer/home?region={}#/findings",
            region, region
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
