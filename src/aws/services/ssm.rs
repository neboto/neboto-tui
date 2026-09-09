//! AWS Systems Manager (SSM).
//!
//! A dedicated `@ssm` service with five sub-tabs over five heterogeneous
//! resource types:
//! - **Parameters** (`SsmParameter`) — Parameter Store. Listing surfaces only
//!   metadata; the value is fetched on demand by an opt-in reveal (`x`) / copy
//!   (`Y`) via [`fetch_parameter_value`], shown transiently and never cached.
//! - **Documents** (`SsmDocument`) — SSM documents (`list_documents`, owner =
//!   self/shared). Content is viewable lazily via [`fetch_document_content`].
//! - **Fleet** (`SsmManagedInstance`) — SSM-managed instances
//!   (`describe_instance_information`): ping status, agent version, platform.
//!   The split pane's Associations section is a lazy
//!   [`fetch_instance_associations`] (per-instance State Manager status).
//! - **Patch** (`SsmPatchSummary`) — per-instance patch compliance
//!   (`list_resource_compliance_summaries`, ComplianceType = Patch).
//! - **Associations** (`SsmAssociation`) — State Manager associations
//!   (`list_associations`): document, schedule, targets, aggregated run
//!   status. Execution history + tags load lazily via
//!   [`fetch_ssm_assoc_detail`].
//! - **Run Command** (`SsmCommand`) — recent command history (`list_commands`,
//!   newest first, capped). Per-instance invocations + output snippets load
//!   lazily via [`fetch_command_invocations`].
//! - **Automation** (`SsmAutomationExecution`) — automation runs
//!   (`describe_automation_executions`, capped). Per-step detail loads lazily
//!   via [`fetch_automation_steps`].
//! - **Maintenance Windows** (`SsmMaintWindow`) —
//!   (`describe_maintenance_windows`). Targets + tasks + recent execution
//!   history load together lazily via [`fetch_maint_window_detail`].
//! - **Patch Baselines** (`SsmPatchBaseline`) — (`describe_patch_baselines`),
//!   shown on the Patch sub-tab alongside compliance (a pipe-filtered grouped
//!   tab). Approval rules + patch groups load lazily via
//!   [`fetch_patch_baseline_detail`]; AWS-provided baselines are `is_noise()`.
//! - **OpsItems** (`SsmOpsItem`) — OpsCenter items (`describe_ops_items`,
//!   capped; resolved/closed are `is_noise()`). Description + operational
//!   data load lazily via [`fetch_ops_item_detail`].
//! - **Sessions** (`SsmSession`) — Session Manager sessions
//!   (`describe_sessions`, Active + History in one phase, history capped).
//!   Flat `details()` — complete as flat.
//!
//! The streaming load is error-tolerant per resource type — a permission gap
//! on one API warns (`ResourceLoadWarning`) and the others keep streaming;
//! only an all-phases-empty failure goes fatal.

use crate::aws::client::AwsClients;
use crate::aws::resource::{native_state_label, Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use aws_sdk_ssm::Client as SsmClient;
use std::any::Any;
use std::collections::HashMap;
use tokio::sync::mpsc;

pub struct SsmService {
    ssm: SsmClient,
}

impl SsmService {
    pub fn new(aws_clients: &AwsClients) -> Self {
        Self {
            ssm: aws_clients.ssm_client(),
        }
    }
}

fn fmt_dt(dt: Option<&aws_sdk_ssm::primitives::DateTime>) -> String {
    dt.map(|t| {
        let s = t.to_string();
        s.split('.').next().unwrap_or(&s).replace('T', " ")
    })
    .unwrap_or_default()
}

#[async_trait]
impl AwsService for SsmService {
    fn service_type(&self) -> ServiceType {
        ServiceType::Ssm
    }

    fn name(&self) -> &str {
        "Systems Manager"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        self.list_resources_streaming(tx, ServiceType::Ssm).await?;
        Ok(vec![])
    }

    async fn list_resources_streaming(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        use std::sync::atomic::{AtomicUsize, Ordering};

        // All nine phases run CONCURRENTLY (`futures::join!` — interleaved on
        // this task, no spawns), so the sub-tab you're on isn't blocked behind
        // an unrelated type's pagination (e.g. Associations behind Documents
        // after a profile switch). Every SSM view filters by resource type, so
        // interleaved batch order is invisible.
        let total = AtomicUsize::new(0);
        let failed_phases = AtomicUsize::new(0);

        let emit = |batch: Vec<Box<dyn Resource>>, msg: &str| {
            if batch.is_empty() {
                return;
            }
            let count = total.fetch_add(batch.len(), Ordering::SeqCst) + batch.len();
            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                service: service_type,
                resources: batch,
                progress: LoadProgress {
                    loaded_count: count,
                    total_count: None,
                    status_message: Some(msg.to_string()),
                },
            });
        };
        // A phase failure warns and keeps the other phases streaming (a fatal
        // ResourceLoadError would make the app drop later batches).
        let warn = |stage: &str, msg: String| {
            failed_phases.fetch_add(1, Ordering::SeqCst);
            let _ = event_tx.send(Event::ResourceLoadWarning {
                service: service_type,
                warning: format!("{}: {}", stage, msg),
            });
        };

        // Simple paginated phases: one page → one batch.
        macro_rules! phase {
            ($stage:expr, $msg:expr, $pag:expr, $items:ident, $ty:ty) => {
                async {
                    let mut pag = $pag.into_paginator().send();
                    loop {
                        match pag.next().await {
                            Some(Ok(page)) => emit(
                                page.$items()
                                    .iter()
                                    .map(|x| Box::new(<$ty>::from_sdk(x)) as Box<dyn Resource>)
                                    .collect(),
                                $msg,
                            ),
                            Some(Err(e)) => {
                                warn($stage, crate::error::sdk_error_message(&e));
                                break;
                            }
                            None => break,
                        }
                    }
                }
            };
        }

        let parameters = phase!(
            "parameters",
            "Loading SSM parameters…",
            self.ssm.describe_parameters(),
            parameters,
            SsmParameter
        );
        let documents = phase!(
            "documents",
            "Loading SSM documents…",
            self.ssm.list_documents(),
            document_identifiers,
            SsmDocument
        );
        let fleet = phase!(
            "fleet",
            "Loading SSM fleet…",
            self.ssm.describe_instance_information(),
            instance_information_list,
            SsmManagedInstance
        );
        let associations = phase!(
            "associations",
            "Loading SSM associations…",
            self.ssm.list_associations(),
            associations,
            SsmAssociation
        );
        let windows = phase!(
            "maintenance windows",
            "Loading SSM maintenance windows…",
            self.ssm.describe_maintenance_windows(),
            window_identities,
            SsmMaintWindow
        );
        let baselines = phase!(
            "patch baselines",
            "Loading SSM patch baselines…",
            self.ssm.describe_patch_baselines(),
            baseline_identities,
            SsmPatchBaseline
        );

        // Patch compliance filters to the Patch compliance type.
        let compliance = async {
            let mut pag = self
                .ssm
                .list_resource_compliance_summaries()
                .into_paginator()
                .send();
            loop {
                match pag.next().await {
                    Some(Ok(page)) => emit(
                        page.resource_compliance_summary_items()
                            .iter()
                            .filter(|c| c.compliance_type() == Some("Patch"))
                            .map(|c| Box::new(SsmPatchSummary::from_sdk(c)) as Box<dyn Resource>)
                            .collect(),
                        "Loading SSM patch compliance…",
                    ),
                    Some(Err(e)) => {
                        warn("patch", crate::error::sdk_error_message(&e));
                        break;
                    }
                    None => break,
                }
            }
        };

        // Run Command history is capped (newest first — the API's order).
        let commands = async {
            let mut seen = 0usize;
            let mut pag = self.ssm.list_commands().into_paginator().send();
            'pages: loop {
                match pag.next().await {
                    Some(Ok(page)) => {
                        let mut batch: Vec<Box<dyn Resource>> = Vec::new();
                        for c in page.commands() {
                            if seen >= MAX_COMMANDS {
                                break;
                            }
                            seen += 1;
                            batch.push(Box::new(SsmCommand::from_sdk(c)));
                        }
                        emit(batch, "Loading SSM commands…");
                        if seen >= MAX_COMMANDS {
                            break 'pages;
                        }
                    }
                    Some(Err(e)) => {
                        warn("run command", crate::error::sdk_error_message(&e));
                        break;
                    }
                    None => break,
                }
            }
        };

        // Automation executions are capped the same way.
        let automations = async {
            let mut seen = 0usize;
            let mut pag = self
                .ssm
                .describe_automation_executions()
                .into_paginator()
                .send();
            'pages: loop {
                match pag.next().await {
                    Some(Ok(page)) => {
                        let mut batch: Vec<Box<dyn Resource>> = Vec::new();
                        for x in page.automation_execution_metadata_list() {
                            if seen >= MAX_AUTOMATION_EXECUTIONS {
                                break;
                            }
                            seen += 1;
                            batch.push(Box::new(SsmAutomationExecution::from_sdk(x)));
                        }
                        emit(batch, "Loading SSM automation executions…");
                        if seen >= MAX_AUTOMATION_EXECUTIONS {
                            break 'pages;
                        }
                    }
                    Some(Err(e)) => {
                        warn("automation", crate::error::sdk_error_message(&e));
                        break;
                    }
                    None => break,
                }
            }
        };

        // OpsCenter items (capped; the API pages them newest-ish first).
        let ops_items = async {
            let mut seen = 0usize;
            let mut pag = self.ssm.describe_ops_items().into_paginator().send();
            'pages: loop {
                match pag.next().await {
                    Some(Ok(page)) => {
                        let mut batch: Vec<Box<dyn Resource>> = Vec::new();
                        for o in page.ops_item_summaries() {
                            if seen >= MAX_OPS_ITEMS {
                                break;
                            }
                            seen += 1;
                            batch.push(Box::new(SsmOpsItem::from_sdk(o)));
                        }
                        emit(batch, "Loading SSM OpsItems…");
                        if seen >= MAX_OPS_ITEMS {
                            break 'pages;
                        }
                    }
                    Some(Err(e)) => {
                        warn("opsitems", crate::error::sdk_error_message(&e));
                        break;
                    }
                    None => break,
                }
            }
        };

        // Session Manager sessions: the API demands a state filter, so Active
        // and History are two calls in one phase (history capped). Legs warn
        // individually but the phase counts as failed only when both do.
        let sessions = async {
            let mut legs_failed = 0usize;
            for (state, cap) in [
                (aws_sdk_ssm::types::SessionState::Active, usize::MAX),
                (
                    aws_sdk_ssm::types::SessionState::History,
                    MAX_SESSION_HISTORY,
                ),
            ] {
                let mut seen = 0usize;
                let mut pag = self
                    .ssm
                    .describe_sessions()
                    .state(state.clone())
                    .into_paginator()
                    .send();
                'pages: loop {
                    match pag.next().await {
                        Some(Ok(page)) => {
                            let mut batch: Vec<Box<dyn Resource>> = Vec::new();
                            for s in page.sessions() {
                                if seen >= cap {
                                    break;
                                }
                                seen += 1;
                                batch.push(Box::new(SsmSession::from_sdk(s)));
                            }
                            emit(batch, "Loading SSM sessions…");
                            if seen >= cap {
                                break 'pages;
                            }
                        }
                        Some(Err(e)) => {
                            legs_failed += 1;
                            let _ = event_tx.send(Event::ResourceLoadWarning {
                                service: service_type,
                                warning: format!(
                                    "sessions ({}): {}",
                                    state.as_str().to_lowercase(),
                                    crate::error::sdk_error_message(&e)
                                ),
                            });
                            break;
                        }
                        None => break,
                    }
                }
            }
            if legs_failed == 2 {
                failed_phases.fetch_add(1, Ordering::SeqCst);
            }
        };

        futures::join!(
            parameters,
            documents,
            fleet,
            compliance,
            associations,
            commands,
            automations,
            windows,
            baselines,
            ops_items,
            sessions
        );

        let total = total.load(Ordering::SeqCst);
        // Only an all-phases-empty failure is fatal — nothing streamed.
        if failed_phases.load(Ordering::SeqCst) == 11 && total == 0 {
            let _ = event_tx.send(Event::ResourceLoadError {
                service: service_type,
                error: "All SSM resource types failed to load".to_string(),
            });
            return Ok(());
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

// ── SsmParameter ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct SsmParameter {
    pub name: String,
    pub arn: String,
    pub param_type: String,
    pub tier: String,
    pub version: i64,
    pub data_type: Option<String>,
    pub description: Option<String>,
    pub allowed_pattern: Option<String>,
    pub kms_key_id: Option<String>,
    pub last_modified: String,
    pub last_modified_user: Option<String>,
    pub tags: HashMap<String, String>,
}

