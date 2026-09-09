use crate::aws::client::AwsClients;
use crate::aws::resource::{native_state_label, Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use aws_sdk_organizations::types::PolicyType;
use aws_sdk_organizations::Client as OrgClient;
use std::any::Any;
use std::collections::HashMap;
use tokio::sync::mpsc;

pub struct OrganizationsService {
    client: OrgClient,
}

impl OrganizationsService {
    pub fn new(aws_clients: &AwsClients) -> Self {
        Self {
            client: aws_clients.organizations_client(),
        }
    }
}

/// Classifies a typed Organizations SDK error into a friendly message when the
/// account isn't in an organization or lacks permissions; falls back to the
/// raw error string otherwise.
trait OrgErrorClassify {
    fn is_not_in_use(&self) -> bool;
    fn is_denied(&self) -> bool;
}

macro_rules! impl_org_error_classify {
    ($ty:ty) => {
        impl OrgErrorClassify for $ty {
            fn is_not_in_use(&self) -> bool {
                self.is_aws_organizations_not_in_use_exception()
            }
            fn is_denied(&self) -> bool {
                self.is_access_denied_exception()
            }
        }
    };
}

impl_org_error_classify!(aws_sdk_organizations::operation::list_accounts::ListAccountsError);
impl_org_error_classify!(aws_sdk_organizations::operation::list_roots::ListRootsError);
impl_org_error_classify!(
    aws_sdk_organizations::operation::list_organizational_units_for_parent::ListOrganizationalUnitsForParentError
);
impl_org_error_classify!(aws_sdk_organizations::operation::list_policies::ListPoliciesError);
impl_org_error_classify!(
    aws_sdk_organizations::operation::list_policies_for_target::ListPoliciesForTargetError
);
impl_org_error_classify!(
    aws_sdk_organizations::operation::list_targets_for_policy::ListTargetsForPolicyError
);
impl_org_error_classify!(aws_sdk_organizations::operation::list_parents::ListParentsError);
impl_org_error_classify!(
    aws_sdk_organizations::operation::describe_organizational_unit::DescribeOrganizationalUnitError
);
impl_org_error_classify!(aws_sdk_organizations::operation::describe_policy::DescribePolicyError);
impl_org_error_classify!(
    aws_sdk_organizations::operation::describe_organization::DescribeOrganizationError
);

fn friendly_error<E, R>(action: &str, err: aws_sdk_organizations::error::SdkError<E, R>) -> String
where
    E: OrgErrorClassify
        + std::fmt::Debug
        + std::error::Error
        + Send
        + Sync
        + aws_smithy_runtime_api::client::result::CreateUnhandledError
        + 'static,
    R: std::fmt::Debug + Send + Sync + 'static,
{
    let service_err = err.into_service_error();
    if service_err.is_not_in_use() {
        "Account is not part of an AWS Organization".to_string()
    } else if service_err.is_denied() {
        "Access denied — missing organizations:List*/Describe* permissions".to_string()
    } else {
        format!("Failed to {}: {:?}", action, service_err)
    }
}

#[async_trait]
impl AwsService for OrganizationsService {
    fn service_type(&self) -> ServiceType {
        ServiceType::Organizations
    }

    fn name(&self) -> &str {
        "Organizations"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        self.list_resources_streaming(tx, ServiceType::Organizations)
            .await?;
        Ok(vec![])
    }

    async fn list_resources_streaming(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        let mut total = 0usize;

        // Phase 0: Organization overview + roots (roots reused for OUs + policies)
        let org = match self.client.describe_organization().send().await {
            Ok(r) => r.organization().cloned(),
            Err(e) => {
                let svc = e.into_service_error();
                if svc.is_not_in_use() {
                    // Genuinely not in an org — the whole service is unusable.
                    let _ = event_tx.send(Event::ResourceLoadError {
                        service: service_type,
                        error: "Account is not part of an AWS Organization".to_string(),
                    });
                    return Ok(());
                }
                // Other errors (e.g. lacking DescribeOrganization specifically) —
                // skip the Overview but still try accounts / policies / etc.
                None
            }
        };

        let mut roots = Vec::new();
        let mut paginator = self.client.list_roots().into_paginator().send();
        while let Some(result) = paginator.next().await {
            match result {
                Ok(page) => roots.extend(page.roots().to_vec()),
                Err(e) => {
                    let _ = event_tx.send(Event::ResourceLoadError {
                        service: service_type,
                        error: friendly_error("list organization roots", e),
                    });
                    return Ok(());
                }
            }
        }

        // Policy types enabled at the root(s).
        let mut enabled_types: Vec<PolicyType> = Vec::new();
        for root in &roots {
            for pts in root.policy_types() {
                if matches!(
                    pts.status(),
                    Some(aws_sdk_organizations::types::PolicyTypeStatus::Enabled)
                ) {
                    if let Some(t) = pts.r#type() {
                        if !enabled_types.contains(t) {
                            enabled_types.push(t.clone());
                        }
                    }
                }
            }
        }

        if let Some(org) = &org {
            let info = OrgInfo {
                org_id: org.id().unwrap_or_default().to_string(),
                arn: org.arn().unwrap_or_default().to_string(),
                feature_set: org
                    .feature_set()
                    .map(|f| f.as_str().to_string())
                    .unwrap_or_default(),
                master_account_id: org.master_account_id().unwrap_or_default().to_string(),
                master_account_email: org.master_account_email().unwrap_or_default().to_string(),
                enabled_policy_types: enabled_types
                    .iter()
                    .map(|t| policy_type_label(Some(t)).to_string())
                    .collect(),
                tags: HashMap::new(),
            };
            total += 1;
            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                service: service_type,
                resources: vec![Box::new(info) as Box<dyn Resource>],
                progress: LoadProgress {
                    loaded_count: total,
                    total_count: None,
                    status_message: Some("Loading organization...".to_string()),
                },
            });
        }

        // Phase 1: Accounts
        let mut paginator = self.client.list_accounts().into_paginator().send();
        while let Some(result) = paginator.next().await {
            match result {
                Ok(page) => {
                    let batch: Vec<Box<dyn Resource>> = page
                        .accounts()
                        .iter()
                        .map(|a| Box::new(OrgAccount::from_sdk(a)) as Box<dyn Resource>)
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
                            status_message: Some("Loading accounts...".to_string()),
                        },
                    });
                }
                Err(e) => {
                    let _ = event_tx.send(Event::ResourceLoadError {
                        service: service_type,
                        error: friendly_error("list accounts", e),
                    });
                    return Ok(());
                }
            }
        }

        // Phase 2: Organizational Units (walk the tree from each root,
        // reusing the roots fetched in phase 0)
        let mut ou_batch: Vec<Box<dyn Resource>> = Vec::new();
        for root in &roots {
            let root_id = root.id().unwrap_or_default().to_string();
            let root_name = root.name().unwrap_or("Root").to_string();
            if let Err(e) = self.walk_ous(&root_id, &root_name, &mut ou_batch).await {
                let _ = event_tx.send(Event::ResourceLoadError {
                    service: service_type,
                    error: e,
                });
                return Ok(());
            }
        }
        let count = ou_batch.len();
        if count > 0 {
            total += count;
            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                service: service_type,
                resources: ou_batch,
                progress: LoadProgress {
                    loaded_count: total,
                    total_count: None,
                    status_message: Some("Loading organizational units...".to_string()),
                },
            });
        }

        // Phase 3: Policies — all enabled types (SCP, Tag, Backup, AI opt-out,
        // RCP, Chatbot, …). Per-type errors are tolerated (skip that type).
        let types_to_list = if enabled_types.is_empty() {
            vec![PolicyType::ServiceControlPolicy]
        } else {
            enabled_types.clone()
        };
        let mut policy_batch: Vec<Box<dyn Resource>> = Vec::new();
        for t in types_to_list {
            let mut paginator = self
                .client
                .list_policies()
                .filter(t.clone())
                .into_paginator()
                .send();
            while let Some(result) = paginator.next().await {
                match result {
                    Ok(page) => {
                        for p in page.policies() {
                            policy_batch.push(Box::new(OrgScp::from_sdk(p)) as Box<dyn Resource>);
                        }
                    }
                    Err(_) => break, // this type isn't listable here — skip it
                }
            }
        }
        if !policy_batch.is_empty() {
            total += policy_batch.len();
            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                service: service_type,
                resources: policy_batch,
                progress: LoadProgress {
                    loaded_count: total,
                    total_count: None,
                    status_message: Some("Loading policies...".to_string()),
                },
            });
        }

        // Phase 4: Delegated administrators (+ the services they administer)
        let mut deleg_batch: Vec<Box<dyn Resource>> = Vec::new();
        let mut paginator = self
            .client
            .list_delegated_administrators()
            .into_paginator()
            .send();
        while let Some(result) = paginator.next().await {
            match result {
                Ok(page) => {
                    for a in page.delegated_administrators() {
                        let account_id = a.id().unwrap_or_default().to_string();
                        let mut services = Vec::new();
                        let mut sp = self
                            .client
                            .list_delegated_services_for_account()
                            .account_id(&account_id)
                            .into_paginator()
                            .send();
                        while let Some(sr) = sp.next().await {
                            if let Ok(spage) = sr {
                                for s in spage.delegated_services() {
                                    if let Some(princ) = s.service_principal() {
                                        services.push(princ.to_string());
                                    }
                                }
                            }
                        }
                        deleg_batch.push(Box::new(OrgDelegatedAdmin {
                            account_id: account_id.clone(),
                            account_name: a.name().unwrap_or_default().to_string(),
                            email: a.email().unwrap_or_default().to_string(),
                            status: a
                                .status()
                                .map(|s| s.as_str().to_string())
                                .unwrap_or_default(),
                            services,
                            tags: HashMap::new(),
                        }) as Box<dyn Resource>);
                    }
                }
                Err(_) => break, // delegated-admin listing not available — skip
            }
        }
        if !deleg_batch.is_empty() {
            total += deleg_batch.len();
            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                service: service_type,
                resources: deleg_batch,
                progress: LoadProgress {
                    loaded_count: total,
                    total_count: None,
                    status_message: Some("Loading delegated admins...".to_string()),
                },
            });
        }

        // Phase 5: Trusted services (AWS service access enabled for the org)
        let mut trusted_batch: Vec<Box<dyn Resource>> = Vec::new();
        let mut paginator = self
            .client
            .list_aws_service_access_for_organization()
            .into_paginator()
            .send();
        while let Some(result) = paginator.next().await {
            match result {
                Ok(page) => {
                    for s in page.enabled_service_principals() {
                        trusted_batch.push(Box::new(OrgTrustedService {
                            service_principal: s
                                .service_principal()
                                .unwrap_or_default()
                                .to_string(),
                            date_enabled: s
                                .date_enabled()
                                .map(|d| crate::aws::services::cloudwatch::fmt_epoch_secs(d.secs())),
                            tags: HashMap::new(),
                        }) as Box<dyn Resource>);
                    }
                }
                Err(_) => break, // trusted-access listing not available — skip
            }
        }
        if !trusted_batch.is_empty() {
            total += trusted_batch.len();
            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                service: service_type,
                resources: trusted_batch,
                progress: LoadProgress {
                    loaded_count: total,
                    total_count: None,
                    status_message: Some("Loading trusted services...".to_string()),
                },
            });
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

