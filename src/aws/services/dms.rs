use crate::aws::cli_actions::{CliAction, CliTier};
use crate::aws::client::AwsClients;
use crate::aws::resource::{native_state_label, shell_quote, Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::aws::services::ec2::MetricsTimeRange;
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use aws_sdk_databasemigration::Client as DmsClient;
use std::any::Any;
use std::collections::HashMap;
use tokio::sync::mpsc;

/// AWS Database Migration Service — four sub-tabs: **Tasks** (the primary
/// view: "why did my replication stop / why is it lagging?"), **Replication
/// instances**, **Endpoints** and **Serverless** replications. One region-
/// scoped client; every list is a paginated `Describe*`.
///
/// Load order: instances + endpoints + connections first (their names label
/// the task rows), then tasks, then serverless configs joined to their
/// replications, then **one batched `ListTagsForResource`** over every ARN
/// (it takes a `ResourceArnList`, so tags cost a handful of calls rather than
/// N+1). Everything is sent once tags are attached — a DMS account has tens
/// of resources, not thousands, so the stream loses nothing by batching.
///
/// Lazy per-row sections: a task's **Tables** (`DescribeTableStatistics`) and
/// **Assessments** (`DescribeReplicationTaskAssessmentRuns`), a serverless
/// replication's **Tables** (`DescribeReplicationTableStatistics`). An
/// instance's Tasks and an endpoint's Used-by sections filter sibling rows
/// already loaded — no fetch.
///
/// Credentials: endpoint engine settings carry a `password` field on some
/// engines. It is **never read**; only the Secrets Manager secret *reference*
/// is kept, and it is never resolved.
pub struct DmsService {
    client: DmsClient,
}

impl DmsService {
    pub fn new(aws_clients: &AwsClients) -> Self {
        Self {
            client: aws_clients.dms_client(),
        }
    }
}

fn fmt_dt(dt: &aws_smithy_types::DateTime) -> String {
    crate::aws::services::cloudwatch::fmt_epoch_secs(dt.secs())
}

/// Last `:`-segment of a DMS ARN — the opaque resource id
/// (`arn:aws:dms:…:task:ABC123` → `ABC123`). Log streams and task metrics key
/// on it, not on the user-facing identifier.
pub fn arn_resource_id(arn: &str) -> &str {
    arn.rsplit(':').next().unwrap_or(arn)
}

/// Tags batch size for `ListTagsForResource`'s `ResourceArnList`.
const TAG_BATCH: usize = 50;

#[async_trait]
impl AwsService for DmsService {
    fn service_type(&self) -> ServiceType {
        ServiceType::Dms
    }

    fn name(&self) -> &str {
        "DMS"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        self.list_resources_streaming(tx, ServiceType::Dms).await?;
        Ok(vec![])
    }

    async fn list_resources_streaming(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        let warn = |msg: String| {
            let _ = event_tx.send(Event::ResourceLoadWarning {
                service: service_type,
                warning: msg,
            });
        };
        let progress = |msg: &str| {
            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                service: service_type,
                resources: Vec::new(),
                progress: LoadProgress {
                    loaded_count: 0,
                    total_count: None,
                    status_message: Some(msg.to_string()),
                },
            });
        };

        // ── Replication instances ─────────────────────────────────────────────
        let mut instances: Vec<DmsInstance> = Vec::new();
        let mut instances_failed = None;
        let mut pages = self.client.describe_replication_instances().into_paginator().send();
        while let Some(page) = pages.next().await {
            match page {
                Ok(p) => instances.extend(p.replication_instances().iter().map(DmsInstance::from_sdk)),
                Err(e) => {
                    instances_failed = Some(crate::error::sdk_error_message(&e));
                    break;
                }
            }
        }

        // ── Endpoints ─────────────────────────────────────────────────────────
        let mut endpoints: Vec<DmsEndpoint> = Vec::new();
        let mut endpoints_failed = None;
        let mut pages = self.client.describe_endpoints().into_paginator().send();
        while let Some(page) = pages.next().await {
            match page {
                Ok(p) => endpoints.extend(p.endpoints().iter().map(DmsEndpoint::from_sdk)),
                Err(e) => {
                    endpoints_failed = Some(crate::error::sdk_error_message(&e));
                    break;
                }
            }
        }

        // ── Connection tests (endpoint × instance) ────────────────────────────
        let mut connections: HashMap<String, Vec<DmsConnection>> = HashMap::new();
        let mut pages = self.client.describe_connections().into_paginator().send();
        while let Some(page) = pages.next().await {
            match page {
                Ok(p) => {
                    for c in p.connections() {
                        if let Some(ep) = c.endpoint_arn() {
                            connections.entry(ep.to_string()).or_default().push(DmsConnection {
                                instance_arn: c.replication_instance_arn().unwrap_or_default().to_string(),
                                instance_id: c
                                    .replication_instance_identifier()
                                    .unwrap_or_default()
                                    .to_string(),
                                status: c.status().unwrap_or("unknown").to_string(),
                                last_failure: c.last_failure_message().map(str::to_string),
                            });
                        }
                    }
                }
                Err(e) => {
                    warn(format!("DMS connections: {}", crate::error::sdk_error_message(&e)));
                    break;
                }
            }
        }
        for ep in &mut endpoints {
            if let Some(c) = connections.remove(&ep.arn) {
                ep.connections = c;
            }
        }

        let instance_names: HashMap<String, String> =
            instances.iter().map(|i| (i.arn.clone(), i.identifier.clone())).collect();
        let endpoint_names: HashMap<String, (String, String)> = endpoints
            .iter()
            .map(|e| (e.arn.clone(), (e.identifier.clone(), e.engine_label())))
            .collect();

        progress("Loading replication tasks…");

        // ── Replication tasks ─────────────────────────────────────────────────
        let mut tasks: Vec<DmsTask> = Vec::new();
        let mut tasks_failed = None;
        let mut pages = self.client.describe_replication_tasks().into_paginator().send();
        while let Some(page) = pages.next().await {
            match page {
                Ok(p) => tasks.extend(
                    p.replication_tasks()
                        .iter()
                        .map(|t| DmsTask::from_sdk(t, &instance_names, &endpoint_names)),
                ),
                Err(e) => {
                    tasks_failed = Some(crate::error::sdk_error_message(&e));
                    break;
                }
            }
        }

        // ── Serverless: configs joined to their replications ──────────────────
        let mut replications: HashMap<String, aws_sdk_databasemigration::types::Replication> =
            HashMap::new();
        let mut pages = self.client.describe_replications().into_paginator().send();
        while let Some(page) = pages.next().await {
            match page {
                Ok(p) => {
                    for r in p.replications() {
                        if let Some(arn) = r.replication_config_arn() {
                            replications.insert(arn.to_string(), r.clone());
                        }
                    }
                }
                Err(e) => {
                    warn(format!("DMS serverless replications: {}", crate::error::sdk_error_message(&e)));
                    break;
                }
            }
        }
        let mut serverless: Vec<DmsServerless> = Vec::new();
        let mut pages = self.client.describe_replication_configs().into_paginator().send();
        while let Some(page) = pages.next().await {
            match page {
                Ok(p) => {
                    for c in p.replication_configs() {
                        let rep = c.replication_config_arn().and_then(|a| replications.get(a));
                        serverless.push(DmsServerless::from_sdk(c, rep, &endpoint_names));
                    }
                }
                Err(e) => {
                    warn(format!("DMS serverless configs: {}", crate::error::sdk_error_message(&e)));
                    break;
                }
            }
        }

        // Every core list failed: nothing can stream, so it's a load error
        // (the usual cause is a missing dms:Describe* permission).
        if let (Some(t), Some(i), Some(e)) = (&tasks_failed, &instances_failed, &endpoints_failed) {
            let _ = event_tx.send(Event::ResourceLoadError {
                service: service_type,
                error: format!("DMS unavailable: {t}"),
            });
            let _ = (i, e);
            return Ok(());
        }
        for (what, err) in [
            ("tasks", &tasks_failed),
            ("replication instances", &instances_failed),
            ("endpoints", &endpoints_failed),
        ] {
            if let Some(e) = err {
                warn(format!("DMS {what}: {e}"));
            }
        }

        // ── Tags, batched over every ARN ──────────────────────────────────────
        let arns: Vec<String> = tasks
            .iter()
            .map(|t| t.arn.clone())
            .chain(instances.iter().map(|i| i.arn.clone()))
            .chain(endpoints.iter().map(|e| e.arn.clone()))
            .chain(serverless.iter().map(|s| s.arn.clone()))
            .filter(|a| !a.is_empty())
            .collect();
        let mut tags: HashMap<String, HashMap<String, String>> = HashMap::new();
        if !arns.is_empty() {
            progress("Loading tags…");
        }
        for chunk in arns.chunks(TAG_BATCH) {
            match self
                .client
                .list_tags_for_resource()
                .set_resource_arn_list(Some(chunk.to_vec()))
                .send()
                .await
            {
                Ok(out) => {
                    for t in out.tag_list() {
                        if let (Some(arn), Some(k)) = (t.resource_arn(), t.key()) {
                            tags.entry(arn.to_string())
                                .or_default()
                                .insert(k.to_string(), t.value().unwrap_or_default().to_string());
                        }
                    }
                }
                Err(e) => {
                    warn(format!("DMS tags: {}", crate::error::sdk_error_message(&e)));
                    break;
                }
            }
        }
        let mut take = |arn: &str| tags.remove(arn).unwrap_or_default();
        for t in &mut tasks {
            t.tags = take(&t.arn);
        }
        for i in &mut instances {
            i.tags = take(&i.arn);
        }
        for e in &mut endpoints {
            e.tags = take(&e.arn);
        }
        for s in &mut serverless {
            s.tags = take(&s.arn);
        }

        // Tasks first (the primary tab), failed/stopped-on-error first so the
        // rows that need attention lead.
        tasks.sort_by(|a, b| b.needs_attention().cmp(&a.needs_attention()).then(a.identifier.cmp(&b.identifier)));
        instances.sort_by(|a, b| a.identifier.cmp(&b.identifier));
        endpoints.sort_by(|a, b| a.identifier.cmp(&b.identifier));
        serverless.sort_by(|a, b| a.identifier.cmp(&b.identifier));

        let mut batch: Vec<Box<dyn Resource>> = Vec::new();
        batch.extend(tasks.into_iter().map(|r| Box::new(r) as Box<dyn Resource>));
        batch.extend(instances.into_iter().map(|r| Box::new(r) as Box<dyn Resource>));
        batch.extend(endpoints.into_iter().map(|r| Box::new(r) as Box<dyn Resource>));
        batch.extend(serverless.into_iter().map(|r| Box::new(r) as Box<dyn Resource>));
        let total = batch.len();
        if total > 0 {
            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                service: service_type,
                resources: batch,
                progress: LoadProgress {
                    loaded_count: total,
                    total_count: Some(total),
                    status_message: None,
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

/// Parse a JSON settings blob, tolerating absence / garbage.
fn parse_json(s: Option<&str>) -> Option<serde_json::Value> {
    s.and_then(|s| serde_json::from_str(s).ok())
}

/// Pretty-print a JSON settings blob (falls back to the raw text).
pub fn pretty_json(s: &str) -> String {
    serde_json::from_str::<serde_json::Value>(s)
        .and_then(|v| serde_json::to_string_pretty(&v))
        .unwrap_or_else(|_| s.to_string())
}

/// Progress stats shared by provisioned tasks and serverless replications.
#[derive(Debug, Clone, Default)]
pub struct DmsStats {
    pub full_load_pct: i32,
    pub elapsed_ms: i64,
    pub tables_loaded: i32,
    pub tables_loading: i32,
    pub tables_queued: i32,
    pub tables_errored: i32,
    pub started: Option<String>,
    pub stopped: Option<String>,
    pub full_load_started: Option<String>,
    pub full_load_finished: Option<String>,
}

// ── Replication task ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct DmsTask {
    pub arn: String,
    pub identifier: String,
    pub status: String,
    pub migration_type: String,
    pub source_endpoint_arn: String,
    pub target_endpoint_arn: String,
    /// `(identifier, engine)` resolved from the endpoints list at load time.
    pub source_endpoint: Option<(String, String)>,
    pub target_endpoint: Option<(String, String)>,
    pub instance_arn: String,
    /// Replication instance identifier, resolved at load time — also the
    /// log group suffix and the metrics dimension.
    pub instance_id: Option<String>,
    pub stop_reason: Option<String>,
    pub last_failure: Option<String>,
    pub created: Option<String>,
    pub started: Option<String>,
    pub cdc_start_position: Option<String>,
    pub cdc_stop_position: Option<String>,
    pub recovery_checkpoint: Option<String>,
    pub stats: Option<DmsStats>,
    pub table_mappings: Option<String>,
    pub settings: Option<String>,
    /// `Logging.EnableLogging` from the settings — whether `t` has a log
    /// group to tail.
    pub logging_enabled: Option<bool>,
    pub tags: HashMap<String, String>,
}

impl DmsTask {
    pub fn from_sdk(
        t: &aws_sdk_databasemigration::types::ReplicationTask,
        instance_names: &HashMap<String, String>,
        endpoint_names: &HashMap<String, (String, String)>,
    ) -> Self {
        let instance_arn = t.replication_instance_arn().unwrap_or_default().to_string();
        let source = t.source_endpoint_arn().unwrap_or_default().to_string();
        let target = t.target_endpoint_arn().unwrap_or_default().to_string();
        let settings = t.replication_task_settings().map(str::to_string);
        let logging_enabled = parse_json(settings.as_deref())
            .and_then(|v| v.get("Logging")?.get("EnableLogging")?.as_bool());
        Self {
            arn: t.replication_task_arn().unwrap_or_default().to_string(),
            identifier: t.replication_task_identifier().unwrap_or_default().to_string(),
            status: t.status().unwrap_or("unknown").to_string(),
            migration_type: t
                .migration_type()
                .map(|m| m.as_str().to_string())
                .unwrap_or_default(),
            source_endpoint: endpoint_names.get(&source).cloned(),
            target_endpoint: endpoint_names.get(&target).cloned(),
            source_endpoint_arn: source,
            target_endpoint_arn: target,
            instance_id: instance_names.get(&instance_arn).cloned(),
            instance_arn,
            stop_reason: t.stop_reason().filter(|s| !s.is_empty()).map(str::to_string),
            last_failure: t.last_failure_message().filter(|s| !s.is_empty()).map(str::to_string),
            created: t.replication_task_creation_date().map(fmt_dt),
            started: t.replication_task_start_date().map(fmt_dt),
            cdc_start_position: t.cdc_start_position().map(str::to_string),
            cdc_stop_position: t.cdc_stop_position().map(str::to_string),
            recovery_checkpoint: t.recovery_checkpoint().map(str::to_string),
            stats: t.replication_task_stats().map(|s| DmsStats {
                full_load_pct: s.full_load_progress_percent(),
                elapsed_ms: s.elapsed_time_millis(),
                tables_loaded: s.tables_loaded(),
                tables_loading: s.tables_loading(),
                tables_queued: s.tables_queued(),
                tables_errored: s.tables_errored(),
                started: s.start_date().map(fmt_dt),
                stopped: s.stop_date().map(fmt_dt),
                full_load_started: s.full_load_start_date().map(fmt_dt),
                full_load_finished: s.full_load_finish_date().map(fmt_dt),
            }),
            table_mappings: t.table_mappings().map(str::to_string),
            settings,
            logging_enabled,
            tags: HashMap::new(),
        }
    }

    /// Stopped by an error (not by a user or a completed full load) — DMS
    /// leaves `status` at `stopped` and says why only in the stop reason.
    pub fn stopped_on_error(&self) -> bool {
        self.status == "stopped"
            && self
                .stop_reason
                .as_deref()
                .is_some_and(|r| r.to_ascii_uppercase().contains("ERROR"))
    }

    fn needs_attention(&self) -> bool {
        self.status.starts_with("failed")
            || self.stopped_on_error()
            || self.stats.as_ref().is_some_and(|s| s.tables_errored > 0)
    }

    /// Log group a task writes to when CloudWatch logging is on.
    pub fn log_group(&self) -> Option<String> {
        self.instance_id.as_ref().map(|i| format!("dms-tasks-{}", i.to_lowercase()))
    }

    /// Log stream of this task within [`Self::log_group`].
    pub fn log_stream(&self) -> String {
        format!("dms-task-{}", arn_resource_id(&self.arn))
    }
}

crate::sections! {
    pub enum DmsTaskDetailSection,
    pub static DMS_TASK_SECTIONS = [
        Overview "Overview",
        Tables "Tables" => crate::app::App::trigger_dms_table_stats_load,
        Assessments "Assessments" => crate::app::App::trigger_dms_assessments_load,
        Settings "Settings",
        Tags "Tags",
    ]
}

impl Resource for DmsTask {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&DMS_TASK_SECTIONS)
    }

    fn references(&self) -> Vec<(String, String)> {
        let mut out = vec![
            ("Source Endpoint".to_string(), self.source_endpoint_arn.clone()),
            ("Target Endpoint".to_string(), self.target_endpoint_arn.clone()),
            ("Replication Instance".to_string(), self.instance_arn.clone()),
        ];
        out.retain(|(_, v)| !v.is_empty());
        out
    }

    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws dms describe-replication-tasks --filters Name=replication-task-arn,Values={}",
            shell_quote(&self.arn)
        ))
    }

    fn cli_actions(&self) -> Vec<CliAction> {
        let arn = shell_quote(&self.arn);
        vec![
            CliAction::new(
                CliTier::Inspect,
                "describe-table-statistics",
                format!("aws dms describe-table-statistics --replication-task-arn {}", arn),
            ),
            CliAction::new(
                CliTier::Change,
                "start (resume-processing)",
                format!(
                    "aws dms start-replication-task --replication-task-arn {} --start-replication-task-type resume-processing",
                    arn
                ),
            )
            .with_note("resumes from the last checkpoint — reload-target would restart the full load"),
            CliAction::new(
                CliTier::Change,
                "stop-replication-task",
                format!("aws dms stop-replication-task --replication-task-arn {}", arn),
            ),
        ]
    }

    fn id(&self) -> &str {
        &self.arn
    }

    fn name(&self) -> &str {
        &self.identifier
    }

    fn resource_type(&self) -> &str {
        "DMS Task"
    }

    fn state(&self) -> ResourceState {
        if self.stopped_on_error() {
            return ResourceState::Unavailable;
        }
        match self.status.as_str() {
            "running" => ResourceState::Running,
            "stopped" | "ready" => ResourceState::Stopped,
            "failed" | "failed-move" => ResourceState::Unavailable,
            "creating" => ResourceState::Creating,
            "deleting" | "deleted" => ResourceState::Deleting,
            "starting" | "stopping" | "modifying" | "moving" | "testing" => ResourceState::Pending,
            other => ResourceState::Unknown(other.to_string()),
        }
    }

    fn state_label(&self) -> String {
        if self.stopped_on_error() {
            return "stopped (error)".to_string();
        }
        native_state_label(&self.status, || self.state())
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!(
            "{} {} {} {} {} {} dms task",
            self.identifier,
            self.status,
            self.migration_type,
            self.source_endpoint.as_ref().map(|e| e.0.as_str()).unwrap_or(""),
            self.target_endpoint.as_ref().map(|e| e.0.as_str()).unwrap_or(""),
            self.instance_id.as_deref().unwrap_or(""),
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Task".to_string(), self.identifier.clone()),
            ("Status".to_string(), self.status.clone()),
            ("Migration Type".to_string(), self.migration_type.clone()),
        ]
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{region}.console.aws.amazon.com/dms/v2/home?region={region}#taskDetails/{}",
            self.identifier
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── Replication instance ──────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct DmsInstance {
    pub arn: String,
    pub identifier: String,
    pub class: String,
    pub status: String,
    pub engine_version: Option<String>,
    pub allocated_storage_gb: i32,
    pub multi_az: bool,
    pub availability_zone: Option<String>,
    pub secondary_az: Option<String>,
    pub publicly_accessible: bool,
    pub auto_minor_upgrade: bool,
    pub maintenance_window: Option<String>,
    pub kms_key_id: Option<String>,
    pub network_type: Option<String>,
    pub private_ips: Vec<String>,
    pub public_ips: Vec<String>,
    pub vpc_id: Option<String>,
    pub subnet_group: Option<String>,
    pub subnets: Vec<(String, String)>, // (subnet id, az)
    pub security_groups: Vec<(String, String)>, // (sg id, status)
    pub pending: Vec<String>,
    pub created: Option<String>,
    pub tags: HashMap<String, String>,
}

impl DmsInstance {
    pub fn from_sdk(i: &aws_sdk_databasemigration::types::ReplicationInstance) -> Self {
        let sng = i.replication_subnet_group();
        let mut pending = Vec::new();
        if let Some(p) = i.pending_modified_values() {
            if let Some(c) = p.replication_instance_class() {
                pending.push(format!("class → {c}"));
            }
            if let Some(s) = p.allocated_storage() {
                pending.push(format!("storage → {s} GB"));
            }
            if let Some(m) = p.multi_az() {
                pending.push(format!("multi-AZ → {m}"));
            }
            if let Some(v) = p.engine_version() {
                pending.push(format!("engine → {v}"));
            }
        }
        Self {
            arn: i.replication_instance_arn().unwrap_or_default().to_string(),
            identifier: i.replication_instance_identifier().unwrap_or_default().to_string(),
            class: i.replication_instance_class().unwrap_or_default().to_string(),
            status: i.replication_instance_status().unwrap_or("unknown").to_string(),
            engine_version: i.engine_version().map(str::to_string),
            allocated_storage_gb: i.allocated_storage(),
            multi_az: i.multi_az(),
            availability_zone: i.availability_zone().map(str::to_string),
            secondary_az: i.secondary_availability_zone().map(str::to_string),
            publicly_accessible: i.publicly_accessible(),
            auto_minor_upgrade: i.auto_minor_version_upgrade(),
            maintenance_window: i.preferred_maintenance_window().map(str::to_string),
            kms_key_id: i.kms_key_id().map(str::to_string),
            network_type: i.network_type().map(str::to_string),
            private_ips: i.replication_instance_private_ip_addresses().to_vec(),
            public_ips: i.replication_instance_public_ip_addresses().to_vec(),
            vpc_id: sng.and_then(|g| g.vpc_id()).map(str::to_string),
            subnet_group: sng
                .and_then(|g| g.replication_subnet_group_identifier())
                .map(str::to_string),
            subnets: sng
                .map(|g| {
                    g.subnets()
                        .iter()
                        .filter_map(|s| {
                            Some((
                                s.subnet_identifier()?.to_string(),
                                s.subnet_availability_zone()
                                    .and_then(|a| a.name())
                                    .unwrap_or_default()
                                    .to_string(),
                            ))
                        })
                        .collect()
                })
                .unwrap_or_default(),
            security_groups: i
                .vpc_security_groups()
                .iter()
                .filter_map(|g| {
                    Some((
                        g.vpc_security_group_id()?.to_string(),
                        g.status().unwrap_or_default().to_string(),
                    ))
                })
                .collect(),
            pending,
            created: i.instance_create_time().map(fmt_dt),
            tags: HashMap::new(),
        }
    }
}

crate::sections! {
    pub enum DmsInstanceDetailSection,
    pub static DMS_INSTANCE_SECTIONS = [
        Overview "Overview",
        Network "Network",
        Tasks "Tasks",
        Tags "Tags",
    ]
}

impl Resource for DmsInstance {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&DMS_INSTANCE_SECTIONS)
    }

    fn security_group_ids(&self) -> Vec<String> {
        self.security_groups.iter().map(|(id, _)| id.clone()).collect()
    }

    fn references(&self) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = self
            .security_groups
            .iter()
            .map(|(id, _)| ("Security Group".to_string(), id.clone()))
            .collect();
        if let Some(v) = &self.vpc_id {
            out.push(("VPC".to_string(), v.clone()));
        }
        out.extend(self.subnets.iter().map(|(s, _)| ("Subnet".to_string(), s.clone())));
        if let Some(k) = &self.kms_key_id {
            out.push(("KMS Key".to_string(), k.clone()));
        }
        out
    }

    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws dms describe-replication-instances --filters Name=replication-instance-arn,Values={}",
            shell_quote(&self.arn)
        ))
    }

    fn cli_actions(&self) -> Vec<CliAction> {
        vec![CliAction::new(
            CliTier::Inspect,
            "describe-connections",
            format!(
                "aws dms describe-connections --filters Name=replication-instance-arn,Values={}",
                shell_quote(&self.arn)
            ),
        )]
    }

    fn id(&self) -> &str {
        &self.arn
    }

    fn name(&self) -> &str {
        &self.identifier
    }

    fn resource_type(&self) -> &str {
        "DMS Replication Instance"
    }

    fn state(&self) -> ResourceState {
        match self.status.as_str() {
            "available" => ResourceState::Available,
            "creating" => ResourceState::Creating,
            "deleting" => ResourceState::Deleting,
            "modifying" | "upgrading" | "rebooting" | "resetting-master-credentials"
            | "maintenance" => ResourceState::Pending,
            "failed" | "storage-full" | "incompatible-credentials" | "incompatible-network"
            | "inaccessible-encryption-credentials" => ResourceState::Unavailable,
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
            "{} {} {} {} dms replication instance",
            self.identifier,
            self.class,
            self.status,
            self.vpc_id.as_deref().unwrap_or("")
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Instance".to_string(), self.identifier.clone()),
            ("Class".to_string(), self.class.clone()),
            ("Status".to_string(), self.status.clone()),
        ]
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{region}.console.aws.amazon.com/dms/v2/home?region={region}#replicationInstanceDetails/{}",
            self.identifier
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── Endpoint ──────────────────────────────────────────────────────────────────

/// One `DescribeConnections` row: the last connection test between an
/// endpoint and a replication instance.
#[derive(Debug, Clone)]
pub struct DmsConnection {
    pub instance_arn: String,
    pub instance_id: String,
    pub status: String,
    pub last_failure: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DmsEndpoint {
    pub arn: String,
    pub identifier: String,
    /// `source` / `target`.
    pub endpoint_type: String,
    pub engine: String,
    pub engine_display: Option<String>,
    pub status: String,
    pub server: Option<String>,
    pub port: Option<i32>,
    pub database: Option<String>,
    pub username: Option<String>,
    pub ssl_mode: Option<String>,
    pub certificate_arn: Option<String>,
    pub kms_key_id: Option<String>,
    pub service_access_role: Option<String>,
    pub extra_connection_attributes: Option<String>,
    /// Secrets Manager secret the endpoint reads its credentials from — a
    /// **reference only**, never resolved.
    pub secret_ref: Option<String>,
    pub connections: Vec<DmsConnection>,
    pub tags: HashMap<String, String>,
}

impl DmsEndpoint {
    pub fn from_sdk(e: &aws_sdk_databasemigration::types::Endpoint) -> Self {
        // The secret reference lives in the engine-specific settings block.
        // Only `secrets_manager_secret_id` is read — never `password`.
        let secret_ref = e
            .postgre_sql_settings()
            .and_then(|s| s.secrets_manager_secret_id())
            .or_else(|| e.my_sql_settings().and_then(|s| s.secrets_manager_secret_id()))
            .or_else(|| e.oracle_settings().and_then(|s| s.secrets_manager_secret_id()))
            .or_else(|| e.microsoft_sql_server_settings().and_then(|s| s.secrets_manager_secret_id()))
            .or_else(|| e.redshift_settings().and_then(|s| s.secrets_manager_secret_id()))
            .or_else(|| e.sybase_settings().and_then(|s| s.secrets_manager_secret_id()))
            .or_else(|| e.ibm_db2_settings().and_then(|s| s.secrets_manager_secret_id()))
            .or_else(|| e.doc_db_settings().and_then(|s| s.secrets_manager_secret_id()))
            .or_else(|| e.mongo_db_settings().and_then(|s| s.secrets_manager_secret_id()))
            .map(str::to_string);
        Self {
            arn: e.endpoint_arn().unwrap_or_default().to_string(),
            identifier: e.endpoint_identifier().unwrap_or_default().to_string(),
            endpoint_type: e
                .endpoint_type()
                .map(|t| t.as_str().to_lowercase())
                .unwrap_or_default(),
            engine: e.engine_name().unwrap_or_default().to_string(),
            engine_display: e.engine_display_name().map(str::to_string),
            status: e.status().unwrap_or("unknown").to_string(),
            server: e.server_name().map(str::to_string),
            port: e.port(),
            database: e.database_name().map(str::to_string),
            username: e.username().map(str::to_string),
            ssl_mode: e.ssl_mode().map(|m| m.as_str().to_string()),
            certificate_arn: e.certificate_arn().map(str::to_string),
            kms_key_id: e.kms_key_id().map(str::to_string),
            service_access_role: e.service_access_role_arn().map(str::to_string),
            extra_connection_attributes: e
                .extra_connection_attributes()
                .filter(|s| !s.is_empty())
                .map(str::to_string),
            secret_ref,
            connections: Vec::new(),
            tags: HashMap::new(),
        }
    }

    /// "PostgreSQL" / "aurora-postgresql" — display name when AWS gives one.
    pub fn engine_label(&self) -> String {
        self.engine_display.clone().unwrap_or_else(|| self.engine.clone())
    }

    /// Any connection test failing — the usual "task won't start" cause.
    pub fn has_failed_connection(&self) -> bool {
        self.connections.iter().any(|c| c.status == "failed")
    }
}

crate::sections! {
    pub enum DmsEndpointDetailSection,
    pub static DMS_ENDPOINT_SECTIONS = [
        Overview "Overview",
        Connections "Connections",
        UsedBy "Used by",
        Tags "Tags",
    ]
}

impl Resource for DmsEndpoint {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&DMS_ENDPOINT_SECTIONS)
    }

    fn references(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        if let Some(s) = &self.secret_ref {
            out.push(("Secret".to_string(), s.clone()));
        }
        if let Some(r) = &self.service_access_role {
            out.push(("Service Access Role".to_string(), r.clone()));
        }
        if let Some(k) = &self.kms_key_id {
            out.push(("KMS Key".to_string(), k.clone()));
        }
        if let Some(c) = &self.certificate_arn {
            out.push(("Certificate".to_string(), c.clone()));
        }
        out
    }

    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws dms describe-endpoints --filters Name=endpoint-arn,Values={}",
            shell_quote(&self.arn)
        ))
    }

    fn cli_actions(&self) -> Vec<CliAction> {
        let mut out = vec![CliAction::new(
            CliTier::Inspect,
            "describe-connections",
            format!(
                "aws dms describe-connections --filters Name=endpoint-arn,Values={}",
                shell_quote(&self.arn)
            ),
        )];
        // `test-connection` needs an instance; offer one row per instance the
        // endpoint has been tested from. It writes a new connection-test
        // record (and opens a connection to the database), so Change tier.
        for c in &self.connections {
            out.push(
                CliAction::new(
                    CliTier::Change,
                    format!("test-connection · {}", c.instance_id),
                    format!(
                        "aws dms test-connection --replication-instance-arn {} --endpoint-arn {}",
                        shell_quote(&c.instance_arn),
                        shell_quote(&self.arn)
                    ),
                )
                .with_note("opens a connection from the replication instance to the database"),
            );
        }
        out
    }

    fn id(&self) -> &str {
        &self.arn
    }

    fn name(&self) -> &str {
        &self.identifier
    }

    fn resource_type(&self) -> &str {
        "DMS Endpoint"
    }

    fn state(&self) -> ResourceState {
        if self.has_failed_connection() {
            return ResourceState::Unavailable;
        }
        match self.status.as_str() {
            "active" => ResourceState::Available,
            "creating" => ResourceState::Creating,
            "deleting" => ResourceState::Deleting,
            other => ResourceState::Unknown(other.to_string()),
        }
    }

    fn state_label(&self) -> String {
        if self.has_failed_connection() {
            return "connection failed".to_string();
        }
        native_state_label(&self.status, || self.state())
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!(
            "{} {} {} {} {} dms endpoint",
            self.identifier,
            self.endpoint_type,
            self.engine,
            self.server.as_deref().unwrap_or(""),
            self.database.as_deref().unwrap_or("")
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Endpoint".to_string(), self.identifier.clone()),
            ("Type".to_string(), self.endpoint_type.clone()),
            ("Engine".to_string(), self.engine_label()),
        ]
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{region}.console.aws.amazon.com/dms/v2/home?region={region}#endpointDetails/{}",
            self.identifier
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── Serverless replication (config + its replication) ────────────────────────

#[derive(Debug, Clone)]
pub struct DmsServerless {
    pub arn: String,
    pub identifier: String,
    pub replication_type: String,
    pub source_endpoint_arn: String,
    pub target_endpoint_arn: String,
    pub source_endpoint: Option<(String, String)>,
    pub target_endpoint: Option<(String, String)>,
    pub min_dcu: Option<i32>,
    pub max_dcu: Option<i32>,
    pub multi_az: Option<bool>,
    pub subnet_group: Option<String>,
    pub security_group_ids: Vec<String>,
    pub kms_key_id: Option<String>,
    pub created: Option<String>,
    pub table_mappings: Option<String>,
    pub settings: Option<String>,
    // The replication itself — None until it has been started once.
    pub status: Option<String>,
    pub stop_reason: Option<String>,
    pub failures: Vec<String>,
    pub provision_state: Option<String>,
    pub provisioned_dcu: Option<i32>,
    pub stats: Option<DmsStats>,
    pub cdc_start_position: Option<String>,
    pub recovery_checkpoint: Option<String>,
    pub last_stop: Option<String>,
    pub tags: HashMap<String, String>,
}

impl DmsServerless {
    pub fn from_sdk(
        c: &aws_sdk_databasemigration::types::ReplicationConfig,
        rep: Option<&aws_sdk_databasemigration::types::Replication>,
        endpoint_names: &HashMap<String, (String, String)>,
    ) -> Self {
        let source = c.source_endpoint_arn().unwrap_or_default().to_string();
        let target = c.target_endpoint_arn().unwrap_or_default().to_string();
        let cc = c.compute_config();
        Self {
            arn: c.replication_config_arn().unwrap_or_default().to_string(),
            identifier: c.replication_config_identifier().unwrap_or_default().to_string(),
            replication_type: c
                .replication_type()
                .map(|m| m.as_str().to_string())
                .unwrap_or_default(),
            source_endpoint: endpoint_names.get(&source).cloned(),
            target_endpoint: endpoint_names.get(&target).cloned(),
            source_endpoint_arn: source,
            target_endpoint_arn: target,
            min_dcu: cc.and_then(|c| c.min_capacity_units()),
            max_dcu: cc.and_then(|c| c.max_capacity_units()),
            multi_az: cc.and_then(|c| c.multi_az()),
            subnet_group: cc.and_then(|c| c.replication_subnet_group_id()).map(str::to_string),
            security_group_ids: cc.map(|c| c.vpc_security_group_ids().to_vec()).unwrap_or_default(),
            kms_key_id: cc.and_then(|c| c.kms_key_id()).map(str::to_string),
            created: c.replication_config_create_time().map(fmt_dt),
            table_mappings: c.table_mappings().map(str::to_string),
            settings: c.replication_settings().map(str::to_string),
            status: rep.and_then(|r| r.status()).map(str::to_string),
            stop_reason: rep
                .and_then(|r| r.stop_reason())
                .filter(|s| !s.is_empty())
                .map(str::to_string),
            failures: rep.map(|r| r.failure_messages().to_vec()).unwrap_or_default(),
            provision_state: rep
                .and_then(|r| r.provision_data())
                .and_then(|p| p.provision_state())
                .map(str::to_string),
            provisioned_dcu: rep
                .and_then(|r| r.provision_data())
                .map(|p| p.provisioned_capacity_units()),
            stats: rep.and_then(|r| r.replication_stats()).map(|s| DmsStats {
                full_load_pct: s.full_load_progress_percent(),
                elapsed_ms: s.elapsed_time_millis(),
                tables_loaded: s.tables_loaded(),
                tables_loading: s.tables_loading(),
                tables_queued: s.tables_queued(),
                tables_errored: s.tables_errored(),
                started: s.start_date().map(fmt_dt),
                stopped: s.stop_date().map(fmt_dt),
                full_load_started: s.full_load_start_date().map(fmt_dt),
                full_load_finished: s.full_load_finish_date().map(fmt_dt),
            }),
            cdc_start_position: rep.and_then(|r| r.cdc_start_position()).map(str::to_string),
            recovery_checkpoint: rep.and_then(|r| r.recovery_checkpoint()).map(str::to_string),
            last_stop: rep.and_then(|r| r.replication_last_stop_time()).map(fmt_dt),
            tags: HashMap::new(),
        }
    }

    fn stopped_on_error(&self) -> bool {
        self.status.as_deref() == Some("stopped")
            && (!self.failures.is_empty()
                || self
                    .stop_reason
                    .as_deref()
                    .is_some_and(|r| r.to_ascii_uppercase().contains("ERROR")))
    }
}

crate::sections! {
    pub enum DmsServerlessDetailSection,
    pub static DMS_SERVERLESS_SECTIONS = [
        Overview "Overview",
        Capacity "Capacity",
        Tables "Tables" => crate::app::App::trigger_dms_serverless_table_stats_load,
        Settings "Settings",
        Tags "Tags",
    ]
}

impl Resource for DmsServerless {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&DMS_SERVERLESS_SECTIONS)
    }

    fn security_group_ids(&self) -> Vec<String> {
        self.security_group_ids.clone()
    }

    fn references(&self) -> Vec<(String, String)> {
        let mut out = vec![
            ("Source Endpoint".to_string(), self.source_endpoint_arn.clone()),
            ("Target Endpoint".to_string(), self.target_endpoint_arn.clone()),
        ];
        out.retain(|(_, v)| !v.is_empty());
        out.extend(
            self.security_group_ids
                .iter()
                .map(|s| ("Security Group".to_string(), s.clone())),
        );
        if let Some(k) = &self.kms_key_id {
            out.push(("KMS Key".to_string(), k.clone()));
        }
        out
    }

    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws dms describe-replications --filters Name=replication-config-arn,Values={}",
            shell_quote(&self.arn)
        ))
    }

    fn cli_actions(&self) -> Vec<CliAction> {
        let arn = shell_quote(&self.arn);
        vec![
            CliAction::new(
                CliTier::Inspect,
                "describe-replication-table-statistics",
                format!(
                    "aws dms describe-replication-table-statistics --replication-config-arn {}",
                    arn
                ),
            ),
            CliAction::new(
                CliTier::Change,
                "start (resume-processing)",
                format!(
                    "aws dms start-replication --replication-config-arn {} --start-replication-type resume-processing",
                    arn
                ),
            ),
            CliAction::new(
                CliTier::Change,
                "stop-replication",
                format!("aws dms stop-replication --replication-config-arn {}", arn),
            ),
        ]
    }

    fn id(&self) -> &str {
        &self.arn
    }

    fn name(&self) -> &str {
        &self.identifier
    }

    fn resource_type(&self) -> &str {
        "DMS Serverless Replication"
    }

    fn state(&self) -> ResourceState {
        if self.stopped_on_error() {
            return ResourceState::Unavailable;
        }
        match self.status.as_deref() {
            None => ResourceState::stateless(),
            Some("running") => ResourceState::Running,
            Some("stopped") | Some("created") => ResourceState::Stopped,
            Some("failed") => ResourceState::Unavailable,
            Some("deleting") => ResourceState::Deleting,
            Some(other) => {
                if other.contains("ing") {
                    ResourceState::Pending
                } else {
                    ResourceState::Unknown(other.to_string())
                }
            }
        }
    }

    fn state_label(&self) -> String {
        if self.stopped_on_error() {
            return "stopped (error)".to_string();
        }
        match &self.status {
            Some(s) => native_state_label(s, || self.state()),
            None => "not started".to_string(),
        }
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!(
            "{} {} {} dms serverless replication",
            self.identifier,
            self.replication_type,
            self.status.as_deref().unwrap_or("")
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Replication".to_string(), self.identifier.clone()),
            ("Type".to_string(), self.replication_type.clone()),
            ("Status".to_string(), self.state_label()),
        ]
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{region}.console.aws.amazon.com/dms/v2/home?region={region}#replicationDetails/{}",
            self.identifier
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── Lazy section data ─────────────────────────────────────────────────────────

/// Cap on table-statistics rows per task (a big schema migration can have
/// thousands of tables; the pane is for spotting the broken ones).
pub const MAX_TABLE_STATS: usize = 1000;

/// One `TableStatistics` row, trimmed to what the Tables section renders.
#[derive(Debug, Clone, Default)]
pub struct DmsTableStat {
    pub schema: String,
    pub table: String,
    pub state: String,
    pub full_load_rows: i64,
    pub full_load_error_rows: i64,
    pub inserts: i64,
    pub updates: i64,
    pub deletes: i64,
    pub ddls: i64,
    pub validation_state: Option<String>,
    pub validation_failed: i64,
    pub last_update: Option<String>,
}

impl DmsTableStat {
    fn from_sdk(t: &aws_sdk_databasemigration::types::TableStatistics) -> Self {
        Self {
            schema: t.schema_name().unwrap_or_default().to_string(),
            table: t.table_name().unwrap_or_default().to_string(),
            state: t.table_state().unwrap_or_default().to_string(),
            full_load_rows: t.full_load_rows(),
            full_load_error_rows: t.full_load_error_rows(),
            inserts: t.inserts(),
            updates: t.updates(),
            deletes: t.deletes(),
            ddls: t.ddls(),
            validation_state: t.validation_state().map(str::to_string),
            validation_failed: t.validation_failed_records(),
            last_update: t.last_update_time().map(fmt_dt),
        }
    }

    /// DMS table states that mean "this table is broken".
    pub fn is_error(&self) -> bool {
        let s = self.state.to_ascii_lowercase();
        s.contains("error") || s.contains("failed") || self.full_load_error_rows > 0
    }
}

/// Table statistics for a lazy Tables section: rows (errors first) + whether
/// the cap cut the list.
#[derive(Debug, Clone, Default)]
pub struct DmsTableStats {
    pub tables: Vec<DmsTableStat>,
    pub truncated: bool,
}

fn finish_table_stats(mut tables: Vec<DmsTableStat>, truncated: bool) -> DmsTableStats {
    tables.sort_by(|a, b| {
        b.is_error()
            .cmp(&a.is_error())
            .then_with(|| a.schema.cmp(&b.schema))
            .then_with(|| a.table.cmp(&b.table))
    });
    DmsTableStats { tables, truncated }
}

/// `DescribeTableStatistics` for one provisioned task, capped.
pub async fn fetch_task_table_stats(client: DmsClient, task_arn: String) -> Result<DmsTableStats> {
    let mut out = Vec::new();
    let mut truncated = false;
    let mut pages = client
        .describe_table_statistics()
        .replication_task_arn(task_arn)
        .max_records(500)
        .into_paginator()
        .send();
    while let Some(page) = pages.next().await {
        let page = page.map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
        out.extend(page.table_statistics().iter().map(DmsTableStat::from_sdk));
        if out.len() >= MAX_TABLE_STATS {
            out.truncate(MAX_TABLE_STATS);
            truncated = true;
            break;
        }
    }
    Ok(finish_table_stats(out, truncated))
}

/// `DescribeReplicationTableStatistics` for one serverless replication, capped.
pub async fn fetch_serverless_table_stats(
    client: DmsClient,
    config_arn: String,
) -> Result<DmsTableStats> {
    let mut out = Vec::new();
    let mut truncated = false;
    let mut pages = client
        .describe_replication_table_statistics()
        .replication_config_arn(config_arn)
        .max_records(500)
        .into_paginator()
        .send();
    while let Some(page) = pages.next().await {
        let page = page.map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
        out.extend(page.replication_table_statistics().iter().map(DmsTableStat::from_sdk));
        if out.len() >= MAX_TABLE_STATS {
            out.truncate(MAX_TABLE_STATS);
            truncated = true;
            break;
        }
    }
    Ok(finish_table_stats(out, truncated))
}

/// One premigration assessment run for a task.
#[derive(Debug, Clone, Default)]
pub struct DmsAssessmentRun {
    pub name: String,
    pub status: String,
    pub created: Option<String>,
    pub latest: bool,
    pub progress: Option<(i32, i32)>, // (completed, total)
    pub passed: i32,
    pub failed: i32,
    pub error: i32,
    pub warning: i32,
    pub skipped: i32,
    pub last_failure: Option<String>,
    /// `s3://bucket/folder` of the full report.
    pub result_location: Option<String>,
}

/// `DescribeReplicationTaskAssessmentRuns` filtered to one task, newest first.
pub async fn fetch_task_assessments(
    client: DmsClient,
    task_arn: String,
) -> Result<Vec<DmsAssessmentRun>> {
    let filter = aws_sdk_databasemigration::types::Filter::builder()
        .name("replication-task-arn")
        .values(task_arn)
        .build()
        .map_err(|e| crate::error::Error::AwsSdk(e.to_string()))?;
    let mut runs: Vec<(i64, DmsAssessmentRun)> = Vec::new();
    let mut pages = client
        .describe_replication_task_assessment_runs()
        .filters(filter)
        .into_paginator()
        .send();
    while let Some(page) = pages.next().await {
        let page = page.map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
        for r in page.replication_task_assessment_runs() {
            let stat = r.result_statistic();
            let created_secs = r
                .replication_task_assessment_run_creation_date()
                .map(|d| d.secs())
                .unwrap_or(0);
            runs.push((
                created_secs,
                DmsAssessmentRun {
                    name: r
                        .assessment_run_name()
                        .unwrap_or_else(|| {
                            r.replication_task_assessment_run_arn()
                                .map(arn_resource_id)
                                .unwrap_or_default()
                        })
                        .to_string(),
                    status: r.status().unwrap_or("unknown").to_string(),
                    created: r.replication_task_assessment_run_creation_date().map(fmt_dt),
                    latest: r.is_latest_task_assessment_run(),
                    progress: r.assessment_progress().map(|p| {
                        (p.individual_assessment_completed_count(), p.individual_assessment_count())
                    }),
                    passed: stat.map(|s| s.passed()).unwrap_or(0),
                    failed: stat.map(|s| s.failed()).unwrap_or(0),
                    error: stat.map(|s| s.error()).unwrap_or(0),
                    warning: stat.map(|s| s.warning()).unwrap_or(0),
                    skipped: stat.map(|s| s.skipped()).unwrap_or(0),
                    last_failure: r.last_failure_message().filter(|s| !s.is_empty()).map(str::to_string),
                    result_location: r.result_location_bucket().map(|b| match r.result_location_folder() {
                        Some(f) if !f.is_empty() => format!("s3://{b}/{f}"),
                        _ => format!("s3://{b}"),
                    }),
                },
            ));
        }
    }
    runs.sort_by_key(|r| std::cmp::Reverse(r.0));
    Ok(runs.into_iter().map(|(_, r)| r).collect())
}

// ── Metrics (`m`) ─────────────────────────────────────────────────────────────

/// Which DMS resource the `m` overlay charts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DmsMetricsFlavor {
    Task,
    Instance,
}