impl SsmParameter {
    pub fn from_sdk(p: &aws_sdk_ssm::types::ParameterMetadata) -> Self {
        Self {
            name: p.name().unwrap_or("").to_string(),
            arn: p.arn().unwrap_or("").to_string(),
            param_type: p.r#type().map(|t| t.as_str().to_string()).unwrap_or_default(),
            tier: p.tier().map(|t| t.as_str().to_string()).unwrap_or_default(),
            version: p.version(),
            data_type: p.data_type().map(|d| d.to_string()),
            description: p.description().map(|d| d.to_string()),
            allowed_pattern: p.allowed_pattern().map(|a| a.to_string()),
            kms_key_id: p.key_id().map(|k| k.to_string()),
            last_modified: fmt_dt(p.last_modified_date()),
            last_modified_user: p.last_modified_user().map(|u| u.to_string()),
            tags: HashMap::new(),
        }
    }

    /// True for `SecureString` parameters — the value is KMS-encrypted at rest.
    pub fn is_secure(&self) -> bool {
        self.param_type.eq_ignore_ascii_case("SecureString")
    }
}

crate::sections! {
    pub enum SsmParameterDetailSection,
    pub static SSM_PARAMETER_SECTIONS = [
        Details "Details",
        Value "Value",
        History "History" => crate::app::App::trigger_ssm_param_detail_load,
        Tags "Tags" => crate::app::App::trigger_ssm_param_detail_load,
    ]
}

impl Resource for SsmParameter {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&SSM_PARAMETER_SECTIONS)
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws ssm get-parameter --name {}",
            crate::aws::resource::shell_quote(self.id())
        ))
    }

    fn id(&self) -> &str {
        &self.name
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn resource_type(&self) -> &str {
        "SSM Parameter"
    }

    fn state(&self) -> ResourceState {
        ResourceState::stateless()
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!("{} {} {}", self.name, self.param_type, self.tier)
    }

    fn details(&self) -> Vec<(String, String)> {
        // Fallback only — the rich split pane lives in details_pane.rs.
        let mut rows = vec![
            ("Name".to_string(), self.name.clone()),
            ("Type".to_string(), self.param_type.clone()),
            ("Tier".to_string(), self.tier.clone()),
            ("Version".to_string(), self.version.to_string()),
        ];
        if let Some(dt) = &self.data_type {
            rows.push(("Data Type".to_string(), dt.clone()));
        }
        if !self.last_modified.is_empty() {
            rows.push(("Last Modified".to_string(), self.last_modified.clone()));
        }
        if let Some(user) = &self.last_modified_user {
            rows.push(("Modified By".to_string(), user.clone()));
        }
        if let Some(kms) = &self.kms_key_id {
            rows.push(("KMS Key".to_string(), kms.clone()));
        }
        if let Some(desc) = &self.description {
            rows.push(("Description".to_string(), desc.clone()));
        }
        if let Some(pattern) = &self.allowed_pattern {
            rows.push(("Allowed Pattern".to_string(), pattern.clone()));
        }
        if !self.arn.is_empty() {
            rows.push(("ARN".to_string(), self.arn.clone()));
        }
        rows
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/systems-manager/parameters/{}/description?region={}",
            region,
            self.name.trim_start_matches('/'),
            region
        ))
    }
}

// ── SsmDocument ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct SsmDocument {
    pub name: String,
    pub owner: String,
    pub doc_type: String,
    pub format: String,
    pub platforms: String,
    pub schema_version: String,
    pub default_version: String,
    pub version_name: Option<String>,
    pub target_type: Option<String>,
    pub tags: HashMap<String, String>,
}

impl SsmDocument {
    pub fn from_sdk(d: &aws_sdk_ssm::types::DocumentIdentifier) -> Self {
        let platforms = d
            .platform_types()
            .iter()
            .map(|p| p.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let tags: HashMap<String, String> = d
            .tags()
            .iter()
            .map(|t| (t.key().to_string(), t.value().to_string()))
            .collect();
        Self {
            name: d.name().unwrap_or("").to_string(),
            owner: d.owner().unwrap_or("").to_string(),
            doc_type: d
                .document_type()
                .map(|t| t.as_str().to_string())
                .unwrap_or_default(),
            format: d
                .document_format()
                .map(|f| f.as_str().to_string())
                .unwrap_or_default(),
            platforms,
            schema_version: d.schema_version().unwrap_or("").to_string(),
            default_version: d.document_version().unwrap_or("").to_string(),
            version_name: d.version_name().map(|v| v.to_string()),
            target_type: d.target_type().map(|t| t.to_string()),
            tags,
        }
    }
}

crate::sections! {
    pub enum SsmDocumentDetailSection,
    pub static SSM_DOCUMENT_SECTIONS = [
        Details "Details",
        Content "Content" => crate::app::App::trigger_ssm_document_content_load,
        Tags "Tags",
    ]
}

impl Resource for SsmDocument {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&SSM_DOCUMENT_SECTIONS)
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws ssm describe-document --name {}",
            crate::aws::resource::shell_quote(self.id())
        ))
    }

    fn id(&self) -> &str {
        &self.name
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn resource_type(&self) -> &str {
        "SSM Document"
    }

    fn state(&self) -> ResourceState {
        ResourceState::stateless()
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!("{} {} {}", self.name, self.owner, self.doc_type)
    }

    fn details(&self) -> Vec<(String, String)> {
        let mut rows = vec![
            ("Name".to_string(), self.name.clone()),
            ("Type".to_string(), self.doc_type.clone()),
            ("Format".to_string(), self.format.clone()),
            ("Owner".to_string(), self.owner.clone()),
        ];
        if !self.platforms.is_empty() {
            rows.push(("Platforms".to_string(), self.platforms.clone()));
        }
        if !self.default_version.is_empty() {
            rows.push(("Default Version".to_string(), self.default_version.clone()));
        }
        if let Some(vn) = &self.version_name {
            rows.push(("Version Name".to_string(), vn.clone()));
        }
        if !self.schema_version.is_empty() {
            rows.push(("Schema Version".to_string(), self.schema_version.clone()));
        }
        if let Some(tt) = &self.target_type {
            rows.push(("Target Type".to_string(), tt.clone()));
        }
        rows
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/systems-manager/documents/{}/description?region={}",
            region, self.name, region
        ))
    }
}

// ── SsmManagedInstance (Fleet) ────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct SsmManagedInstance {
    pub instance_id: String,
    pub node_name: Option<String>,
    pub ping_status: String,
    pub last_ping: String,
    pub agent_version: String,
    pub is_latest: bool,
    pub platform_type: String,
    pub platform_name: Option<String>,
    pub platform_version: Option<String>,
    pub computer_name: Option<String>,
    pub ip_address: Option<String>,
    pub instance_kind: String,
    pub association_status: Option<String>,
    pub iam_role: Option<String>,
    pub tags: HashMap<String, String>,
}

impl SsmManagedInstance {
    pub fn from_sdk(i: &aws_sdk_ssm::types::InstanceInformation) -> Self {
        Self {
            instance_id: i.instance_id().unwrap_or("").to_string(),
            node_name: i.name().filter(|n| !n.is_empty()).map(|n| n.to_string()),
            ping_status: i
                .ping_status()
                .map(|p| p.as_str().to_string())
                .unwrap_or_default(),
            last_ping: fmt_dt(i.last_ping_date_time()),
            agent_version: i.agent_version().unwrap_or("").to_string(),
            is_latest: i.is_latest_version().unwrap_or(false),
            platform_type: i
                .platform_type()
                .map(|p| p.as_str().to_string())
                .unwrap_or_default(),
            platform_name: i.platform_name().map(|p| p.to_string()),
            platform_version: i.platform_version().map(|p| p.to_string()),
            computer_name: i.computer_name().map(|c| c.to_string()),
            ip_address: i.ip_address().map(|c| c.to_string()),
            instance_kind: i
                .resource_type()
                .map(|r| r.as_str().to_string())
                .unwrap_or_default(),
            association_status: i.association_status().map(|a| a.to_string()),
            iam_role: i.iam_role().filter(|r| !r.is_empty()).map(|r| r.to_string()),
            tags: HashMap::new(),
        }
    }

    /// Whether the SSM agent is reachable (Session Manager / Run Command work).
    pub fn is_online(&self) -> bool {
        self.ping_status.eq_ignore_ascii_case("Online")
    }
}

crate::sections! {
    pub enum SsmFleetDetailSection,
    pub static SSM_FLEET_SECTIONS = [
        Overview "Overview" => crate::app::App::trigger_ssm_fleet_section_load,
        Associations "Associations" => crate::app::App::trigger_ssm_fleet_section_load,
        Inventory "Inventory" => crate::app::App::trigger_ssm_fleet_section_load,
        Patches "Patches" => crate::app::App::trigger_ssm_fleet_section_load,
    ]
}

impl Resource for SsmManagedInstance {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&SSM_FLEET_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.instance_id
    }

    fn name(&self) -> &str {
        self.node_name.as_deref().unwrap_or(&self.instance_id)
    }

    fn resource_type(&self) -> &str {
        "SSM Managed Instance"
    }

    fn state(&self) -> ResourceState {
        match self.ping_status.to_ascii_lowercase().as_str() {
            "online" => ResourceState::Running,
            "connectionlost" => ResourceState::Unavailable,
            "inactive" => ResourceState::Stopped,
            other => ResourceState::Unknown(other.to_string()),
        }
    }
    fn state_label(&self) -> String {
        native_state_label(&self.ping_status, || self.state())
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!(
            "{} {} {} {}",
            self.instance_id,
            self.node_name.clone().unwrap_or_default(),
            self.ping_status,
            self.platform_name.clone().unwrap_or_default(),
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        let mut rows = vec![
            ("Instance ID".to_string(), self.instance_id.clone()),
            ("Ping Status".to_string(), self.ping_status.clone()),
            ("Kind".to_string(), self.instance_kind.clone()),
            (
                "Agent".to_string(),
                if self.is_latest {
                    format!("{} (latest)", self.agent_version)
                } else {
                    format!("{} (update available)", self.agent_version)
                },
            ),
        ];
        if let Some(n) = &self.node_name {
            rows.push(("Name".to_string(), n.clone()));
        }
        let platform = match (&self.platform_name, &self.platform_version) {
            (Some(n), Some(v)) => format!("{} {} ({})", n, v, self.platform_type),
            (Some(n), None) => format!("{} ({})", n, self.platform_type),
            _ => self.platform_type.clone(),
        };
        rows.push(("Platform".to_string(), platform));
        if let Some(ip) = &self.ip_address {
            rows.push(("IP Address".to_string(), ip.clone()));
        }
        if let Some(c) = &self.computer_name {
            rows.push(("Computer Name".to_string(), c.clone()));
        }
        if let Some(r) = &self.iam_role {
            rows.push(("IAM Role".to_string(), r.clone()));
        }
        if let Some(a) = &self.association_status {
            rows.push(("Association".to_string(), a.clone()));
        }
        if !self.last_ping.is_empty() {
            rows.push(("Last Ping".to_string(), self.last_ping.clone()));
        }
        rows
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/systems-manager/managed-instances/{}/description?region={}",
            region, self.instance_id, region
        ))
    }
}

// ── SsmPatchSummary (Patch compliance) ────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct SsmPatchSummary {
    pub resource_id: String,
    pub status: String,
    pub overall_severity: Option<String>,
    pub compliant_count: i32,
    pub non_compliant_count: i32,
    pub severity_breakdown: Option<String>,
    pub tags: HashMap<String, String>,
}

impl SsmPatchSummary {
    pub fn from_sdk(c: &aws_sdk_ssm::types::ResourceComplianceSummaryItem) -> Self {
        let compliant_count = c.compliant_summary().map(|s| s.compliant_count()).unwrap_or(0);
        let non_compliant_count = c
            .non_compliant_summary()
            .map(|s| s.non_compliant_count())
            .unwrap_or(0);
        let severity_breakdown = c.non_compliant_summary().and_then(|s| s.severity_summary()).map(|s| {
            let parts = [
                ("Critical", s.critical_count()),
                ("High", s.high_count()),
                ("Medium", s.medium_count()),
                ("Low", s.low_count()),
            ];
            parts
                .iter()
                .filter(|(_, n)| *n > 0)
                .map(|(label, n)| format!("{} {}", n, label))
                .collect::<Vec<_>>()
                .join(", ")
        });
        Self {
            resource_id: c.resource_id().unwrap_or("").to_string(),
            status: c
                .status()
                .map(|s| s.as_str().to_string())
                .unwrap_or_default(),
            overall_severity: c.overall_severity().map(|s| s.as_str().to_string()),
            compliant_count,
            non_compliant_count,
            severity_breakdown: severity_breakdown.filter(|s| !s.is_empty()),
            tags: HashMap::new(),
        }
    }