impl OrganizationsService {
    /// Walks the OU tree under `root_id` breadth-first (using an explicit
    /// work queue rather than recursion, since async fns can't recurse
    /// without boxing every call), appending each discovered OU with its
    /// slash-joined ancestor path into `out`.
    async fn walk_ous(
        &self,
        root_id: &str,
        root_path: &str,
        out: &mut Vec<Box<dyn Resource>>,
    ) -> std::result::Result<(), String> {
        let mut queue: std::collections::VecDeque<(String, String)> =
            std::collections::VecDeque::new();
        queue.push_back((root_id.to_string(), root_path.to_string()));

        while let Some((parent_id, parent_path)) = queue.pop_front() {
            let mut paginator = self
                .client
                .list_organizational_units_for_parent()
                .parent_id(&parent_id)
                .into_paginator()
                .send();
            while let Some(result) = paginator.next().await {
                let page = result.map_err(|e| friendly_error("list organizational units", e))?;
                for ou in page.organizational_units() {
                    let ou_id = ou.id().unwrap_or_default().to_string();
                    let ou_name = ou.name().unwrap_or_default().to_string();
                    out.push(Box::new(OrgUnit::new(
                        ou_id.clone(),
                        ou.arn().unwrap_or_default().to_string(),
                        ou_name.clone(),
                        parent_path.clone(),
                        parent_id.clone(),
                    )));
                    let child_path = format!("{}/{}", parent_path, ou_name);
                    queue.push_back((ou_id, child_path));
                }
            }
        }
        Ok(())
    }
}