#[derive(Debug, Clone)]
pub struct DmsMetricsData {
    pub time_range: MetricsTimeRange,
    pub flavor: DmsMetricsFlavor,
    /// Four `(title, series)` charts, laid out 2×2.
    pub series: Vec<(&'static str, Vec<(f64, f64)>)>,
    pub x_max: f64,
}

#[derive(Debug, Clone)]
pub enum DmsMetricsState {
    Loading,
    Loaded(DmsMetricsData),
}

/// Pull `AWS/DMS` metrics. A task is keyed by **two** dimensions —
/// `ReplicationInstanceIdentifier` (the instance's name) and
/// `ReplicationTaskIdentifier` (the task's opaque **resource id**, the ARN
/// suffix, not its user-facing identifier). An instance takes the first only.
pub async fn fetch_dms_metrics(
    cw: aws_sdk_cloudwatch::Client,
    flavor: DmsMetricsFlavor,
    instance_id: String,
    task_resource_id: Option<String>,
    time_range: MetricsTimeRange,
) -> Result<DmsMetricsData> {
    use aws_sdk_cloudwatch::types::{Dimension, Statistic};

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let start = now - time_range.duration_secs();
    let period = time_range.period_secs();
    let start_dt = aws_sdk_cloudwatch::primitives::DateTime::from_secs(start);
    let end_dt = aws_sdk_cloudwatch::primitives::DateTime::from_secs(now);

    let dims = || {
        let mut d = vec![Dimension::builder()
            .name("ReplicationInstanceIdentifier")
            .value(&instance_id)
            .build()];
        if let Some(t) = &task_resource_id {
            d.push(Dimension::builder().name("ReplicationTaskIdentifier").value(t).build());
        }
        d
    };
    let metric = |name: &'static str, stat: Statistic| {
        cw.get_metric_statistics()
            .namespace("AWS/DMS")
            .metric_name(name)
            .set_dimensions(Some(dims()))
            .start_time(start_dt)
            .end_time(end_dt)
            .period(period)
            .set_statistics(Some(vec![stat]))
            .send()
    };
    let parse = crate::aws::services::ec2::parse_metric_datapoints;
    let mb = |v: Vec<(f64, f64)>| v.into_iter().map(|(x, y)| (x, y / 1_048_576.0)).collect();