    pub fn is_compliant(&self) -> bool {
        self.status.eq_ignore_ascii_case("COMPLIANT")
    }
}

impl Resource for SsmPatchSummary {
    fn id(&self) -> &str {
        &self.resource_id
    }

    fn name(&self) -> &str {
        &self.resource_id
    }

    fn resource_type(&self) -> &str {
        "SSM Patch Compliance"
    }

    fn state(&self) -> ResourceState {
        if self.is_compliant() {
            ResourceState::Running
        } else {
            ResourceState::Unavailable
        }
    }
    fn state_label(&self) -> String {
        native_state_label(&self.status, || self.state())
    }

    fn is_noise(&self) -> bool {
        // Hide fully-compliant instances so non-compliant ones stand out.
        self.is_compliant()
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!(
            "{} {} {}",
            self.resource_id,
            self.status,
            self.overall_severity.clone().unwrap_or_default()
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        let mut rows = vec![
            ("Instance ID".to_string(), self.resource_id.clone()),
            ("Status".to_string(), self.status.clone()),
        ];
        if let Some(sev) = &self.overall_severity {
            rows.push(("Overall Severity".to_string(), sev.clone()));
        }
        rows.push(("Compliant".to_string(), self.compliant_count.to_string()));
        rows.push((
            "Non-Compliant".to_string(),
            self.non_compliant_count.to_string(),
        ));
        if let Some(b) = &self.severity_breakdown {
            rows.push(("Breakdown".to_string(), b.clone()));
        }
        rows
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/systems-manager/managed-instances/{}/patch?region={}",
            region, self.resource_id, region
        ))
    }
}

// ── SsmAssociation (State Manager) ────────────────────────────────────────────

/// A State Manager association: a document applied to targets on a schedule.
/// Everything here comes from the `ListAssociations` summary; execution
/// history + tags load lazily via [`fetch_ssm_assoc_detail`].
#[derive(Debug, Clone)]
pub struct SsmAssociation {
    pub association_id: String,
    pub association_name: Option<String>,
    pub document_name: String,
    pub document_version: Option<String>,
    pub association_version: Option<String>,
    /// Legacy direct-instance associations carry the instance id instead of
    /// target expressions.
    pub instance_id: Option<String>,
    pub schedule: Option<String>,
    pub schedule_offset: Option<i32>,
    pub last_execution: String,
    pub status: Option<String>,
    pub detailed_status: Option<String>,
    /// Aggregated per-status resource counts from the overview, sorted by key
    /// (e.g. `[("Failed", 1), ("Success", 9)]`).
    pub status_counts: Vec<(String, i32)>,
    /// Target expressions as `(key, values)` — e.g.
    /// `("tag:Environment", ["prod"])` or `("InstanceIds", ["i-…"])`.
    pub targets: Vec<(String, Vec<String>)>,
    pub tags: HashMap<String, String>,
}

impl SsmAssociation {
    pub fn from_sdk(a: &aws_sdk_ssm::types::Association) -> Self {
        let overview = a.overview();
        let mut status_counts: Vec<(String, i32)> = overview
            .and_then(|o| o.association_status_aggregated_count())
            .map(|m| m.iter().map(|(k, v)| (k.clone(), *v)).collect())
            .unwrap_or_default();
        status_counts.sort_by(|x, y| x.0.cmp(&y.0));
        let targets = a
            .targets()
            .iter()
            .map(|t| {
                (
                    t.key().unwrap_or("").to_string(),
                    t.values().iter().map(|v| v.to_string()).collect(),
                )
            })
            .collect();
        Self {
            association_id: a.association_id().unwrap_or("").to_string(),
            association_name: a.association_name().map(|n| n.to_string()),
            document_name: a.name().unwrap_or("").to_string(),
            document_version: a.document_version().map(|v| v.to_string()),
            association_version: a.association_version().map(|v| v.to_string()),
            instance_id: a.instance_id().map(|i| i.to_string()),
            schedule: a.schedule_expression().map(|s| s.to_string()),
            schedule_offset: a.schedule_offset(),
            last_execution: fmt_dt(a.last_execution_date()),
            status: overview.and_then(|o| o.status()).map(|s| s.to_string()),
            detailed_status: overview
                .and_then(|o| o.detailed_status())
                .map(|s| s.to_string()),
            status_counts,
            targets,
            tags: HashMap::new(),
        }
    }
}

crate::sections! {
    pub enum SsmAssociationDetailSection,
    pub static SSM_ASSOCIATION_SECTIONS = [
        Overview "Overview",
        Targets "Targets",
        Executions "Executions" => crate::app::App::trigger_ssm_assoc_detail_load,
        Tags "Tags" => crate::app::App::trigger_ssm_assoc_detail_load,
    ]
}

impl Resource for SsmAssociation {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&SSM_ASSOCIATION_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.association_id
    }

    fn name(&self) -> &str {
        self.association_name
            .as_deref()
            .filter(|n| !n.is_empty())
            .unwrap_or(&self.document_name)
    }

    fn resource_type(&self) -> &str {
        "SSM Association"
    }

    fn state(&self) -> ResourceState {
        match self
            .status
            .as_deref()
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str()
        {
            "success" => ResourceState::Available,
            "failed" => ResourceState::Unavailable,
            "pending" => ResourceState::Pending,
            "" => ResourceState::Unknown("no runs".to_string()),
            other => ResourceState::Unknown(other.to_string()),
        }
    }
    fn state_label(&self) -> String {
        native_state_label(self.status.as_deref().unwrap_or(""), || self.state())
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        // Include the target expressions (tag filters / instance ids) so an
        // association is findable by what it targets.
        let target_values: Vec<&str> = self
            .targets
            .iter()
            .flat_map(|(_, vs)| vs.iter().map(|v| v.as_str()))
            .collect();
        format!(
            "{} {} {} {} {} {}",
            self.association_name.clone().unwrap_or_default(),
            self.document_name,
            self.association_id,
            self.status.clone().unwrap_or_default(),
            self.instance_id.clone().unwrap_or_default(),
            target_values.join(" "),
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        // Fallback only — the rich split pane lives in details_pane.rs.
        let mut rows = vec![
            ("Association ID".to_string(), self.association_id.clone()),
            ("Document".to_string(), self.document_name.clone()),
        ];
        if let Some(n) = &self.association_name {
            rows.push(("Name".to_string(), n.clone()));
        }
        if let Some(s) = &self.status {
            rows.push(("Status".to_string(), s.clone()));
        }
        if let Some(s) = &self.schedule {
            rows.push(("Schedule".to_string(), s.clone()));
        }
        if !self.last_execution.is_empty() {
            rows.push(("Last Execution".to_string(), self.last_execution.clone()));
        }
        rows
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/systems-manager/state-manager/{}/description?region={}",
            region, self.association_id, region
        ))
    }
}

// ── SsmCommand (Run Command history) ──────────────────────────────────────────

/// Cap on the Run Command history rows loaded (newest first — the API's order).
pub const MAX_COMMANDS: usize = 100;

/// One Run Command invocation from `ListCommands`. Per-instance results load
/// lazily via [`fetch_command_invocations`].
#[derive(Debug, Clone)]
pub struct SsmCommand {
    pub command_id: String,
    pub document_name: String,
    pub status: String,
    pub status_details: Option<String>,
    pub comment: Option<String>,
    pub requested: String,
    pub instance_ids: Vec<String>,
    pub targets: Vec<(String, Vec<String>)>,
    pub target_count: i32,
    pub completed_count: i32,
    pub error_count: i32,
    pub delivery_timed_out_count: i32,
    pub max_concurrency: Option<String>,
    pub max_errors: Option<String>,
    /// `s3://bucket/prefix` when output goes to S3 (jumpable).
    pub output_s3: Option<String>,
    /// CloudWatch log group when CW output is enabled.
    pub cw_log_group: Option<String>,
    pub timeout_seconds: Option<i32>,
    pub parameters: Vec<(String, String)>,
    pub tags: HashMap<String, String>,
}

impl SsmCommand {
    pub fn from_sdk(c: &aws_sdk_ssm::types::Command) -> Self {
        let targets = c
            .targets()
            .iter()
            .map(|t| {
                (
                    t.key().unwrap_or("").to_string(),
                    t.values().iter().map(|v| v.to_string()).collect(),
                )
            })
            .collect();
        let mut parameters: Vec<(String, String)> = c
            .parameters()
            .map(|m| {
                m.iter()
                    .map(|(k, v)| (k.clone(), v.join(", ")))
                    .collect()
            })
            .unwrap_or_default();
        parameters.sort_by(|a, b| a.0.cmp(&b.0));
        let output_s3 = c.output_s3_bucket_name().map(|b| {
            let prefix = c.output_s3_key_prefix().unwrap_or("");
            if prefix.is_empty() {
                format!("s3://{}", b)
            } else {
                format!("s3://{}/{}", b, prefix)
            }
        });
        let cw_log_group = c
            .cloud_watch_output_config()
            .filter(|cfg| cfg.cloud_watch_output_enabled())
            .map(|cfg| {
                let g = cfg.cloud_watch_log_group_name().unwrap_or("");
                if g.is_empty() {
                    format!("/aws/ssm/{}", c.document_name().unwrap_or(""))
                } else {
                    g.to_string()
                }
            });
        Self {
            command_id: c.command_id().unwrap_or("").to_string(),
            document_name: c.document_name().unwrap_or("").to_string(),
            status: c
                .status()
                .map(|s| s.as_str().to_string())
                .unwrap_or_default(),
            status_details: c
                .status_details()
                .filter(|d| !d.is_empty())
                .map(|d| d.to_string()),
            comment: c.comment().filter(|s| !s.is_empty()).map(|s| s.to_string()),
            requested: fmt_dt(c.requested_date_time()),
            instance_ids: c.instance_ids().iter().map(|i| i.to_string()).collect(),
            targets,
            target_count: c.target_count(),
            completed_count: c.completed_count(),
            error_count: c.error_count(),
            delivery_timed_out_count: c.delivery_timed_out_count(),
            max_concurrency: c.max_concurrency().map(|m| m.to_string()),
            max_errors: c.max_errors().map(|m| m.to_string()),
            output_s3,
            cw_log_group,
            timeout_seconds: c.timeout_seconds(),
            parameters,
            tags: HashMap::new(),
        }
    }
}

crate::sections! {
    pub enum SsmCommandDetailSection,
    pub static SSM_COMMAND_SECTIONS = [
        Overview "Overview",
        Invocations "Invocations" => crate::app::App::trigger_ssm_cmd_invocations_load,
    ]
}

impl Resource for SsmCommand {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&SSM_COMMAND_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.command_id
    }

    fn name(&self) -> &str {
        &self.document_name
    }

    fn resource_type(&self) -> &str {
        "SSM Command"
    }

    fn state(&self) -> ResourceState {
        match self.status.to_ascii_lowercase().as_str() {
            "success" => ResourceState::Available,
            "failed" | "timedout" => ResourceState::Unavailable,
            "cancelled" | "cancelling" => ResourceState::Stopped,
            "pending" | "inprogress" => ResourceState::Pending,
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
        // Include instance ids + target expressions so "which commands ran on
        // i-…" is a plain fuzzy search.
        let target_values: Vec<&str> = self
            .targets
            .iter()
            .flat_map(|(_, vs)| vs.iter().map(|v| v.as_str()))
            .collect();
        format!(
            "{} {} {} {} {} {}",
            self.command_id,
            self.document_name,
            self.status,
            self.comment.clone().unwrap_or_default(),
            self.instance_ids.join(" "),
            target_values.join(" "),
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        // Fallback only — the rich split pane lives in details_pane.rs.
        vec![
            ("Command ID".to_string(), self.command_id.clone()),
            ("Document".to_string(), self.document_name.clone()),
            ("Status".to_string(), self.status.clone()),
            ("Requested".to_string(), self.requested.clone()),
        ]
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/systems-manager/run-command/{}?region={}",
            region, self.command_id, region
        ))
    }
}

// ── SsmAutomationExecution ────────────────────────────────────────────────────