// ── OrgAccount ───────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct OrgAccount {
    pub account_id: String,
    pub arn: String,
    pub account_name: String,
    pub email: String,
    pub status: String,
    pub joined_method: Option<String>,
    pub joined_timestamp: Option<String>,
    pub tags: HashMap<String, String>,
}

impl OrgAccount {
    pub fn from_sdk(a: &aws_sdk_organizations::types::Account) -> Self {
        Self {
            account_id: a.id().unwrap_or_default().to_string(),
            arn: a.arn().unwrap_or_default().to_string(),
            account_name: a.name().unwrap_or_default().to_string(),
            email: a.email().unwrap_or_default().to_string(),
            status: a
                .status()
                .map(|s| s.as_str().to_string())
                .unwrap_or_else(|| "—".to_string()),
            joined_method: a.joined_method().map(|m| m.as_str().to_string()),
            joined_timestamp: a.joined_timestamp().map(|d| fmt_epoch_secs(d.secs())),
            tags: HashMap::new(),
        }
    }
}

crate::sections! {
    pub enum OrgAccountDetailSection,
    pub static ORG_ACCOUNT_SECTIONS = [
        Details "Details",
        OuPath "OU Path" => crate::app::App::trigger_org_account_details_load,
        Policies "Policies" => crate::app::App::trigger_org_account_details_load,
    ]
}