    let series = match flavor {
        DmsMetricsFlavor::Task => {
            let (src, tgt, incoming, full_load) = tokio::join!(
                metric("CDCLatencySource", Statistic::Average),
                metric("CDCLatencyTarget", Statistic::Average),
                metric("CDCIncomingChanges", Statistic::Maximum),
                metric("FullLoadThroughputRowsTarget", Statistic::Average),
            );
            vec![
                ("CDC Latency Source (s)", parse(src, start)),
                ("CDC Latency Target (s)", parse(tgt, start)),
                ("CDC Incoming Changes", parse(incoming, start)),
                ("Full Load Rows/s (target)", parse(full_load, start)),
            ]
        }
        DmsMetricsFlavor::Instance => {
            let (cpu, mem, storage, swap) = tokio::join!(
                metric("CPUUtilization", Statistic::Average),
                metric("FreeableMemory", Statistic::Average),
                metric("FreeStorageSpace", Statistic::Average),
                metric("SwapUsage", Statistic::Average),
            );
            vec![
                ("CPU %", parse(cpu, start)),
                ("Freeable Memory (MB)", mb(parse(mem, start))),
                ("Free Storage (MB)", mb(parse(storage, start))),
                ("Swap Usage (MB)", mb(parse(swap, start))),
            ]
        }
    };