/// Cap on the automation-execution rows loaded (newest first).
pub const MAX_AUTOMATION_EXECUTIONS: usize = 100;

/// One automation run from `DescribeAutomationExecutions`. Step detail loads
/// lazily via [`fetch_automation_steps`].
#[derive(Debug, Clone)]
pub struct SsmAutomationExecution {
    pub execution_id: String,
    pub document_name: String,
    pub document_version: Option<String>,
    pub status: String,
    pub start: String,
    pub end: String,
    pub executed_by: Option<String>,
    pub mode: Option<String>,
    pub automation_type: Option<String>,
    pub current_step: Option<String>,
    pub current_action: Option<String>,
    pub failure_message: Option<String>,
    pub parent_id: Option<String>,
    pub target: Option<String>,
    pub targets: Vec<(String, Vec<String>)>,
    pub outputs: Vec<(String, String)>,
    pub tags: HashMap<String, String>,
}

impl SsmAutomationExecution {
    pub fn from_sdk(x: &aws_sdk_ssm::types::AutomationExecutionMetadata) -> Self {
        let targets = x
            .targets()
            .iter()
            .map(|t| {
                (
                    t.key().unwrap_or("").to_string(),
                    t.values().iter().map(|v| v.to_string()).collect(),
                )
            })
            .collect();
        let mut outputs: Vec<(String, String)> = x
            .outputs()
            .map(|m| {
                m.iter()
                    .map(|(k, v)| (k.clone(), v.join(", ")))
                    .collect()
            })
            .unwrap_or_default();
        outputs.sort_by(|a, b| a.0.cmp(&b.0));
        Self {
            execution_id: x.automation_execution_id().unwrap_or("").to_string(),
            document_name: x.document_name().unwrap_or("").to_string(),
            document_version: x.document_version().map(|v| v.to_string()),
            status: x
                .automation_execution_status()
                .map(|s| s.as_str().to_string())
                .unwrap_or_default(),
            start: fmt_dt(x.execution_start_time()),
            end: fmt_dt(x.execution_end_time()),
            executed_by: x.executed_by().map(|e| e.to_string()),
            mode: x.mode().map(|m| m.as_str().to_string()),
            automation_type: x.automation_type().map(|t| t.as_str().to_string()),
            current_step: x.current_step_name().filter(|s| !s.is_empty()).map(|s| s.to_string()),
            current_action: x.current_action().filter(|s| !s.is_empty()).map(|s| s.to_string()),
            failure_message: x
                .failure_message()
                .filter(|m| !m.is_empty())
                .map(|m| m.to_string()),
            parent_id: x.parent_automation_execution_id().map(|p| p.to_string()),
            target: x.target().filter(|t| !t.is_empty()).map(|t| t.to_string()),
            targets,
            outputs,
            tags: HashMap::new(),
        }
    }
}

crate::sections! {
    pub enum SsmAutomationDetailSection,
    pub static SSM_AUTOMATION_SECTIONS = [
        Overview "Overview",
        Steps "Steps" => crate::app::App::trigger_ssm_automation_steps_load,
    ]
}

impl Resource for SsmAutomationExecution {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&SSM_AUTOMATION_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.execution_id
    }

    fn name(&self) -> &str {
        &self.document_name
    }

    fn resource_type(&self) -> &str {
        "SSM Automation"
    }

    fn state(&self) -> ResourceState {
        match self.status.to_ascii_lowercase().as_str() {
            "success" | "completedwithsuccess" | "approved" | "exited" => ResourceState::Available,
            "failed" | "timedout" | "completedwithfailure" | "rejected" => {
                ResourceState::Unavailable
            }
            "cancelled" | "cancelling" => ResourceState::Stopped,
            "pending" | "inprogress" | "waiting" | "scheduled" | "runbookinprogress"
            | "pendingapproval" => ResourceState::Pending,
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
        // Include the run's target(s) so an execution is findable by the
        // resource it acted on.
        let target_values: Vec<&str> = self
            .targets
            .iter()
            .flat_map(|(_, vs)| vs.iter().map(|v| v.as_str()))
            .collect();
        format!(
            "{} {} {} {} {} {}",
            self.execution_id,
            self.document_name,
            self.status,
            self.executed_by.clone().unwrap_or_default(),
            self.target.clone().unwrap_or_default(),
            target_values.join(" "),
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        // Fallback only — the rich split pane lives in details_pane.rs.
        vec![
            ("Execution ID".to_string(), self.execution_id.clone()),
            ("Document".to_string(), self.document_name.clone()),
            ("Status".to_string(), self.status.clone()),
            ("Started".to_string(), self.start.clone()),
        ]
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/systems-manager/automation/execution/{}?region={}",
            region, self.execution_id, region
        ))
    }
}

// ── SsmMaintWindow (Maintenance Windows) ──────────────────────────────────────

/// One maintenance window from `DescribeMaintenanceWindows`. Targets, tasks,
/// and execution history load lazily via [`fetch_maint_window_detail`].
#[derive(Debug, Clone)]
pub struct SsmMaintWindow {
    pub window_id: String,
    pub window_name: String,
    pub description: Option<String>,
    pub enabled: bool,
    pub schedule: String,
    pub schedule_timezone: Option<String>,
    pub schedule_offset: Option<i32>,
    pub duration_hours: Option<i32>,
    pub cutoff_hours: i32,
    pub next_execution: Option<String>,
    pub start_date: Option<String>,
    pub end_date: Option<String>,
    pub tags: HashMap<String, String>,
}

impl SsmMaintWindow {
    pub fn from_sdk(w: &aws_sdk_ssm::types::MaintenanceWindowIdentity) -> Self {
        Self {
            window_id: w.window_id().unwrap_or("").to_string(),
            window_name: w.name().unwrap_or("").to_string(),
            description: w.description().filter(|d| !d.is_empty()).map(|d| d.to_string()),
            enabled: w.enabled(),
            schedule: w.schedule().unwrap_or("").to_string(),
            schedule_timezone: w.schedule_timezone().map(|t| t.to_string()),
            schedule_offset: w.schedule_offset(),
            duration_hours: w.duration(),
            cutoff_hours: w.cutoff(),
            next_execution: w
                .next_execution_time()
                .map(|t| t.replace('T', " ")),
            start_date: w.start_date().map(|d| d.to_string()),
            end_date: w.end_date().map(|d| d.to_string()),
            tags: HashMap::new(),
        }
    }
}

crate::sections! {
    pub enum SsmMaintWindowDetailSection,
    pub static SSM_MAINT_WINDOW_SECTIONS = [
        Overview "Overview",
        Targets "Targets" => crate::app::App::trigger_ssm_mw_detail_load,
        Tasks "Tasks" => crate::app::App::trigger_ssm_mw_detail_load,
        History "History" => crate::app::App::trigger_ssm_mw_detail_load,
    ]
}

impl Resource for SsmMaintWindow {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&SSM_MAINT_WINDOW_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.window_id
    }

    fn name(&self) -> &str {
        if self.window_name.is_empty() {
            &self.window_id
        } else {
            &self.window_name
        }
    }

    fn resource_type(&self) -> &str {
        "SSM Maintenance Window"
    }

    fn state(&self) -> ResourceState {
        if self.enabled {
            ResourceState::Available
        } else {
            ResourceState::Stopped
        }
    }
    fn state_label(&self) -> String {
        if self.enabled { "enabled" } else { "disabled" }.to_string()
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!("{} {} {}", self.window_id, self.window_name, self.schedule)
    }

    fn details(&self) -> Vec<(String, String)> {
        // Fallback only — the rich split pane lives in details_pane.rs.
        vec![
            ("Window ID".to_string(), self.window_id.clone()),
            ("Name".to_string(), self.window_name.clone()),
            (
                "Enabled".to_string(),
                if self.enabled { "yes" } else { "no" }.to_string(),
            ),
            ("Schedule".to_string(), self.schedule.clone()),
        ]
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/systems-manager/maintenance-windows/{}/description?region={}",
            region, self.window_id, region
        ))
    }
}

// ── SsmPatchBaseline ──────────────────────────────────────────────────────────

/// One patch baseline from `DescribePatchBaselines`, shown on the Patch
/// sub-tab alongside compliance rows. Approval rules load lazily via
/// [`fetch_patch_baseline_detail`].
#[derive(Debug, Clone)]
pub struct SsmPatchBaseline {
    /// Full id as returned by the API — an ARN for AWS-provided baselines
    /// (which is what `GetPatchBaseline` requires for them), `pb-…` for
    /// custom ones.
    pub baseline_id: String,
    /// Bare `pb-…` id — the resource id, so jumps from a patch state's
    /// `Baseline` row (always bare) resolve for AWS-provided baselines too.
    pub short_id: String,
    pub baseline_name: String,
    pub operating_system: String,
    pub description: Option<String>,
    pub default_baseline: bool,
    pub tags: HashMap<String, String>,
}

impl SsmPatchBaseline {
    pub fn from_sdk(b: &aws_sdk_ssm::types::PatchBaselineIdentity) -> Self {
        let baseline_id = b.baseline_id().unwrap_or("").to_string();
        Self {
            short_id: baseline_id
                .rsplit('/')
                .next()
                .unwrap_or(&baseline_id)
                .to_string(),
            baseline_id,
            baseline_name: b.baseline_name().unwrap_or("").to_string(),
            operating_system: b
                .operating_system()
                .map(|o| o.as_str().to_string())
                .unwrap_or_default(),
            description: b
                .baseline_description()
                .filter(|d| !d.is_empty())
                .map(|d| d.to_string()),
            default_baseline: b.default_baseline(),
            tags: HashMap::new(),
        }
    }

    /// AWS-provided baselines (the `AWS-*` set) — noise next to custom ones.
    pub fn is_aws_provided(&self) -> bool {
        self.baseline_name.starts_with("AWS-")
    }
}

crate::sections! {
    pub enum SsmBaselineDetailSection,
    pub static SSM_BASELINE_SECTIONS = [
        Overview "Overview",
        Rules "Rules" => crate::app::App::trigger_ssm_baseline_detail_load,
    ]
}

impl Resource for SsmPatchBaseline {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&SSM_BASELINE_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.short_id
    }

    fn name(&self) -> &str {
        &self.baseline_name
    }

    fn resource_type(&self) -> &str {
        "SSM Patch Baseline"
    }

    fn state(&self) -> ResourceState {
        ResourceState::stateless()
    }

    fn is_noise(&self) -> bool {
        self.is_aws_provided()
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!(
            "{} {} {} baseline",
            self.baseline_name, self.baseline_id, self.operating_system
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        // Fallback only — the rich split pane lives in details_pane.rs.
        vec![
            ("Baseline ID".to_string(), self.baseline_id.clone()),
            ("Name".to_string(), self.baseline_name.clone()),
            ("Operating System".to_string(), self.operating_system.clone()),
        ]
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn console_url(&self, region: &str) -> Option<String> {
        let id = self
            .baseline_id
            .rsplit('/')
            .next()
            .unwrap_or(&self.baseline_id);
        Some(format!(
            "https://{}.console.aws.amazon.com/systems-manager/patch-manager/baselines/{}?region={}",
            region, id, region
        ))
    }
}

// ── SsmOpsItem (OpsCenter) ────────────────────────────────────────────────────

/// Cap on the OpsItem rows loaded.
pub const MAX_OPS_ITEMS: usize = 200;

/// One OpsCenter item from `DescribeOpsItems`. Description + operational data
/// load lazily via [`fetch_ops_item_detail`].
#[derive(Debug, Clone)]
pub struct SsmOpsItem {
    pub ops_item_id: String,
    pub title: String,
    pub status: String,
    pub severity: Option<String>,
    pub priority: Option<i32>,
    pub source: Option<String>,
    pub category: Option<String>,
    pub ops_item_type: Option<String>,
    pub created_by: Option<String>,
    pub created: String,
    pub last_modified: String,
    pub tags: HashMap<String, String>,
}

impl SsmOpsItem {
    pub fn from_sdk(o: &aws_sdk_ssm::types::OpsItemSummary) -> Self {
        Self {
            ops_item_id: o.ops_item_id().unwrap_or("").to_string(),
            title: o.title().unwrap_or("").to_string(),
            status: o
                .status()
                .map(|s| s.as_str().to_string())
                .unwrap_or_default(),
            severity: o.severity().map(|s| s.to_string()),
            priority: o.priority(),
            source: o.source().map(|s| s.to_string()),
            category: o.category().map(|c| c.to_string()),
            ops_item_type: o.ops_item_type().map(|t| t.to_string()),
            created_by: o.created_by().map(|c| c.to_string()),
            created: fmt_dt(o.created_time()),
            last_modified: fmt_dt(o.last_modified_time()),
            tags: HashMap::new(),
        }
    }
}

crate::sections! {
    pub enum SsmOpsItemDetailSection,
    pub static SSM_OPS_ITEM_SECTIONS = [
        Overview "Overview",
        Detail "Detail" => crate::app::App::trigger_ssm_ops_item_detail_load,
    ]
}

impl Resource for SsmOpsItem {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&SSM_OPS_ITEM_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.ops_item_id
    }

    fn name(&self) -> &str {
        if self.title.is_empty() {
            &self.ops_item_id
        } else {
            &self.title
        }
    }

    fn resource_type(&self) -> &str {
        "SSM OpsItem"
    }

    fn state(&self) -> ResourceState {
        match self.status.to_ascii_lowercase().as_str() {
            "resolved" | "closed" | "completedwithsuccess" | "approved" => {
                ResourceState::Available
            }
            "failed" | "timedout" | "completedwithfailure" | "rejected" | "cancelled" => {
                ResourceState::Unavailable
            }
            // Open items are the ones needing attention.
            _ => ResourceState::Pending,
        }
    }
    fn state_label(&self) -> String {
        native_state_label(&self.status, || self.state())
    }

    fn is_noise(&self) -> bool {
        // Hide resolved/closed items so open ones stand out (`a` toggles).
        matches!(
            self.status.to_ascii_lowercase().as_str(),
            "resolved" | "closed" | "completedwithsuccess"
        )
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!(
            "{} {} {} {} {}",
            self.ops_item_id,
            self.title,
            self.status,
            self.source.clone().unwrap_or_default(),
            self.severity.clone().unwrap_or_default(),
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        // Fallback only — the rich split pane lives in details_pane.rs.
        vec![
            ("OpsItem ID".to_string(), self.ops_item_id.clone()),
            ("Title".to_string(), self.title.clone()),
            ("Status".to_string(), self.status.clone()),
            ("Created".to_string(), self.created.clone()),
        ]
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/systems-manager/opsitems/{}?region={}",
            region, self.ops_item_id, region
        ))
    }
}