impl Resource for OrgAccount {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&ORG_ACCOUNT_SECTIONS)
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws organizations describe-account --account-id {}",
            crate::aws::resource::shell_quote(self.id())
        ))
    }

    fn id(&self) -> &str {
        &self.account_id
    }

    fn name(&self) -> &str {
        &self.account_name
    }

    fn resource_type(&self) -> &str {
        "Organizations Account"
    }

    fn state(&self) -> ResourceState {
        if self.status == "ACTIVE" {
            ResourceState::Available
        } else {
            ResourceState::Unknown(self.status.clone())
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
            self.account_name, self.account_id, self.email, self.arn
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Account Name".to_string(), self.account_name.clone()),
            ("Account ID".to_string(), self.account_id.clone()),
            ("Email".to_string(), self.email.clone()),
            ("Status".to_string(), self.status.clone()),
            (
                "Joined".to_string(),
                self.joined_timestamp.clone().unwrap_or_else(|| "—".to_string()),
            ),
        ]
    }

    fn console_url(&self, _region: &str) -> Option<String> {
        Some(format!(
            "https://us-east-1.console.aws.amazon.com/organizations/v2/home/accounts/{}",
            self.account_id
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── OrgUnit ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct OrgUnit {
    pub ou_id: String,
    pub arn: String,
    pub ou_name: String,
    /// Slash-joined path of the *ancestors* ("Root/Workloads"), not including
    /// this OU's own name — `full_path` appends that.
    pub parent_path: String,
    /// The direct parent's id: another `ou-…`, or the `r-…` root for a
    /// top-level OU. Rendered as a jumpable Overview row.
    pub parent_id: String,
    /// `parent_path` with this OU's own name appended — the OU's position in
    /// the tree, and what `name()` returns. Precomputed because `name()`
    /// hands back a `&str`.
    pub full_path: String,
    pub tags: HashMap<String, String>,
}

impl OrgUnit {
    pub fn new(
        ou_id: String,
        arn: String,
        ou_name: String,
        parent_path: String,
        parent_id: String,
    ) -> Self {
        let full_path = if parent_path.is_empty() {
            ou_name.clone()
        } else {
            format!("{}/{}", parent_path, ou_name)
        };
        Self {
            ou_id,
            arn,
            ou_name,
            parent_path,
            parent_id,
            full_path,
            tags: HashMap::new(),
        }
    }
}

crate::sections! {
    pub enum OrgUnitDetailSection,
    pub static ORG_OU_SECTIONS = [
        Overview "Overview" => crate::app::App::trigger_org_ou_details_load,
        Children "Children" => crate::app::App::trigger_org_ou_details_load,
        Policies "Policies" => crate::app::App::trigger_org_ou_details_load,
        Tags "Tags" => crate::app::App::trigger_org_ou_details_load,
    ]
}

impl Resource for OrgUnit {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&ORG_OU_SECTIONS)
    }

    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws organizations describe-organizational-unit --organizational-unit-id {}",
            crate::aws::resource::shell_quote(self.id())
        ))
    }

    fn id(&self) -> &str {
        &self.ou_id
    }

    /// The **full path**, not the bare OU name: an OU's position in the tree
    /// is most of what identifies it (sibling names are unique, but names
    /// repeat across branches), and name-ascending sort then lays the list
    /// out as the tree. The detail pane's header still shows `ou_name`.
    fn name(&self) -> &str {
        &self.full_path
    }

    fn resource_type(&self) -> &str {
        "Organizations OU"
    }

    fn state(&self) -> ResourceState {
        ResourceState::stateless()
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!("{} {} {}", self.ou_name, self.ou_id, self.full_path)
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("OU Name".to_string(), self.ou_name.clone()),
            ("OU ID".to_string(), self.ou_id.clone()),
            ("Path".to_string(), self.full_path.clone()),
            ("ARN".to_string(), self.arn.clone()),
        ]
    }

    fn console_url(&self, _region: &str) -> Option<String> {
        Some(format!(
            "https://us-east-1.console.aws.amazon.com/organizations/v2/home/ous/{}",
            self.ou_id
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── OrgScp ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct OrgScp {
    pub policy_id: String,
    pub arn: String,
    pub policy_name: String,
    /// Short policy-type label: SCP / Tag / Backup / AI Opt-out / RCP / Chatbot / Declarative EC2.
    pub policy_type: String,
    pub description: Option<String>,
    pub aws_managed: bool,
    pub tags: HashMap<String, String>,
}

/// Short human label for a policy type (the SDK enum is non-exhaustive).
pub fn policy_type_label(t: Option<&PolicyType>) -> &'static str {
    match t {
        Some(PolicyType::ServiceControlPolicy) => "SCP",
        Some(PolicyType::TagPolicy) => "Tag",
        Some(PolicyType::BackupPolicy) => "Backup",
        Some(PolicyType::AiservicesOptOutPolicy) => "AI Opt-out",
        Some(PolicyType::ResourceControlPolicy) => "RCP",
        Some(PolicyType::ChatbotPolicy) => "Chatbot",
        Some(PolicyType::DeclarativePolicyEc2) => "Declarative EC2",
        _ => "Policy",
    }
}

impl OrgScp {
    pub fn from_sdk(p: &aws_sdk_organizations::types::PolicySummary) -> Self {
        Self {
            policy_id: p.id().unwrap_or_default().to_string(),
            arn: p.arn().unwrap_or_default().to_string(),
            policy_name: p.name().unwrap_or_default().to_string(),
            policy_type: policy_type_label(p.r#type()).to_string(),
            description: p.description().map(|s| s.to_string()),
            aws_managed: p.aws_managed(),
            tags: HashMap::new(),
        }
    }
}

crate::sections! {
    pub enum OrgScpDetailSection,
    pub static ORG_SCP_SECTIONS = [
        Document "Document" => crate::app::App::trigger_org_scp_details_load,
        AttachedTargets "Attached Targets" => crate::app::App::trigger_org_scp_details_load,
    ]
}

impl Resource for OrgScp {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&ORG_SCP_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.policy_id
    }

    fn name(&self) -> &str {
        &self.policy_name
    }

    fn resource_type(&self) -> &str {
        "Organizations Policy"
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
            self.policy_name, self.policy_type, self.policy_id, self.arn
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Policy Name".to_string(), self.policy_name.clone()),
            ("Type".to_string(), self.policy_type.clone()),
            ("Policy ID".to_string(), self.policy_id.clone()),
            (
                "AWS Managed".to_string(),
                if self.aws_managed { "Yes" } else { "No" }.to_string(),
            ),
            (
                "Description".to_string(),
                self.description.clone().unwrap_or_else(|| "—".to_string()),
            ),
        ]
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── OrgInfo (Overview) ─────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct OrgInfo {
    pub org_id: String,
    pub arn: String,
    pub feature_set: String,
    pub master_account_id: String,
    pub master_account_email: String,
    pub enabled_policy_types: Vec<String>,
    pub tags: HashMap<String, String>,
}

impl Resource for OrgInfo {
    fn id(&self) -> &str {
        &self.org_id
    }
    fn name(&self) -> &str {
        &self.org_id
    }
    fn resource_type(&self) -> &str {
        "Organization"
    }
    fn state(&self) -> ResourceState {
        ResourceState::stateless()
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        format!("organization {} {}", self.org_id, self.master_account_id)
    }
    fn details(&self) -> Vec<(String, String)> {
        let types = if self.enabled_policy_types.is_empty() {
            "None (consolidated billing — SCPs/policies don't apply)".to_string()
        } else {
            self.enabled_policy_types.join(", ")
        };
        vec![
            ("Organization ID".to_string(), self.org_id.clone()),
            ("ARN".to_string(), self.arn.clone()),
            ("Feature set".to_string(), self.feature_set.clone()),
            ("Management account".to_string(), self.master_account_id.clone()),
            ("Management email".to_string(), self.master_account_email.clone()),
            ("Enabled policy types".to_string(), types),
        ]
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── OrgDelegatedAdmin (Delegated) ──────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct OrgDelegatedAdmin {
    pub account_id: String,
    pub account_name: String,
    pub email: String,
    pub status: String,
    pub services: Vec<String>, // service principals this account is delegated admin for
    pub tags: HashMap<String, String>,
}

impl Resource for OrgDelegatedAdmin {
    fn id(&self) -> &str {
        &self.account_id
    }
    fn name(&self) -> &str {
        &self.account_name
    }
    fn resource_type(&self) -> &str {
        "Organizations Delegated Admin"
    }
    fn state(&self) -> ResourceState {
        if self.status == "ACTIVE" {
            ResourceState::Available
        } else {
            ResourceState::Unknown(self.status.clone())
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
            self.account_name,
            self.account_id,
            self.email,
            self.services.join(" ")
        )
    }
    fn details(&self) -> Vec<(String, String)> {
        let mut rows = vec![
            ("Account".to_string(), self.account_name.clone()),
            ("Account ID".to_string(), self.account_id.clone()),
            ("Email".to_string(), self.email.clone()),
            ("Status".to_string(), self.status.clone()),
            (
                "Delegated services".to_string(),
                self.services.len().to_string(),
            ),
        ];
        for s in &self.services {
            rows.push(("  service".to_string(), s.clone()));
        }
        rows
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── OrgTrustedService (Trusted Services / enabled service access) ──────────

#[derive(Debug, Clone)]
pub struct OrgTrustedService {
    pub service_principal: String,
    pub date_enabled: Option<String>,
    pub tags: HashMap<String, String>,
}

impl Resource for OrgTrustedService {
    fn id(&self) -> &str {
        &self.service_principal
    }
    fn name(&self) -> &str {
        &self.service_principal
    }
    fn resource_type(&self) -> &str {
        "Organizations Trusted Service"
    }
    fn state(&self) -> ResourceState {
        ResourceState::stateless()
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        self.service_principal.clone()
    }
    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Service principal".to_string(), self.service_principal.clone()),
            (
                "Date enabled".to_string(),
                self.date_enabled.clone().unwrap_or_else(|| "—".to_string()),
            ),
        ]
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── Effective-policy resolution (shared by the account + OU panes) ────────

/// A policy that applies to some target, and *where* it's attached. SCPs and
/// RCPs are inherited down the tree, so the attachment point is usually an
/// ancestor OU or the root — listing only direct attachments (which is all
/// `ListPoliciesForTarget` returns for one target) hides the policies that
/// actually govern an account.
#[derive(Debug, Clone)]
pub struct OrgAttachedPolicy {
    /// Short type label from `policy_type_label` (SCP / Tag / Backup / …).
    pub type_label: String,
    pub name: String,
    pub policy_id: String,
    /// The ancestor it's attached to as `(name, id)`; `None` = attached
    /// directly to the resource being viewed.
    pub source: Option<(String, String)>,
}

/// Every policy type Organizations knows about, used when the enabled-type
/// probe (`ListRoots`) itself fails and we have to guess.
const ALL_POLICY_TYPES: [PolicyType; 7] = [
    PolicyType::ServiceControlPolicy,
    PolicyType::ResourceControlPolicy,
    PolicyType::TagPolicy,
    PolicyType::BackupPolicy,
    PolicyType::AiservicesOptOutPolicy,
    PolicyType::ChatbotPolicy,
    PolicyType::DeclarativePolicyEc2,
];

/// One `ListRoots` call, reused for two things the policy walk needs: which
/// policy types are enabled (`ListPoliciesForTarget` demands a type filter and
/// rejects disabled ones) and the root id→name map (`ListParents` reports a
/// root's id but no name, and `DescribeOrganizationalUnit` doesn't accept a
/// root id). A failure degrades to guessing all types and an unnamed root.
async fn org_roots_info(client: &OrgClient) -> (Vec<PolicyType>, HashMap<String, String>) {
    let resp = match client.list_roots().send().await {
        Ok(r) => r,
        Err(_) => return (ALL_POLICY_TYPES.to_vec(), HashMap::new()),
    };
    let mut types: Vec<PolicyType> = Vec::new();
    let mut names = HashMap::new();
    for root in resp.roots() {
        if let Some(id) = root.id() {
            names.insert(id.to_string(), root.name().unwrap_or("Root").to_string());
        }
        for pts in root.policy_types() {
            if matches!(
                pts.status(),
                Some(aws_sdk_organizations::types::PolicyTypeStatus::Enabled)
            ) {
                if let Some(t) = pts.r#type() {
                    if !types.contains(t) {
                        types.push(t.clone());
                    }
                }
            }
        }
    }
    if types.is_empty() {
        types = ALL_POLICY_TYPES.to_vec();
    }
    (types, names)
}

/// Walk `child_id` up to the root via `ListParents`, returning the ancestors
/// **nearest-first** as `(name, id)`.
async fn ancestor_chain(
    client: &OrgClient,
    child_id: &str,
    root_names: &HashMap<String, String>,
) -> Result<Vec<(String, String)>> {
    let mut chain = Vec::new();
    let mut cursor = child_id.to_string();
    // Depth guard: the OU tree is capped at 5 levels, but a malformed
    // response must not spin here.
    for _ in 0..16 {
        let parents_resp = client
            .list_parents()
            .child_id(&cursor)
            .send()
            .await
            .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
        let parent = match parents_resp.parents().first() {
            Some(p) => p.clone(),
            None => break,
        };
        let parent_id = parent.id().unwrap_or_default().to_string();
        if parent_id.is_empty() {
            break;
        }
        if matches!(
            parent.r#type(),
            Some(aws_sdk_organizations::types::ParentType::Root)
        ) {
            let name = root_names
                .get(&parent_id)
                .cloned()
                .unwrap_or_else(|| "Root".to_string());
            chain.push((name, parent_id));
            break;
        }
        let ou_resp = client
            .describe_organizational_unit()
            .organizational_unit_id(&parent_id)
            .send()
            .await
            .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
        let name = ou_resp
            .organizational_unit()
            .and_then(|ou| ou.name())
            .unwrap_or(&parent_id)
            .to_string();
        chain.push((name, parent_id.clone()));
        cursor = parent_id;
    }
    Ok(chain)
}

/// List the policies attached to `target_id` itself plus every ancestor in
/// `chain` (nearest-first), across every enabled policy type. Returns the
/// flattened set and, separately, the first *denial* seen — a type that simply
/// isn't listable here is skipped silently, mirroring the list load's phase 3.
async fn effective_policies(
    client: &OrgClient,
    target_id: &str,
    chain: &[(String, String)],
    enabled: &[PolicyType],
) -> (Vec<OrgAttachedPolicy>, Option<String>) {
    let mut out = Vec::new();
    let mut denial = None;

    let mut targets: Vec<(Option<(String, String)>, String)> =
        vec![(None, target_id.to_string())];
    targets.extend(
        chain
            .iter()
            .map(|(name, id)| (Some((name.clone(), id.clone())), id.clone())),
    );

    for t in enabled {
        let label = policy_type_label(Some(t));
        for (source, id) in &targets {
            let mut paginator = client
                .list_policies_for_target()
                .target_id(id)
                .filter(t.clone())
                .into_paginator()
                .send();
            while let Some(result) = paginator.next().await {
                match result {
                    Ok(page) => {
                        for p in page.policies() {
                            out.push(OrgAttachedPolicy {
                                type_label: label.to_string(),
                                name: p.name().unwrap_or_default().to_string(),
                                policy_id: p.id().unwrap_or_default().to_string(),
                                source: source.clone(),
                            });
                        }
                    }
                    Err(e) => {
                        let msg = crate::error::sdk_error_message(&e);
                        if msg.to_lowercase().contains("denied") {
                            denial.get_or_insert(msg);
                        }
                        break;
                    }
                }
            }
        }
    }
    (out, denial)
}

// ── Lazy-loaded account details (OU path + effective policies) ─────────────

#[derive(Debug, Clone)]
pub struct OrgAccountDetails {
    /// Root-first `(name, id)` chain down to the account's parent OU.
    pub ou_path: Vec<(String, String)>,
    pub ou_path_error: Option<String>,
    /// Policies governing this account: attached directly **and** inherited
    /// from every OU above it, which is where most orgs attach their SCPs.
    pub policies: Vec<OrgAttachedPolicy>,
    pub policies_error: Option<String>,
}

pub async fn fetch_org_account_details(
    client: OrgClient,
    account_id: String,
) -> Result<OrgAccountDetails> {
    let (enabled, root_names) = org_roots_info(&client).await;
    // A failed climb degrades to direct attachments only rather than blanking
    // the Policies section — the two sections fail independently.
    let (chain, ou_path_error) = match ancestor_chain(&client, &account_id, &root_names).await {
        Ok(c) => (c, None),
        Err(e) => (Vec::new(), Some(e.to_string())),
    };
    let (policies, policies_error) =
        effective_policies(&client, &account_id, &chain, &enabled).await;

    let mut ou_path = chain;
    ou_path.reverse(); // root first, reading top-down like the console
    Ok(OrgAccountDetails {
        ou_path,
        ou_path_error,
        policies,
        policies_error,
    })
}

// ── Lazy-loaded OU details (children + policies + tags) ────────────────────

/// One account sitting directly under an OU.
#[derive(Debug, Clone)]
pub struct OrgChildAccount {
    pub account_id: String,
    pub account_name: String,
    pub email: String,
    pub status: String,
}

/// Everything the OU pane's Children / Policies / Tags sections need, fetched
/// in one go — the calls are small and always wanted together, so bundling
/// them behind a single `LazyMap` entry (keyed by OU id) beats three
/// independent fetches (the Route53-Profiles pattern).
#[derive(Debug, Clone, Default)]
pub struct OrgUnitDetails {
    pub child_ous: Vec<(String, String)>, // (name, ou id)
    pub accounts: Vec<OrgChildAccount>,
    /// Policies governing this OU: attached directly **and** inherited from
    /// every OU/root above it.
    pub policies: Vec<OrgAttachedPolicy>,
    pub tags: Vec<(String, String)>,
    /// Per-part failures, so one missing permission doesn't blank the others.
    pub children_error: Option<String>,
    pub policies_error: Option<String>,
    pub tags_error: Option<String>,
}

pub async fn fetch_org_ou_details(client: OrgClient, ou_id: String) -> Result<OrgUnitDetails> {
    let mut d = OrgUnitDetails::default();

    // Children: nested OUs, then the accounts parked directly in this OU.
    let mut paginator = client
        .list_organizational_units_for_parent()
        .parent_id(&ou_id)
        .into_paginator()
        .send();
    while let Some(result) = paginator.next().await {
        match result {
            Ok(page) => {
                for ou in page.organizational_units() {
                    d.child_ous.push((
                        ou.name().unwrap_or_default().to_string(),
                        ou.id().unwrap_or_default().to_string(),
                    ));
                }
            }
            Err(e) => {
                d.children_error = Some(crate::error::sdk_error_message(&e));
                break;
            }
        }
    }
    let mut paginator = client
        .list_accounts_for_parent()
        .parent_id(&ou_id)
        .into_paginator()
        .send();
    while let Some(result) = paginator.next().await {
        match result {
            Ok(page) => {
                for a in page.accounts() {
                    d.accounts.push(OrgChildAccount {
                        account_id: a.id().unwrap_or_default().to_string(),
                        account_name: a.name().unwrap_or_default().to_string(),
                        email: a.email().unwrap_or_default().to_string(),
                        status: a
                            .status()
                            .map(|s| s.as_str().to_string())
                            .unwrap_or_else(|| "—".to_string()),
                    });
                }
            }
            Err(e) => {
                d.children_error
                    .get_or_insert_with(|| crate::error::sdk_error_message(&e));
                break;
            }
        }
    }
    d.child_ous.sort_by(|a, b| a.0.cmp(&b.0));
    d.accounts.sort_by(|a, b| a.account_name.cmp(&b.account_name));

    // Policies: direct attachments plus everything inherited from the OUs and
    // root above this one — the same walk the account pane does.
    let (enabled, root_names) = org_roots_info(&client).await;
    let chain = ancestor_chain(&client, &ou_id, &root_names)
        .await
        .unwrap_or_default();
    let (policies, policies_error) = effective_policies(&client, &ou_id, &chain, &enabled).await;
    d.policies = policies;
    d.policies_error = policies_error;

    let mut paginator = client
        .list_tags_for_resource()
        .resource_id(&ou_id)
        .into_paginator()
        .send();
    while let Some(result) = paginator.next().await {
        match result {
            Ok(page) => {
                for tag in page.tags() {
                    d.tags.push((tag.key().to_string(), tag.value().to_string()));
                }
            }
            Err(e) => {
                d.tags_error = Some(crate::error::sdk_error_message(&e));
                break;
            }
        }
    }
    d.tags.sort_by(|a, b| a.0.cmp(&b.0));

    Ok(d)
}

// ── Lazy-loaded SCP details (document + attached targets) ──────────────────

#[derive(Debug, Clone)]
pub struct OrgScpDetails {
    pub document_json: String,
    pub attached_targets: Vec<(String, String, String)>, // (name, id, type)
}

pub async fn fetch_org_scp_details(client: OrgClient, policy_id: String) -> Result<OrgScpDetails> {
    let policy_resp = client
        .describe_policy()
        .policy_id(&policy_id)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;

    let document_json = policy_resp
        .policy()
        .and_then(|p| p.content())
        .map(|c| {
            serde_json::from_str::<serde_json::Value>(c)
                .ok()
                .and_then(|v| serde_json::to_string_pretty(&v).ok())
                .unwrap_or_else(|| c.to_string())
        })
        .unwrap_or_else(|| "No policy document".to_string());

    let mut attached_targets = Vec::new();
    let mut paginator = client
        .list_targets_for_policy()
        .policy_id(&policy_id)
        .into_paginator()
        .send();
    while let Some(result) = paginator.next().await {
        let page = result.map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
        for t in page.targets() {
            attached_targets.push((
                t.name().unwrap_or_default().to_string(),
                t.target_id().unwrap_or_default().to_string(),
                t.r#type()
                    .map(|ty| ty.as_str().to_string())
                    .unwrap_or_default(),
            ));
        }
    }

    Ok(OrgScpDetails {
        document_json,
        attached_targets,
    })
}

// ── Timestamp helpers ────────────────────────────────────────────────────

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