    Ok(DmsMetricsData {
        time_range,
        flavor,
        series,
        x_max: time_range.duration_secs() as f64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(status: &str, stop: Option<&str>) -> DmsTask {
        let t = aws_sdk_databasemigration::types::ReplicationTask::builder()
            .replication_task_arn("arn:aws:dms:us-east-1:123456789012:task:ABCDEF123")
            .replication_task_identifier("orders-cdc")
            .replication_instance_arn("arn:aws:dms:us-east-1:123456789012:rep:INST1")
            .status(status)
            .set_stop_reason(stop.map(str::to_string))
            .replication_task_settings(r#"{"Logging":{"EnableLogging":true}}"#)
            .build();
        let instances = HashMap::from([(
            "arn:aws:dms:us-east-1:123456789012:rep:INST1".to_string(),
            "Prod-Repl-1".to_string(),
        )]);
        DmsTask::from_sdk(&t, &instances, &HashMap::new())
    }

    #[test]
    fn stopped_on_error_is_red_and_says_so() {
        let t = task("stopped", Some("Stop Reason FATAL_ERROR Error Level FATAL"));
        assert!(t.stopped_on_error());
        assert_eq!(t.state(), ResourceState::Unavailable);
        assert_eq!(t.state_label(), "stopped (error)");

        let t = task("stopped", Some("Stop Reason FULL_LOAD_ONLY_FINISHED"));
        assert!(!t.stopped_on_error());
        assert_eq!(t.state(), ResourceState::Stopped);
    }

    #[test]
    fn log_location_and_metrics_key_use_the_resource_id() {
        let t = task("running", None);
        assert_eq!(t.log_group().as_deref(), Some("dms-tasks-prod-repl-1"));
        assert_eq!(t.log_stream(), "dms-task-ABCDEF123");
        assert_eq!(arn_resource_id(&t.arn), "ABCDEF123");
        assert_eq!(t.logging_enabled, Some(true));
    }

    #[test]
    fn table_stats_put_errors_first() {
        let ok = DmsTableStat {
            schema: "a".into(),
            table: "t1".into(),
            state: "Table completed".into(),
            ..Default::default()
        };
        let bad = DmsTableStat {
            schema: "z".into(),
            table: "t2".into(),
            state: "Table error".into(),
            ..Default::default()
        };
        let s = finish_table_stats(vec![ok, bad], false);
        assert_eq!(s.tables[0].table, "t2");
    }
}