// ── SsmSession (Session Manager) ──────────────────────────────────────────────

/// Cap on the terminated-session history rows loaded (active sessions are
/// never capped).
pub const MAX_SESSION_HISTORY: usize = 100;

/// One Session Manager session from `DescribeSessions` (Active or History).
/// Flat `details()` — complete as flat.
#[derive(Debug, Clone)]
pub struct SsmSession {
    pub session_id: String,
    pub target: String,
    pub status: String,
    pub owner: Option<String>,
    pub start: String,
    pub end: String,
    pub document_name: Option<String>,
    pub reason: Option<String>,
    pub access_type: Option<String>,
    pub max_duration_min: Option<String>,
    pub output_url: Option<String>,
    pub tags: HashMap<String, String>,
}

impl SsmSession {
    pub fn from_sdk(s: &aws_sdk_ssm::types::Session) -> Self {
        Self {
            session_id: s.session_id().unwrap_or("").to_string(),
            target: s.target().unwrap_or("").to_string(),
            status: s
                .status()
                .map(|st| st.as_str().to_string())
                .unwrap_or_default(),
            owner: s.owner().map(|o| o.to_string()),
            start: fmt_dt(s.start_date()),
            end: fmt_dt(s.end_date()),
            document_name: s.document_name().map(|d| d.to_string()),
            reason: s.reason().filter(|r| !r.is_empty()).map(|r| r.to_string()),
            access_type: s.access_type().map(|a| a.as_str().to_string()),
            max_duration_min: s.max_session_duration().map(|m| m.to_string()),
            output_url: s
                .output_url()
                .and_then(|u| u.s3_output_url())
                .filter(|u| !u.is_empty())
                .map(|u| u.to_string()),
            tags: HashMap::new(),
        }
    }
}

impl Resource for SsmSession {
    fn id(&self) -> &str {
        &self.session_id
    }

    fn name(&self) -> &str {
        &self.target
    }

    fn resource_type(&self) -> &str {
        "SSM Session"
    }

    fn state(&self) -> ResourceState {
        match self.status.to_ascii_lowercase().as_str() {
            "connected" => ResourceState::Running,
            "connecting" => ResourceState::Pending,
            "disconnected" => ResourceState::Unknown("disconnected".to_string()),
            "failed" => ResourceState::Unavailable,
            "terminated" | "terminating" => ResourceState::Stopped,
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
            self.session_id,
            self.target,
            self.status,
            self.owner.clone().unwrap_or_default(),
            self.document_name.clone().unwrap_or_default(),
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        let mut rows = vec![
            ("Session ID".to_string(), self.session_id.clone()),
            ("Target".to_string(), self.target.clone()),
            ("Status".to_string(), self.status.clone()),
        ];
        if let Some(o) = &self.owner {
            rows.push(("Owner".to_string(), o.clone()));
        }
        if let Some(a) = &self.access_type {
            rows.push(("Access Type".to_string(), a.clone()));
        }
        if !self.start.is_empty() {
            rows.push(("Started".to_string(), self.start.clone()));
        }
        if !self.end.is_empty() {
            rows.push(("Ended".to_string(), self.end.clone()));
        }
        if let Some(d) = &self.document_name {
            rows.push(("Document".to_string(), d.clone()));
        }
        if let Some(r) = &self.reason {
            rows.push(("Reason".to_string(), r.clone()));
        }
        if let Some(m) = &self.max_duration_min {
            rows.push(("Max Duration".to_string(), format!("{} min", m)));
        }
        if let Some(u) = &self.output_url {
            rows.push(("Output S3".to_string(), u.clone()));
        }
        rows
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/systems-manager/session-manager/{}?region={}",
            region, self.session_id, region
        ))
    }
}

// ── Parameter detail: version history + tags (lazy) ───────────────────────────

/// One version from a parameter's history. For non-`SecureString` parameters the
/// plaintext `value` is included (matching the console); `SecureString` values
/// are deliberately omitted — the current value is only ever revealed via the
/// opt-in `x` action, never cached.
#[derive(Debug, Clone)]
pub struct SsmParamVersion {
    pub version: i64,
    pub last_modified: String,
    pub user: Option<String>,
    pub param_type: String,
    pub description: Option<String>,
    pub labels: Vec<String>,
    pub value: Option<String>,
    pub changed: bool,
}

/// `(history, tags)` payload of `LazyStore::ssm_param_details` — one fetch
/// backs both the History and Tags sections.
pub type SsmParamDetail = (Vec<SsmParamVersion>, Vec<(String, String)>);

/// Cap on how much of a non-secure history value we surface, to keep the pane
/// readable (and avoid holding large blobs).
const MAX_HISTORY_VALUE_LEN: usize = 240;

/// Fetch a parameter's version history (`GetParameterHistory`, no decryption)
/// and tags (`ListTagsForResource`) together. `is_secure` suppresses the
/// plaintext value column for `SecureString` parameters.
pub async fn fetch_ssm_param_detail(
    ssm: SsmClient,
    name: String,
    is_secure: bool,
) -> Result<(Vec<SsmParamVersion>, Vec<(String, String)>)> {
    // ── history ──
    let mut raw: Vec<(i64, SsmParamVersion)> = Vec::new();
    let mut paginator = ssm
        .get_parameter_history()
        .name(&name)
        .with_decryption(false)
        .into_paginator()
        .send();
    while let Some(page) = paginator.next().await {
        let page = page.map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
        for p in page.parameters() {
            let version = p.version();
            let value = if is_secure {
                None
            } else {
                p.value().map(|v| {
                    if v.len() > MAX_HISTORY_VALUE_LEN {
                        format!("{}… ({} bytes)", &v[..MAX_HISTORY_VALUE_LEN], v.len())
                    } else {
                        v.to_string()
                    }
                })
            };
            raw.push((
                version,
                SsmParamVersion {
                    version,
                    last_modified: fmt_dt(p.last_modified_date()),
                    user: p.last_modified_user().map(|u| u.to_string()),
                    param_type: p
                        .r#type()
                        .map(|t| t.as_str().to_string())
                        .unwrap_or_default(),
                    description: p.description().map(|d| d.to_string()),
                    labels: p.labels().iter().map(|l| l.to_string()).collect(),
                    value,
                    changed: false,
                },
            ));
        }
    }
    // Compute "changed vs previous" in ascending version order, then present
    // newest-first.
    raw.sort_by_key(|(v, _)| *v);
    let mut prev: Option<String> = None;
    for (_, ver) in raw.iter_mut() {
        if let Some(val) = &ver.value {
            ver.changed = prev.as_deref() != Some(val.as_str());
            prev = Some(val.clone());
        }
    }
    let history: Vec<SsmParamVersion> = raw.into_iter().rev().map(|(_, v)| v).collect();

    // ── tags ──
    let tag_resp = ssm
        .list_tags_for_resource()
        .resource_type(aws_sdk_ssm::types::ResourceTypeForTagging::Parameter)
        .resource_id(&name)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
    let mut tags: Vec<(String, String)> = tag_resp
        .tag_list()
        .iter()
        .map(|t| (t.key().to_string(), t.value().to_string()))
        .collect();
    tags.sort_by(|a, b| a.0.cmp(&b.0));

    Ok((history, tags))
}

// ── Document content (lazy) ───────────────────────────────────────────────────

/// `(content, format)` payload of `LazyStore::ssm_doc_contents`.
pub type SsmDocContent = (String, String);

/// Fetch a document's body (`GetDocument`). Returns the content and its format
/// (JSON/YAML/TEXT) for syntax-appropriate viewing/editing.
pub async fn fetch_document_content(ssm: SsmClient, name: String) -> Result<(String, String)> {
    let resp = ssm
        .get_document()
        .name(&name)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
    let content = resp.content().unwrap_or("").to_string();
    let format = resp
        .document_format()
        .map(|f| f.as_str().to_string())
        .unwrap_or_else(|| "JSON".to_string());
    Ok((content, format))
}

// ── On-demand value reveal (opt-in `x`; value never cached) ───────────────────

/// Pretty-print a value if it parses as JSON, otherwise return it unchanged.
fn maybe_pretty(value: &str) -> String {
    serde_json::from_str::<serde_json::Value>(value)
        .ok()
        .and_then(|v| serde_json::to_string_pretty(&v).ok())
        .unwrap_or_else(|| value.to_string())
}

/// Fetch a parameter's value on demand (decrypted for SecureString). When
/// `pretty` is true, JSON values are pretty-printed for display; when false,
/// the exact value is returned (used for clipboard copy). Returns the value to
/// be used transiently — callers must not cache it.
pub async fn fetch_parameter_value(ssm: SsmClient, name: String, pretty: bool) -> Result<String> {
    let resp = ssm
        .get_parameter()
        .name(&name)
        .with_decryption(true)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;

    let value = resp
        .parameter()
        .and_then(|p| p.value())
        .unwrap_or("<empty parameter>");

    Ok(if pretty { maybe_pretty(value) } else { value.to_string() })
}

// ── Association detail: execution history + tags (lazy) ──────────────────────

/// One run of an association from `DescribeAssociationExecutions`.
#[derive(Debug, Clone)]
pub struct SsmAssocExecution {
    pub execution_id: String,
    pub status: String,
    pub detailed_status: Option<String>,
    pub created: String,
    /// Per-status resource counts, prettified from the API's
    /// `{Success=9,Failed=1}` string.
    pub resource_counts: Option<String>,
}

/// Cap on the execution history surfaced in the pane (newest first).
pub const MAX_ASSOC_EXECUTIONS: usize = 20;

/// `(execution history, tags)` payload of `LazyStore::ssm_assoc_details` —
/// one fetch backs both the Executions and Tags sections.
pub type SsmAssocDetail = (Vec<SsmAssocExecution>, Vec<(String, String)>);

/// Prettify the API's `{Success=9,Failed=1}` resource-count string.
fn pretty_resource_counts(raw: &str) -> String {
    raw.trim_matches(|c| c == '{' || c == '}')
        .split(',')
        .map(|part| part.trim().replace('=', " "))
        .filter(|p| !p.is_empty())
        .map(|p| {
            // "Success 9" reads better as "9 Success".
            match p.split_once(' ') {
                Some((status, n)) => format!("{} {}", n, status),
                None => p,
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Fetch an association's execution history (`DescribeAssociationExecutions`,
/// newest first, capped at [`MAX_ASSOC_EXECUTIONS`]) and tags
/// (`ListTagsForResource`) together.
pub async fn fetch_ssm_assoc_detail(
    ssm: SsmClient,
    association_id: String,
) -> Result<(Vec<SsmAssocExecution>, Vec<(String, String)>)> {
    // ── executions ──
    let mut executions: Vec<SsmAssocExecution> = Vec::new();
    let mut paginator = ssm
        .describe_association_executions()
        .association_id(&association_id)
        .into_paginator()
        .send();
    while let Some(page) = paginator.next().await {
        let page =
            page.map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
        for x in page.association_executions() {
            executions.push(SsmAssocExecution {
                execution_id: x.execution_id().unwrap_or("").to_string(),
                status: x.status().unwrap_or("").to_string(),
                detailed_status: x.detailed_status().map(|s| s.to_string()),
                created: fmt_dt(x.created_time()),
                resource_counts: x
                    .resource_count_by_status()
                    .map(pretty_resource_counts)
                    .filter(|s| !s.is_empty()),
            });
        }
        if executions.len() >= MAX_ASSOC_EXECUTIONS {
            break;
        }
    }
    // Newest first; `fmt_dt` output sorts lexicographically.
    executions.sort_by(|a, b| b.created.cmp(&a.created));
    executions.truncate(MAX_ASSOC_EXECUTIONS);

    // ── tags ──
    let tag_resp = ssm
        .list_tags_for_resource()
        .resource_type(aws_sdk_ssm::types::ResourceTypeForTagging::Association)
        .resource_id(&association_id)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
    let mut tags: Vec<(String, String)> = tag_resp
        .tag_list()
        .iter()
        .map(|t| (t.key().to_string(), t.value().to_string()))
        .collect();
    tags.sort_by(|a, b| a.0.cmp(&b.0));

    Ok((executions, tags))
}

// ── Per-instance association status (lazy Fleet section) ─────────────────────

/// One association's status on a specific managed instance, from
/// `DescribeInstanceAssociationsStatus`.
#[derive(Debug, Clone)]
pub struct SsmInstanceAssoc {
    pub association_id: String,
    pub doc_name: String,
    pub association_name: Option<String>,
    pub status: String,
    pub detailed_status: Option<String>,
    pub execution_date: String,
    pub execution_summary: Option<String>,
    pub error_code: Option<String>,
}

/// Fetch every association's status on one managed instance
/// (`DescribeInstanceAssociationsStatus`).
pub async fn fetch_instance_associations(
    ssm: SsmClient,
    instance_id: String,
) -> Result<Vec<SsmInstanceAssoc>> {
    let mut out: Vec<SsmInstanceAssoc> = Vec::new();
    let mut paginator = ssm
        .describe_instance_associations_status()
        .instance_id(&instance_id)
        .into_paginator()
        .send();
    while let Some(page) = paginator.next().await {
        let page =
            page.map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
        for i in page.instance_association_status_infos() {
            out.push(SsmInstanceAssoc {
                association_id: i.association_id().unwrap_or("").to_string(),
                doc_name: i.name().unwrap_or("").to_string(),
                association_name: i.association_name().map(|n| n.to_string()),
                status: i.status().unwrap_or("").to_string(),
                detailed_status: i.detailed_status().map(|s| s.to_string()),
                execution_date: fmt_dt(i.execution_date()),
                execution_summary: i.execution_summary().map(|s| s.to_string()),
                error_code: i.error_code().filter(|c| !c.is_empty()).map(|c| c.to_string()),
            });
        }
    }
    Ok(out)
}

// ── Per-instance inventory (lazy Fleet section) ───────────────────────────────

/// One installed application from the `AWS:Application` inventory type.
#[derive(Debug, Clone, PartialEq)]
pub struct SsmInventoryApp {
    pub name: String,
    pub version: String,
    pub publisher: String,
}

/// A managed instance's software inventory: installed applications (capped for
/// display, with the true total) plus the `AWS:InstanceDetailedInformation`
/// record (CPU/OS details) when present.
#[derive(Debug, Clone, Default)]
pub struct SsmInventoryData {
    pub apps: Vec<SsmInventoryApp>,
    pub total_apps: usize,
    pub capture_time: Option<String>,
    /// Prettified key-value rows from `AWS:InstanceDetailedInformation`.
    pub detail: Vec<(String, String)>,
}

/// Cap on the installed-application rows surfaced in the pane.
pub const MAX_INVENTORY_APPS: usize = 100;

fn parse_app_entry(map: &HashMap<String, String>) -> SsmInventoryApp {
    let get = |k: &str| map.get(k).cloned().unwrap_or_default();
    SsmInventoryApp {
        name: get("Name"),
        version: get("Version"),
        publisher: get("Publisher"),
    }
}

/// Prettify the `AWS:InstanceDetailedInformation` entry into display rows.
fn parse_detailed_info(map: &HashMap<String, String>) -> Vec<(String, String)> {
    // (inventory key, display label) — fixed order, skipping empty values.
    const LABELS: [(&str, &str); 6] = [
        ("CPUModel", "CPU Model"),
        ("CPUs", "CPUs"),
        ("CPUCores", "CPU Cores"),
        ("CPUSpeedMHz", "CPU Speed (MHz)"),
        ("CPUHyperThreadEnabled", "Hyper-Threading"),
        ("OSServicePack", "OS Service Pack"),
    ];
    LABELS
        .iter()
        .filter_map(|(key, label)| {
            map.get(*key)
                .filter(|v| !v.is_empty())
                .map(|v| (label.to_string(), v.clone()))
        })
        .collect()
}

/// Fetch a managed instance's software inventory: every `AWS:Application`
/// entry (`ListInventoryEntries` — no fluent paginator, hand-rolled token
/// loop), capped at [`MAX_INVENTORY_APPS`] for display, plus the
/// best-effort `AWS:InstanceDetailedInformation` record.
pub async fn fetch_instance_inventory(
    ssm: SsmClient,
    instance_id: String,
) -> Result<SsmInventoryData> {
    let mut data = SsmInventoryData::default();
    let mut token: Option<String> = None;
    loop {
        let mut req = ssm
            .list_inventory_entries()
            .instance_id(&instance_id)
            .type_name("AWS:Application");
        if let Some(t) = &token {
            req = req.next_token(t);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
        if data.capture_time.is_none() {
            data.capture_time = resp.capture_time().map(|t| t.replace('T', " "));
        }
        for entry in resp.entries() {
            data.total_apps += 1;
            if data.apps.len() < MAX_INVENTORY_APPS {
                data.apps.push(parse_app_entry(entry));
            }
        }
        token = crate::aws::pagination::next_page_token(resp.next_token(), &token);
        if token.is_none() {
            break;
        }
    }
    data.apps
        .sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

    // Best-effort CPU/OS record — same permission, but tolerate a miss.
    if let Ok(resp) = ssm
        .list_inventory_entries()
        .instance_id(&instance_id)
        .type_name("AWS:InstanceDetailedInformation")
        .send()
        .await
    {
        if let Some(entry) = resp.entries().first() {
            data.detail = parse_detailed_info(entry);
        }
    }

    Ok(data)
}

// ── Per-instance patch detail (lazy Fleet section) ────────────────────────────

/// The instance's Patch Manager rollup from `DescribeInstancePatchStates`.
#[derive(Debug, Clone)]
pub struct SsmPatchState {
    pub baseline_id: String,
    pub patch_group: String,
    pub operation: String,
    pub operation_end: String,
    pub reboot_option: Option<String>,
    pub installed: i32,
    pub installed_other: i32,
    pub installed_pending_reboot: i32,
    pub installed_rejected: i32,
    pub missing: i32,
    pub failed: i32,
    pub not_applicable: i32,
    pub critical_non_compliant: Option<i32>,
    pub security_non_compliant: Option<i32>,
}

impl SsmPatchState {
    fn from_sdk(s: &aws_sdk_ssm::types::InstancePatchState) -> Self {
        Self {
            baseline_id: s.baseline_id().to_string(),
            patch_group: s.patch_group().to_string(),
            operation: s.operation().as_str().to_string(),
            operation_end: {
                let t = s.operation_end_time().to_string();
                t.split('.').next().unwrap_or(&t).replace('T', " ")
            },
            reboot_option: s.reboot_option().map(|r| r.as_str().to_string()),
            installed: s.installed_count(),
            installed_other: s.installed_other_count(),
            installed_pending_reboot: s.installed_pending_reboot_count().unwrap_or(0),
            installed_rejected: s.installed_rejected_count().unwrap_or(0),
            missing: s.missing_count(),
            failed: s.failed_count(),
            not_applicable: s.not_applicable_count(),
            critical_non_compliant: s.critical_non_compliant_count(),
            security_non_compliant: s.security_non_compliant_count(),
        }
    }
}

/// One actionable patch from `DescribeInstancePatches`.
#[derive(Debug, Clone)]
pub struct SsmPatchItem {
    pub title: String,
    pub kb_id: String,
    pub classification: String,
    pub severity: String,
    pub state: String,
    pub cve_ids: Option<String>,
}

/// Patch states worth listing — everything installed-and-fine or
/// not-applicable is noise next to the rollup counts.
fn is_actionable_patch(state: &str) -> bool {
    matches!(
        state,
        "MISSING"
            | "FAILED"
            | "INSTALLED_PENDING_REBOOT"
            | "INSTALLED_REJECTED"
            | "AVAILABLE_SECURITY_UPDATE"
    )
}

/// `(rollup, actionable patches, truncated)` payload of
/// `LazyStore::ssm_instance_patches`.
pub type SsmInstancePatches = (Option<SsmPatchState>, Vec<SsmPatchItem>, bool);

/// Cap on the actionable-patch rows surfaced in the pane.
pub const MAX_INSTANCE_PATCH_ROWS: usize = 100;

/// Fetch an instance's Patch Manager state: the rollup
/// (`DescribeInstancePatchStates`) and, when the instance has one, the
/// actionable patch list (`DescribeInstancePatches`, filtered client-side to
/// missing/failed/pending-reboot states, capped at
/// [`MAX_INSTANCE_PATCH_ROWS`]).
pub async fn fetch_instance_patches(
    ssm: SsmClient,
    instance_id: String,
) -> Result<SsmInstancePatches> {
    let resp = ssm
        .describe_instance_patch_states()
        .instance_ids(&instance_id)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
    let state = resp
        .instance_patch_states()
        .first()
        .map(SsmPatchState::from_sdk);
    // Never patched by Patch Manager — no compliance data to list.
    if state.is_none() {
        return Ok((None, Vec::new(), false));
    }

    let mut patches: Vec<SsmPatchItem> = Vec::new();
    let mut truncated = false;
    let mut paginator = ssm
        .describe_instance_patches()
        .instance_id(&instance_id)
        .into_paginator()
        .send();
    'pages: while let Some(page) = paginator.next().await {
        let page =
            page.map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
        for p in page.patches() {
            if !is_actionable_patch(p.state().as_str()) {
                continue;
            }
            if patches.len() >= MAX_INSTANCE_PATCH_ROWS {
                truncated = true;
                break 'pages;
            }
            patches.push(SsmPatchItem {
                title: p.title().to_string(),
                kb_id: p.kb_id().to_string(),
                classification: p.classification().to_string(),
                severity: p.severity().to_string(),
                state: p.state().as_str().to_string(),
                cve_ids: p.cve_ids().filter(|c| !c.is_empty()).map(|c| c.to_string()),
            });
        }
    }
    // Severity-ish ordering: failed first, then missing, then pending reboot.
    let rank = |s: &str| match s {
        "FAILED" => 0,
        "MISSING" => 1,
        "AVAILABLE_SECURITY_UPDATE" => 2,
        "INSTALLED_PENDING_REBOOT" => 3,
        _ => 4,
    };
    patches.sort_by(|a, b| rank(&a.state).cmp(&rank(&b.state)).then(a.title.cmp(&b.title)));

    Ok((state, patches, truncated))
}

// ── Command invocations (lazy Run Command section) ────────────────────────────

/// Truncate to at most `n` characters on a char boundary (byte slicing can
/// panic mid-UTF-8).
fn truncate_chars(s: &str, n: usize) -> String {
    match s.char_indices().nth(n) {
        Some((idx, _)) => format!("{}… ({} chars)", &s[..idx], s.chars().count()),
        None => s.to_string(),
    }
}

/// Cap on the per-plugin output snippet surfaced inline.
const MAX_INVOCATION_OUTPUT_LEN: usize = 400;

/// One per-instance invocation of a command, with its plugin output snippet.
#[derive(Debug, Clone)]
pub struct SsmCmdInvocation {
    pub instance_id: String,
    pub instance_name: Option<String>,
    pub status: String,
    pub status_details: Option<String>,
    pub requested: String,
    /// `(plugin name, status, response code, output snippet)` per plugin.
    pub plugins: Vec<(String, String, i32, Option<String>)>,
}

/// Fetch a command's per-instance invocations (`ListCommandInvocations` with
/// `details=true` so each plugin carries its output, truncated for display).
pub async fn fetch_command_invocations(
    ssm: SsmClient,
    command_id: String,
) -> Result<Vec<SsmCmdInvocation>> {
    let mut out: Vec<SsmCmdInvocation> = Vec::new();
    let mut paginator = ssm
        .list_command_invocations()
        .command_id(&command_id)
        .details(true)
        .into_paginator()
        .send();
    while let Some(page) = paginator.next().await {
        let page =
            page.map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
        for i in page.command_invocations() {
            let plugins = i
                .command_plugins()
                .iter()
                .map(|p| {
                    (
                        p.name().unwrap_or("").to_string(),
                        p.status()
                            .map(|s| s.as_str().to_string())
                            .unwrap_or_default(),
                        p.response_code(),
                        p.output()
                            .map(str::trim)
                            .filter(|o| !o.is_empty())
                            .map(|o| truncate_chars(o, MAX_INVOCATION_OUTPUT_LEN)),
                    )
                })
                .collect();
            out.push(SsmCmdInvocation {
                instance_id: i.instance_id().unwrap_or("").to_string(),
                instance_name: i
                    .instance_name()
                    .filter(|n| !n.is_empty())
                    .map(|n| n.to_string()),
                status: i
                    .status()
                    .map(|s| s.as_str().to_string())
                    .unwrap_or_default(),
                status_details: i
                    .status_details()
                    .filter(|d| !d.is_empty())
                    .map(|d| d.to_string()),
                requested: fmt_dt(i.requested_date_time()),
                plugins,
            });
        }
    }
    Ok(out)
}

// ── Automation steps (lazy Automation section) ────────────────────────────────

/// One step of an automation run from `DescribeAutomationStepExecutions`.
#[derive(Debug, Clone)]
pub struct SsmAutomationStep {
    pub name: String,
    pub action: String,
    pub status: String,
    pub start: String,
    pub duration_secs: Option<i64>,
    pub failure_message: Option<String>,
}

/// Fetch an automation execution's steps in run order
/// (`DescribeAutomationStepExecutions`).
pub async fn fetch_automation_steps(
    ssm: SsmClient,
    execution_id: String,
) -> Result<Vec<SsmAutomationStep>> {
    let mut out: Vec<SsmAutomationStep> = Vec::new();
    let mut paginator = ssm
        .describe_automation_step_executions()
        .automation_execution_id(&execution_id)
        .into_paginator()
        .send();
    while let Some(page) = paginator.next().await {
        let page =
            page.map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
        for s in page.step_executions() {
            let duration_secs = match (s.execution_start_time(), s.execution_end_time()) {
                (Some(a), Some(b)) => Some(b.secs() - a.secs()),
                _ => None,
            };
            out.push(SsmAutomationStep {
                name: s.step_name().unwrap_or("").to_string(),
                action: s.action().unwrap_or("").to_string(),
                status: s
                    .step_status()
                    .map(|st| st.as_str().to_string())
                    .unwrap_or_default(),
                start: fmt_dt(s.execution_start_time()),
                duration_secs,
                failure_message: s
                    .failure_message()
                    .filter(|m| !m.is_empty())
                    .map(|m| m.to_string()),
            });
        }
    }
    Ok(out)
}

// ── Maintenance window detail: targets + tasks + history (lazy) ───────────────

/// One registered target group of a maintenance window.
#[derive(Debug, Clone)]
pub struct SsmMwTarget {
    pub name: Option<String>,
    pub resource_type: String,
    pub targets: Vec<(String, Vec<String>)>,
}

/// One registered task of a maintenance window.
#[derive(Debug, Clone)]
pub struct SsmMwTask {
    pub name: Option<String>,
    pub task_type: String,
    pub task_arn: String,
    pub priority: i32,
    pub max_concurrency: Option<String>,
    pub max_errors: Option<String>,
    pub cutoff_behavior: Option<String>,
}

/// One past run of a maintenance window.
#[derive(Debug, Clone)]
pub struct SsmMwExecution {
    pub execution_id: String,
    pub status: String,
    pub status_details: Option<String>,
    pub start: String,
    pub end: String,
}

/// `(targets, tasks, executions)` payload of `LazyStore::ssm_mw_details` —
/// one fetch backs the Targets, Tasks, and History sections.
pub type SsmMaintWindowDetail = (Vec<SsmMwTarget>, Vec<SsmMwTask>, Vec<SsmMwExecution>);

/// Cap on the execution-history rows surfaced in the pane (newest first).
pub const MAX_MW_EXECUTIONS: usize = 10;

/// Fetch a maintenance window's registered targets
/// (`DescribeMaintenanceWindowTargets`), tasks
/// (`DescribeMaintenanceWindowTasks`, priority order), and recent execution
/// history (`DescribeMaintenanceWindowExecutions`, newest first, capped at
/// [`MAX_MW_EXECUTIONS`]) together.
pub async fn fetch_maint_window_detail(
    ssm: SsmClient,
    window_id: String,
) -> Result<SsmMaintWindowDetail> {
    // ── targets ──
    let mut targets: Vec<SsmMwTarget> = Vec::new();
    let mut t_pag = ssm
        .describe_maintenance_window_targets()
        .window_id(&window_id)
        .into_paginator()
        .send();
    while let Some(page) = t_pag.next().await {
        let page = page
            .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
        for t in page.targets() {
            targets.push(SsmMwTarget {
                name: t.name().filter(|n| !n.is_empty()).map(|n| n.to_string()),
                resource_type: t
                    .resource_type()
                    .map(|r| r.as_str().to_string())
                    .unwrap_or_default(),
                targets: t
                    .targets()
                    .iter()
                    .map(|x| {
                        (
                            x.key().unwrap_or("").to_string(),
                            x.values().iter().map(|v| v.to_string()).collect(),
                        )
                    })
                    .collect(),
            });
        }
    }
    // ── tasks (priority order) ──
    let mut tasks: Vec<SsmMwTask> = Vec::new();
    let mut k_pag = ssm
        .describe_maintenance_window_tasks()
        .window_id(&window_id)
        .into_paginator()
        .send();
    while let Some(page) = k_pag.next().await {
        let page = page
            .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
        for t in page.tasks() {
            tasks.push(SsmMwTask {
                name: t.name().filter(|n| !n.is_empty()).map(|n| n.to_string()),
                task_type: t
                    .r#type()
                    .map(|y| y.as_str().to_string())
                    .unwrap_or_default(),
                task_arn: t.task_arn().unwrap_or("").to_string(),
                priority: t.priority(),
                max_concurrency: t.max_concurrency().map(|m| m.to_string()),
                max_errors: t.max_errors().map(|m| m.to_string()),
                cutoff_behavior: t.cutoff_behavior().map(|c| c.as_str().to_string()),
            });
        }
    }
    tasks.sort_by_key(|t| t.priority);

    // ── recent executions ──
    let mut executions: Vec<SsmMwExecution> = Vec::new();
    let mut x_pag = ssm
        .describe_maintenance_window_executions()
        .window_id(&window_id)
        .into_paginator()
        .send();
    while let Some(page) = x_pag.next().await {
        let page = page
            .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
        for x in page.window_executions() {
            executions.push(SsmMwExecution {
                execution_id: x.window_execution_id().unwrap_or("").to_string(),
                status: x
                    .status()
                    .map(|s| s.as_str().to_string())
                    .unwrap_or_default(),
                status_details: x
                    .status_details()
                    .filter(|d| !d.is_empty())
                    .map(|d| d.to_string()),
                start: fmt_dt(x.start_time()),
                end: fmt_dt(x.end_time()),
            });
        }
        if executions.len() >= MAX_MW_EXECUTIONS {
            break;
        }
    }
    executions.sort_by(|a, b| b.start.cmp(&a.start));
    executions.truncate(MAX_MW_EXECUTIONS);

    Ok((targets, tasks, executions))
}

// ── Patch baseline detail: approval rules + groups (lazy) ─────────────────────

/// One approval rule of a patch baseline, pre-flattened for display.
#[derive(Debug, Clone)]
pub struct SsmPatchRule {
    /// `(filter key, values)` rows, e.g. `("CLASSIFICATION", "SecurityUpdates")`.
    pub filters: Vec<(String, String)>,
    pub approve_after_days: Option<i32>,
    pub approve_until_date: Option<String>,
    pub compliance_level: Option<String>,
    pub include_non_security: bool,
}

/// A patch baseline's full ruleset from `GetPatchBaseline`.
#[derive(Debug, Clone, Default)]
pub struct SsmBaselineDetail {
    pub approval_rules: Vec<SsmPatchRule>,
    pub global_filters: Vec<(String, String)>,
    pub approved_patches: Vec<String>,
    pub approved_compliance_level: Option<String>,
    pub rejected_patches: Vec<String>,
    pub rejected_action: Option<String>,
    pub patch_groups: Vec<String>,
    pub sources: Vec<String>,
    pub created: String,
    pub modified: String,
}

fn flatten_filter_group(
    g: Option<&aws_sdk_ssm::types::PatchFilterGroup>,
) -> Vec<(String, String)> {
    g.map(|g| {
        g.patch_filters()
            .iter()
            .map(|f| {
                (
                    f.key().as_str().to_string(),
                    f.values().join(", "),
                )
            })
            .collect()
    })
    .unwrap_or_default()
}

/// Fetch a patch baseline's approval rules, explicit approve/reject lists,
/// patch groups, and sources (`GetPatchBaseline`).
pub async fn fetch_patch_baseline_detail(
    ssm: SsmClient,
    baseline_id: String,
) -> Result<SsmBaselineDetail> {
    let resp = ssm
        .get_patch_baseline()
        .baseline_id(&baseline_id)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
    let approval_rules = resp
        .approval_rules()
        .map(|rg| {
            rg.patch_rules()
                .iter()
                .map(|r| SsmPatchRule {
                    filters: flatten_filter_group(r.patch_filter_group()),
                    approve_after_days: r.approve_after_days(),
                    approve_until_date: r.approve_until_date().map(|d| d.to_string()),
                    compliance_level: r.compliance_level().map(|c| c.as_str().to_string()),
                    include_non_security: r.enable_non_security().unwrap_or(false),
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(SsmBaselineDetail {
        approval_rules,
        global_filters: flatten_filter_group(resp.global_filters()),
        approved_patches: resp
            .approved_patches()
            .iter()
            .map(|p| p.to_string())
            .collect(),
        approved_compliance_level: resp
            .approved_patches_compliance_level()
            .map(|c| c.as_str().to_string()),
        rejected_patches: resp
            .rejected_patches()
            .iter()
            .map(|p| p.to_string())
            .collect(),
        rejected_action: resp.rejected_patches_action().map(|a| a.as_str().to_string()),
        patch_groups: resp.patch_groups().iter().map(|g| g.to_string()).collect(),
        sources: resp
            .sources()
            .iter()
            .map(|s| s.name().to_string())
            .collect(),
        created: fmt_dt(resp.created_date()),
        modified: fmt_dt(resp.modified_date()),
    })
}

// ── OpsItem detail: description + operational data (lazy) ─────────────────────

/// An OpsItem's full body from `GetOpsItem`.
#[derive(Debug, Clone, Default)]
pub struct SsmOpsItemDetail {
    pub description: String,
    /// Related resource ARNs extracted from the `/aws/resources` operational
    /// data (rendered as key-value rows so they jump-classify).
    pub related_resources: Vec<String>,
    /// Remaining operational data as `(key, value)`, sorted by key;
    /// `/aws/dedup` (an internal dedup blob) is skipped.
    pub operational_data: Vec<(String, String)>,
    pub related_ops_items: Vec<String>,
    pub notifications: Vec<String>,
}

/// Pull every `"arn": "…"` value out of the `/aws/resources` JSON blob.
fn extract_resource_arns(json: &str) -> Vec<String> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    fn walk(v: &serde_json::Value, out: &mut Vec<String>) {
        match v {
            serde_json::Value::Object(m) => {
                for (k, val) in m {
                    if k == "arn" {
                        if let Some(s) = val.as_str() {
                            out.push(s.to_string());
                        }
                    } else {
                        walk(val, out);
                    }
                }
            }
            serde_json::Value::Array(a) => a.iter().for_each(|x| walk(x, out)),
            _ => {}
        }
    }
    walk(&v, &mut out);
    out
}

/// Fetch an OpsItem's description, operational data (related-resource ARNs
/// split out), related OpsItems, and notification topics (`GetOpsItem`).
pub async fn fetch_ops_item_detail(
    ssm: SsmClient,
    ops_item_id: String,
) -> Result<SsmOpsItemDetail> {
    let resp = ssm
        .get_ops_item()
        .ops_item_id(&ops_item_id)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
    let Some(item) = resp.ops_item() else {
        return Ok(SsmOpsItemDetail::default());
    };
    let mut detail = SsmOpsItemDetail {
        description: item.description().unwrap_or("").to_string(),
        ..Default::default()
    };
    if let Some(data) = item.operational_data() {
        let mut rows: Vec<(String, String)> = Vec::new();
        for (k, v) in data {
            let value = v.value().unwrap_or("").to_string();
            if k == "/aws/resources" {
                detail.related_resources = extract_resource_arns(&value);
            } else if k != "/aws/dedup" {
                rows.push((k.clone(), value));
            }
        }
        rows.sort_by(|a, b| a.0.cmp(&b.0));
        detail.operational_data = rows;
    }
    detail.related_ops_items = item
        .related_ops_items()
        .iter()
        .map(|r| r.ops_item_id().to_string())
        .collect();
    detail.notifications = item
        .notifications()
        .iter()
        .filter_map(|n| n.arn())
        .map(|a| a.to_string())
        .collect();
    Ok(detail)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_sdk_ssm::types::{Association, AssociationOverview, Target};

    fn assoc(status: Option<&str>) -> Association {
        let mut overview = AssociationOverview::builder();
        if let Some(s) = status {
            overview = overview.status(s).detailed_status("Detailed");
        }
        let overview = overview
            .association_status_aggregated_count("Success", 9)
            .association_status_aggregated_count("Failed", 1)
            .build();
        Association::builder()
            .association_id("assoc-1234")
            .association_name("nightly-patching")
            .name("AWS-RunPatchBaseline")
            .schedule_expression("cron(0 2 * * ? *)")
            .targets(
                Target::builder()
                    .key("tag:Environment")
                    .values("prod")
                    .values("staging")
                    .build(),
            )
            .overview(overview)
            .build()
    }

    #[test]
    fn association_parses_summary_fields() {
        let a = SsmAssociation::from_sdk(&assoc(Some("Success")));
        assert_eq!(a.association_id, "assoc-1234");
        assert_eq!(a.name(), "nightly-patching");
        assert_eq!(a.document_name, "AWS-RunPatchBaseline");
        assert_eq!(a.schedule.as_deref(), Some("cron(0 2 * * ? *)"));
        assert_eq!(
            a.targets,
            vec![(
                "tag:Environment".to_string(),
                vec!["prod".to_string(), "staging".to_string()]
            )]
        );
        // Status counts sorted by key.
        assert_eq!(
            a.status_counts,
            vec![("Failed".to_string(), 1), ("Success".to_string(), 9)]
        );
    }

    #[test]
    fn association_states_map_to_colors() {
        assert_eq!(
            SsmAssociation::from_sdk(&assoc(Some("Success"))).state(),
            ResourceState::Available
        );
        assert_eq!(
            SsmAssociation::from_sdk(&assoc(Some("Failed"))).state(),
            ResourceState::Unavailable
        );
        assert_eq!(
            SsmAssociation::from_sdk(&assoc(Some("Pending"))).state(),
            ResourceState::Pending
        );
        assert_eq!(
            SsmAssociation::from_sdk(&assoc(None)).state(),
            ResourceState::Unknown("no runs".to_string())
        );
    }

    #[test]
    fn association_name_falls_back_to_document() {
        let a = SsmAssociation::from_sdk(
            &Association::builder()
                .association_id("assoc-5678")
                .name("AWS-GatherSoftwareInventory")
                .build(),
        );
        assert_eq!(a.name(), "AWS-GatherSoftwareInventory");
    }

    #[test]
    fn resource_counts_prettify() {
        assert_eq!(
            pretty_resource_counts("{Success=9,Failed=1}"),
            "9 Success, 1 Failed"
        );
        assert_eq!(pretty_resource_counts("{}"), "");
    }

    #[test]
    fn inventory_entries_parse_apps_and_detail() {
        let mut app: HashMap<String, String> = HashMap::new();
        app.insert("Name".to_string(), "openssl".to_string());
        app.insert("Version".to_string(), "3.0.8".to_string());
        app.insert("Publisher".to_string(), "OpenSSL Project".to_string());
        assert_eq!(
            parse_app_entry(&app),
            SsmInventoryApp {
                name: "openssl".to_string(),
                version: "3.0.8".to_string(),
                publisher: "OpenSSL Project".to_string(),
            }
        );

        let mut info: HashMap<String, String> = HashMap::new();
        info.insert("CPUModel".to_string(), "Intel Xeon".to_string());
        info.insert("CPUCores".to_string(), "4".to_string());
        info.insert("OSServicePack".to_string(), "".to_string());
        let rows = parse_detailed_info(&info);
        // Fixed label order, empty values skipped.
        assert_eq!(
            rows,
            vec![
                ("CPU Model".to_string(), "Intel Xeon".to_string()),
                ("CPU Cores".to_string(), "4".to_string()),
            ]
        );
    }

    #[test]
    fn command_parses_and_maps_states() {
        use aws_sdk_ssm::types::{Command, CommandStatus};
        let c = Command::builder()
            .command_id("cmd-1234")
            .document_name("AWS-RunShellScript")
            .status(CommandStatus::Failed)
            .target_count(3)
            .completed_count(3)
            .error_count(2)
            .instance_ids("i-0abc123def456")
            .targets(
                Target::builder()
                    .key("tag:Environment")
                    .values("prod")
                    .build(),
            )
            .output_s3_bucket_name("my-bucket")
            .output_s3_key_prefix("ssm/output")
            .build();
        let cmd = SsmCommand::from_sdk(&c);
        assert_eq!(cmd.command_id, "cmd-1234");
        assert_eq!(cmd.name(), "AWS-RunShellScript");
        assert_eq!(cmd.output_s3.as_deref(), Some("s3://my-bucket/ssm/output"));
        assert_eq!(cmd.state(), ResourceState::Unavailable);
        // Fuzzy-searchable by what it ran on.
        assert!(cmd.search_text().contains("i-0abc123def456"));
        assert!(cmd.search_text().contains("prod"));
        let ok = SsmCommand {
            status: "Success".to_string(),
            ..cmd.clone()
        };
        assert_eq!(ok.state(), ResourceState::Available);
        let cancelled = SsmCommand {
            status: "Cancelled".to_string(),
            ..cmd
        };
        assert_eq!(cancelled.state(), ResourceState::Stopped);
    }

    #[test]
    fn automation_states_map_to_colors() {
        use aws_sdk_ssm::types::{AutomationExecutionMetadata, AutomationExecutionStatus};
        let x = AutomationExecutionMetadata::builder()
            .automation_execution_id("exec-1")
            .document_name("AWS-RestartEC2Instance")
            .automation_execution_status(AutomationExecutionStatus::CompletedWithFailure)
            .failure_message("step 2 timed out")
            .build();
        let auto = SsmAutomationExecution::from_sdk(&x);
        assert_eq!(auto.state(), ResourceState::Unavailable);
        assert_eq!(auto.failure_message.as_deref(), Some("step 2 timed out"));
        let waiting = SsmAutomationExecution {
            status: "Waiting".to_string(),
            ..auto
        };
        assert_eq!(waiting.state(), ResourceState::Pending);
    }

    #[test]
    fn output_snippets_truncate_on_char_boundary() {
        assert_eq!(truncate_chars("short", 400), "short");
        let long = "é".repeat(500);
        let t = truncate_chars(&long, 400);
        assert!(t.starts_with(&"é".repeat(400)));
        assert!(t.ends_with("(500 chars)"));
    }

    #[test]
    fn maint_window_parses_and_maps_states() {
        use aws_sdk_ssm::types::MaintenanceWindowIdentity;
        let w = MaintenanceWindowIdentity::builder()
            .window_id("mw-0123456789abcdef0")
            .name("weekly-patching")
            .enabled(true)
            .schedule("cron(0 4 ? * SUN *)")
            .schedule_timezone("Australia/Melbourne")
            .duration(3)
            .cutoff(1)
            .next_execution_time("2026-07-12T04:00:00+10:00")
            .build();
        let mw = SsmMaintWindow::from_sdk(&w);
        assert_eq!(mw.name(), "weekly-patching");
        assert_eq!(mw.state(), ResourceState::Available);
        assert_eq!(
            mw.next_execution.as_deref(),
            Some("2026-07-12 04:00:00+10:00")
        );
        let disabled = SsmMaintWindow { enabled: false, ..mw };
        assert_eq!(disabled.state(), ResourceState::Stopped);
    }

    #[test]
    fn aws_provided_baselines_are_noise() {
        use aws_sdk_ssm::types::{OperatingSystem, PatchBaselineIdentity};
        let aws = SsmPatchBaseline::from_sdk(
            &PatchBaselineIdentity::builder()
                .baseline_id("arn:aws:ssm:ap-southeast-2::patchbaseline/pb-0e392de35e7c563b7")
                .baseline_name("AWS-AmazonLinux2DefaultPatchBaseline")
                .operating_system(OperatingSystem::AmazonLinux2)
                .default_baseline(true)
                .build(),
        );
        assert!(aws.is_noise());
        let custom = SsmPatchBaseline::from_sdk(
            &PatchBaselineIdentity::builder()
                .baseline_id("pb-0abcdef1234567890")
                .baseline_name("prod-linux-baseline")
                .operating_system(OperatingSystem::AmazonLinux2023)
                .build(),
        );
        assert!(!custom.is_noise());
        assert_eq!(custom.name(), "prod-linux-baseline");
    }

    #[test]
    fn ops_items_map_states_and_hide_resolved() {
        use aws_sdk_ssm::types::{OpsItemStatus, OpsItemSummary};
        let open = SsmOpsItem::from_sdk(
            &OpsItemSummary::builder()
                .ops_item_id("oi-0011")
                .title("High CPU on prod fleet")
                .status(OpsItemStatus::Open)
                .severity("2")
                .source("CloudWatch")
                .build(),
        );
        assert_eq!(open.name(), "High CPU on prod fleet");
        assert_eq!(open.state(), ResourceState::Pending);
        assert!(!open.is_noise());
        let resolved = SsmOpsItem {
            status: "Resolved".to_string(),
            ..open.clone()
        };
        assert_eq!(resolved.state(), ResourceState::Available);
        assert!(resolved.is_noise());
        let failed = SsmOpsItem {
            status: "Failed".to_string(),
            ..open
        };
        assert_eq!(failed.state(), ResourceState::Unavailable);
    }

    #[test]
    fn sessions_map_states() {
        use aws_sdk_ssm::types::{Session, SessionStatus};
        let s = SsmSession::from_sdk(
            &Session::builder()
                .session_id("stojan-0abc")
                .target("i-0abc123def456")
                .status(SessionStatus::Connected)
                .owner("arn:aws:iam::123456789012:user/stojan")
                .build(),
        );
        assert_eq!(s.name(), "i-0abc123def456");
        assert_eq!(s.state(), ResourceState::Running);
        let done = SsmSession {
            status: "Terminated".to_string(),
            ..s
        };
        assert_eq!(done.state(), ResourceState::Stopped);
    }

    #[test]
    fn ops_item_resource_arns_extract_from_json() {
        let json = r#"[{"arn":"arn:aws:ec2:ap-southeast-2:123456789012:instance/i-0abc"},
                       {"nested":{"arn":"arn:aws:s3:::my-bucket"}}]"#;
        assert_eq!(
            extract_resource_arns(json),
            vec![
                "arn:aws:ec2:ap-southeast-2:123456789012:instance/i-0abc".to_string(),
                "arn:aws:s3:::my-bucket".to_string(),
            ]
        );
        assert!(extract_resource_arns("not json").is_empty());
    }

    #[test]
    fn only_actionable_patch_states_are_listed() {
        for s in [
            "MISSING",
            "FAILED",
            "INSTALLED_PENDING_REBOOT",
            "INSTALLED_REJECTED",
            "AVAILABLE_SECURITY_UPDATE",
        ] {
            assert!(is_actionable_patch(s), "{s} should be listed");
        }
        for s in ["INSTALLED", "INSTALLED_OTHER", "NOT_APPLICABLE"] {
            assert!(!is_actionable_patch(s), "{s} is noise");
        }
    }
}
