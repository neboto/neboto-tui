use crate::aws::cli_actions::{CliAction, CliTier};
use crate::aws::client::AwsClients;
use crate::aws::resource::{native_state_label, shell_quote, Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use aws_sdk_datasync::Client as DataSyncClient;
use futures::stream::StreamExt;
use std::any::Any;
use std::collections::HashMap;
use tokio::sync::mpsc;

/// AWS DataSync — three sub-tabs: **Tasks** (the primary view: "did last
/// night's transfer run, and what did it move?"), **Locations** and
/// **Agents**. One region-scoped client.
///
/// Load order: locations first (`ListLocations` already carries the URI, and
/// the URIs label the task rows), then agents (`ListAgents` + `DescribeAgent`
/// each), then tasks (`ListTasks` + `DescribeTask` each, streamed in chunks).
/// Tags arrive at list time from the Tagging API (`ListTimeTags` in
/// `build_services`) — DataSync's own `ListTagsForResource` is per ARN.
///
/// Lazy per-row sections: a task's **Executions** (`ListTaskExecutions` +
/// `DescribeTaskExecution` for the most recent ones) and a location's
/// **Configuration** (the `DescribeLocation<Type>` call its URI scheme
/// selects). A location's Tasks section filters sibling rows — no fetch.
///
/// Credentials: location describes can return a user name and an object
/// storage access key id next to Secrets Manager references. Only the secret
/// **ARNs** are shown; nothing is resolved.
pub struct DataSyncService {
    client: DataSyncClient,
}

impl DataSyncService {
    pub fn new(aws_clients: &AwsClients) -> Self {
        Self {
            client: aws_clients.datasync_client(),
        }
    }
}

fn fmt_dt(dt: &aws_smithy_types::DateTime) -> String {
    crate::aws::services::cloudwatch::fmt_epoch_secs(dt.secs())
}

/// Last `/`-segment of an ARN: `…:task/task-0abc` → `task-0abc`, an EC2
/// `…:security-group/sg-0abc` → `sg-0abc` (which the id-prefix jump follows).
pub fn arn_tail(arn: &str) -> &str {
    arn.rsplit('/').next().unwrap_or(arn)
}

/// `DescribeTask` / `DescribeAgent` fan-out. DataSync's control plane
/// throttles early; adaptive retry paces the rest.
const DESCRIBE_FANOUT: usize = 5;

/// Tasks are streamed in chunks of this many as their describes finish.
const TASK_CHUNK: usize = 25;

/// Most executions described for a task's Executions section.
pub const MAX_EXECUTIONS: usize = 30;

#[async_trait]
impl AwsService for DataSyncService {
    fn service_type(&self) -> ServiceType {
        ServiceType::DataSync
    }

    fn name(&self) -> &str {
        "DataSync"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        self.list_resources_streaming(tx, ServiceType::DataSync).await?;
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
        let mut loaded = 0usize;
        let mut send = |batch: Vec<Box<dyn Resource>>, status: Option<&str>| {
            loaded += batch.len();
            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                service: service_type,
                resources: batch,
                progress: LoadProgress {
                    loaded_count: loaded,
                    total_count: None,
                    status_message: status.map(str::to_string),
                },
            });
        };

        // ── Locations ─────────────────────────────────────────────────────────
        let mut locations: Vec<DataSyncLocation> = Vec::new();
        let mut locations_failed = None;
        let mut pages = self.client.list_locations().into_paginator().send();
        while let Some(page) = pages.next().await {
            match page {
                Ok(p) => locations.extend(p.locations().iter().filter_map(DataSyncLocation::from_sdk)),
                Err(e) => {
                    locations_failed = Some(crate::error::sdk_error_message(&e));
                    break;
                }
            }
        }
        let location_uris: HashMap<String, String> =
            locations.iter().map(|l| (l.arn.clone(), l.uri.clone())).collect();

        // ── Agents ────────────────────────────────────────────────────────────
        let mut agent_entries = Vec::new();
        let mut agents_failed = None;
        let mut pages = self.client.list_agents().into_paginator().send();
        while let Some(page) = pages.next().await {
            match page {
                Ok(p) => agent_entries.extend(p.agents().iter().cloned()),
                Err(e) => {
                    agents_failed = Some(crate::error::sdk_error_message(&e));
                    break;
                }
            }
        }

        // ── Task list ─────────────────────────────────────────────────────────
        let mut task_entries = Vec::new();
        let mut tasks_failed = None;
        let mut pages = self.client.list_tasks().into_paginator().send();
        while let Some(page) = pages.next().await {
            match page {
                Ok(p) => task_entries.extend(p.tasks().iter().cloned()),
                Err(e) => {
                    tasks_failed = Some(crate::error::sdk_error_message(&e));
                    break;
                }
            }
        }

        // Every list failed: nothing can stream (the usual cause is a missing
        // datasync:List* permission, or a region without DataSync).
        if let (Some(t), Some(_), Some(_)) = (&tasks_failed, &locations_failed, &agents_failed) {
            let _ = event_tx.send(Event::ResourceLoadError {
                service: service_type,
                error: format!("DataSync unavailable: {t}"),
            });
            return Ok(());
        }
        for (what, err) in [
            ("tasks", &tasks_failed),
            ("locations", &locations_failed),
            ("agents", &agents_failed),
        ] {
            if let Some(e) = err {
                warn(format!("DataSync {what}: {e}"));
            }
        }

        // ── Agent details ─────────────────────────────────────────────────────
        let client = &self.client;
        let described: Vec<(DataSyncAgent, Option<String>)> = futures::stream::iter(agent_entries)
            .map(|entry| async move {
                let arn = entry.agent_arn().unwrap_or_default().to_string();
                match client.describe_agent().agent_arn(&arn).send().await {
                    Ok(out) => (DataSyncAgent::from_describe(&out, &arn), None),
                    Err(e) => (DataSyncAgent::from_list_entry(&entry), Some(crate::error::sdk_error_message(&e))),
                }
            })
            .buffered(DESCRIBE_FANOUT)
            .collect()
            .await;
        let mut agents = Vec::with_capacity(described.len());
        let mut agent_errors = Vec::new();
        for (agent, err) in described {
            agents.push(agent);
            agent_errors.extend(err);
        }
        if let Some(first) = agent_errors.first() {
            warn(format!(
                "DataSync agent details: {} of {} failed ({first})",
                agent_errors.len(),
                agents.len()
            ));
        }
        agents.sort_by(|a, b| a.name.cmp(&b.name));
        locations.sort_by(|a, b| a.uri.cmp(&b.uri));

        let mut first: Vec<Box<dyn Resource>> = Vec::new();
        first.extend(locations.into_iter().map(|r| Box::new(r) as Box<dyn Resource>));
        first.extend(agents.into_iter().map(|r| Box::new(r) as Box<dyn Resource>));
        if !first.is_empty() || !task_entries.is_empty() {
            send(first, (!task_entries.is_empty()).then_some("Loading tasks…"));
        }

        // ── Task details, streamed ────────────────────────────────────────────
        let uris = &location_uris;
        let total_tasks = task_entries.len();
        let mut task_errors: Vec<String> = Vec::new();
        let mut described = futures::stream::iter(task_entries)
            .map(|entry| async move {
                let arn = entry.task_arn().unwrap_or_default().to_string();
                match client.describe_task().task_arn(&arn).send().await {
                    Ok(out) => (DataSyncTask::from_describe(&out, &arn, uris), None),
                    Err(e) => (DataSyncTask::from_list_entry(&entry), Some(crate::error::sdk_error_message(&e))),
                }
            })
            .buffered(DESCRIBE_FANOUT)
            .chunks(TASK_CHUNK);
        while let Some(chunk) = described.next().await {
            let mut batch: Vec<Box<dyn Resource>> = Vec::with_capacity(chunk.len());
            for (task, err) in chunk {
                task_errors.extend(err);
                batch.push(Box::new(task));
            }
            send(batch, None);
        }
        if let Some(first) = task_errors.first() {
            warn(format!(
                "DataSync task details: {} of {total_tasks} failed ({first})",
                task_errors.len()
            ));
        }

        let _ = event_tx.send(Event::ResourcesFullyLoaded {
            service: service_type,
            total_count: loaded,
        });
        Ok(())
    }

    async fn get_resource_details(&self, _id: &str) -> Result<Box<dyn Resource>> {
        Err(crate::error::Error::NotImplemented)
    }
}

// ── Task ──────────────────────────────────────────────────────────────────────

/// The `Options` block as display rows, in the console's order. Only what
/// the API returned — an Enhanced-mode task leaves several unset.
fn option_rows(o: &aws_sdk_datasync::types::Options) -> Vec<(String, String)> {
    let mut rows = Vec::new();
    let mut put = |k: &str, v: Option<&str>| {
        if let Some(v) = v {
            rows.push((k.to_string(), v.to_string()));
        }
    };
    put("Transfer Mode", o.transfer_mode().map(|v| v.as_str()));
    put("Verify Mode", o.verify_mode().map(|v| v.as_str()));
    put("Overwrite Mode", o.overwrite_mode().map(|v| v.as_str()));
    put("Preserve Deleted Files", o.preserve_deleted_files().map(|v| v.as_str()));
    put("Task Queueing", o.task_queueing().map(|v| v.as_str()));
    put("Log Level", o.log_level().map(|v| v.as_str()));
    put("Object Tags", o.object_tags().map(|v| v.as_str()));
    put("Atime", o.atime().map(|v| v.as_str()));
    put("Mtime", o.mtime().map(|v| v.as_str()));
    put("UID", o.uid().map(|v| v.as_str()));
    put("GID", o.gid().map(|v| v.as_str()));
    put("POSIX Permissions", o.posix_permissions().map(|v| v.as_str()));
    put("Preserve Devices", o.preserve_devices().map(|v| v.as_str()));
    put(
        "Security Descriptor",
        o.security_descriptor_copy_flags().map(|v| v.as_str()),
    );
    let bps = o.bytes_per_second().map(|b| {
        if b < 0 {
            "unlimited".to_string()
        } else {
            format!("{}/s", crate::aws::services::backup::fmt_bytes(b))
        }
    });
    put("Bandwidth Limit", bps.as_deref());
    rows
}

/// Filter rules → their patterns. A rule's value is one `|`-delimited
/// string; every rule is kept (there is normally one per list).
fn filter_patterns(rules: &[aws_sdk_datasync::types::FilterRule]) -> Vec<String> {
    rules
        .iter()
        .filter_map(|r| r.value())
        .flat_map(|v| v.split('|'))
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .collect()
}

#[derive(Debug, Clone, Default)]
pub struct DataSyncTask {
    pub arn: String,
    pub name: String,
    pub status: String,
    pub mode: String,
    pub source_arn: String,
    pub destination_arn: String,
    /// Location URIs resolved from the locations list at load time.
    pub source_uri: Option<String>,
    pub destination_uri: Option<String>,
    pub log_group_arn: Option<String>,
    pub current_execution_arn: Option<String>,
    pub error_code: Option<String>,
    pub error_detail: Option<String>,
    pub created: Option<String>,
    pub schedule: Option<String>,
    pub schedule_status: Option<String>,
    pub schedule_disabled_reason: Option<String>,
    pub schedule_disabled_by: Option<String>,
    pub options: Vec<(String, String)>,
    pub includes: Vec<String>,
    pub excludes: Vec<String>,
    pub manifest: Option<String>,
    /// `(level, output type, S3 bucket ARN, prefix)` of the task report.
    pub report: Option<(String, String, String, String)>,
    pub source_enis: Vec<String>,
    pub destination_enis: Vec<String>,
    /// False when `DescribeTask` failed and only the list entry is known.
    pub described: bool,
    pub tags: HashMap<String, String>,
}

impl DataSyncTask {
    pub fn from_list_entry(e: &aws_sdk_datasync::types::TaskListEntry) -> Self {
        Self {
            arn: e.task_arn().unwrap_or_default().to_string(),
            name: e.name().unwrap_or_default().to_string(),
            status: e.status().map(|s| s.as_str().to_string()).unwrap_or_default(),
            mode: e.task_mode().map(|m| m.as_str().to_string()).unwrap_or_default(),
            ..Default::default()
        }
    }

    pub fn from_describe(
        t: &aws_sdk_datasync::operation::describe_task::DescribeTaskOutput,
        arn: &str,
        location_uris: &HashMap<String, String>,
    ) -> Self {
        let source_arn = t.source_location_arn().unwrap_or_default().to_string();
        let destination_arn = t.destination_location_arn().unwrap_or_default().to_string();
        let details = t.schedule_details();
        Self {
            arn: t.task_arn().unwrap_or(arn).to_string(),
            name: t.name().unwrap_or_default().to_string(),
            status: t.status().map(|s| s.as_str().to_string()).unwrap_or_default(),
            mode: t.task_mode().map(|m| m.as_str().to_string()).unwrap_or_default(),
            source_uri: location_uris.get(&source_arn).cloned(),
            destination_uri: location_uris.get(&destination_arn).cloned(),
            source_arn,
            destination_arn,
            log_group_arn: t.cloud_watch_log_group_arn().filter(|s| !s.is_empty()).map(str::to_string),
            current_execution_arn: t.current_task_execution_arn().map(str::to_string),
            error_code: t.error_code().filter(|s| !s.is_empty()).map(str::to_string),
            error_detail: t.error_detail().filter(|s| !s.is_empty()).map(str::to_string),
            created: t.creation_time().map(fmt_dt),
            schedule: t.schedule().map(|s| s.schedule_expression().to_string()),
            schedule_status: t
                .schedule()
                .and_then(|s| s.status())
                .map(|s| s.as_str().to_string()),
            schedule_disabled_reason: details
                .and_then(|d| d.disabled_reason())
                .filter(|s| !s.is_empty())
                .map(str::to_string),
            schedule_disabled_by: details.and_then(|d| d.disabled_by()).map(|b| b.as_str().to_string()),
            options: t.options().map(option_rows).unwrap_or_default(),
            includes: filter_patterns(t.includes()),
            excludes: filter_patterns(t.excludes()),
            manifest: t.manifest_config().map(|m| {
                format!(
                    "{} · {}",
                    m.action().map(|a| a.as_str()).unwrap_or("—"),
                    m.format().map(|f| f.as_str()).unwrap_or("—")
                )
            }),
            report: t.task_report_config().map(|r| {
                let s3 = r.destination().and_then(|d| d.s3());
                (
                    r.report_level().map(|l| l.as_str().to_string()).unwrap_or_default(),
                    r.output_type().map(|o| o.as_str().to_string()).unwrap_or_default(),
                    s3.map(|s| s.s3_bucket_arn().to_string()).unwrap_or_default(),
                    s3.and_then(|s| s.subdirectory()).unwrap_or_default().to_string(),
                )
            }),
            source_enis: t.source_network_interface_arns().to_vec(),
            destination_enis: t.destination_network_interface_arns().to_vec(),
            described: true,
            tags: HashMap::new(),
        }
    }

    /// `task-0abc…` — the `TaskId` metrics dimension and the console path.
    pub fn task_id(&self) -> &str {
        arn_tail(&self.arn)
    }

    /// A schedule that DataSync (not the user) switched off — it does this
    /// after repeated failures, and the task still reads AVAILABLE.
    pub fn schedule_disabled_by_service(&self) -> bool {
        self.schedule_status.as_deref() == Some("DISABLED")
            && self.schedule_disabled_by.as_deref() == Some("SERVICE")
    }

    /// Log group name from the task's `CloudWatchLogGroupArn`
    /// (`arn:…:log-group:/aws/datasync:*`).
    pub fn log_group(&self) -> Option<String> {
        let arn = self.log_group_arn.as_deref()?;
        let name = arn.split_once(":log-group:")?.1;
        let name = name.strip_suffix(":*").unwrap_or(name);
        (!name.is_empty()).then(|| name.to_string())
    }
}

crate::sections! {
    pub enum DataSyncTaskDetailSection,
    pub static DATASYNC_TASK_SECTIONS = [
        Overview "Overview" => crate::app::App::trigger_datasync_executions_load,
        Executions "Executions" => crate::app::App::trigger_datasync_executions_load,
        Options "Options",
        Tags "Tags",
    ]
}

impl Resource for DataSyncTask {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&DATASYNC_TASK_SECTIONS)
    }

    fn references(&self) -> Vec<(String, String)> {
        let mut out = vec![
            ("Source Location".to_string(), self.source_arn.clone()),
            ("Destination Location".to_string(), self.destination_arn.clone()),
        ];
        if let Some(g) = &self.log_group_arn {
            out.push(("Log Group".to_string(), g.clone()));
        }
        out.extend(
            self.source_enis
                .iter()
                .chain(&self.destination_enis)
                .map(|e| ("Network Interface".to_string(), arn_tail(e).to_string())),
        );
        out.retain(|(_, v)| !v.is_empty());
        out
    }

    fn cli_command(&self) -> Option<String> {
        Some(format!("aws datasync describe-task --task-arn {}", shell_quote(&self.arn)))
    }

    fn cli_actions(&self) -> Vec<CliAction> {
        let arn = shell_quote(&self.arn);
        let mut actions = vec![
            CliAction::new(
                CliTier::Inspect,
                "list-task-executions",
                format!("aws datasync list-task-executions --task-arn {arn}"),
            ),
            CliAction::new(
                CliTier::Change,
                "start-task-execution",
                format!("aws datasync start-task-execution --task-arn {arn}"),
            )
            .with_note("runs the transfer with the task's saved options, filters and overwrite/delete settings"),
        ];
        if let Some(exec) = &self.current_execution_arn {
            actions.push(CliAction::new(
                CliTier::Change,
                "cancel-task-execution",
                format!(
                    "aws datasync cancel-task-execution --task-execution-arn {}",
                    shell_quote(exec)
                ),
            ));
        }
        actions
    }

    fn id(&self) -> &str {
        &self.arn
    }

    fn name(&self) -> &str {
        if self.name.is_empty() {
            self.task_id()
        } else {
            &self.name
        }
    }

    fn resource_type(&self) -> &str {
        "DataSync Task"
    }

    fn state(&self) -> ResourceState {
        match self.status.as_str() {
            "AVAILABLE" => ResourceState::Available,
            "RUNNING" => ResourceState::Running,
            "QUEUED" => ResourceState::Pending,
            "CREATING" => ResourceState::Creating,
            "UNAVAILABLE" => ResourceState::Unavailable,
            other => ResourceState::Unknown(other.to_lowercase()),
        }
    }

    fn state_label(&self) -> String {
        native_state_label(&self.status, || self.state())
    }

    fn list_summary(&self) -> Option<String> {
        match (&self.source_uri, &self.destination_uri) {
            (Some(s), Some(d)) => Some(format!("{s} → {d}")),
            _ => Some(self.task_id().to_string()),
        }
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn list_tags_slot(&mut self) -> Option<(String, &mut HashMap<String, String>)> {
        Some((self.arn.clone(), &mut self.tags))
    }

    fn search_text(&self) -> String {
        format!(
            "{} {} {} {} {} {} {} datasync task",
            self.name,
            self.arn,
            self.status,
            self.mode,
            self.source_uri.as_deref().unwrap_or(""),
            self.destination_uri.as_deref().unwrap_or(""),
            self.schedule.as_deref().unwrap_or(""),
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Task".to_string(), self.name().to_string()),
            ("Status".to_string(), self.state_label()),
            ("ARN".to_string(), self.arn.clone()),
        ]
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{region}.console.aws.amazon.com/datasync/home?region={region}#/tasks/{}",
            self.task_id()
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── Location ──────────────────────────────────────────────────────────────────

/// What a location is, from its URI scheme — `ListLocations` has no type
/// field, and the scheme is also what picks the `DescribeLocation*` call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocationKind {
    S3,
    Efs,
    Nfs,
    Smb,
    Hdfs,
    ObjectStorage,
    AzureBlob,
    FsxLustre,
    FsxWindows,
    FsxOpenZfs,
    FsxOntap,
    Other,
}

impl LocationKind {
    pub fn from_uri(uri: &str) -> Self {
        match uri.split_once("://").map(|(s, _)| s).unwrap_or("") {
            "s3" => Self::S3,
            "efs" => Self::Efs,
            "nfs" => Self::Nfs,
            "smb" => Self::Smb,
            "hdfs" => Self::Hdfs,
            "object-storage" => Self::ObjectStorage,
            "azure-blob" => Self::AzureBlob,
            "fsxl" => Self::FsxLustre,
            "fsxw" => Self::FsxWindows,
            "fsxz" => Self::FsxOpenZfs,
            s if s.starts_with("fsxn") => Self::FsxOntap,
            _ => Self::Other,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::S3 => "S3",
            Self::Efs => "EFS",
            Self::Nfs => "NFS",
            Self::Smb => "SMB",
            Self::Hdfs => "HDFS",
            Self::ObjectStorage => "Object storage",
            Self::AzureBlob => "Azure Blob",
            Self::FsxLustre => "FSx for Lustre",
            Self::FsxWindows => "FSx for Windows",
            Self::FsxOpenZfs => "FSx for OpenZFS",
            Self::FsxOntap => "FSx for ONTAP",
            Self::Other => "Other",
        }
    }

    /// The CLI's `describe-location-*` suffix; `None` for a scheme this
    /// build doesn't know.
    pub fn describe_suffix(self) -> Option<&'static str> {
        Some(match self {
            Self::S3 => "s3",
            Self::Efs => "efs",
            Self::Nfs => "nfs",
            Self::Smb => "smb",
            Self::Hdfs => "hdfs",
            Self::ObjectStorage => "object-storage",
            Self::AzureBlob => "azure-blob",
            Self::FsxLustre => "fsx-lustre",
            Self::FsxWindows => "fsx-windows",
            Self::FsxOpenZfs => "fsx-open-zfs",
            Self::FsxOntap => "fsx-ontap",
            Self::Other => return None,
        })
    }
}

#[derive(Debug, Clone)]
pub struct DataSyncLocation {
    pub arn: String,
    pub uri: String,
    pub kind: LocationKind,
    pub tags: HashMap<String, String>,
}

impl DataSyncLocation {
    pub fn from_sdk(l: &aws_sdk_datasync::types::LocationListEntry) -> Option<Self> {
        let arn = l.location_arn()?.to_string();
        let uri = l.location_uri().unwrap_or_default().to_string();
        Some(Self {
            kind: LocationKind::from_uri(&uri),
            arn,
            uri,
            tags: HashMap::new(),
        })
    }

    pub fn location_id(&self) -> &str {
        arn_tail(&self.arn)
    }
}

crate::sections! {
    pub enum DataSyncLocationDetailSection,
    pub static DATASYNC_LOCATION_SECTIONS = [
        Overview "Overview",
        Configuration "Configuration" => crate::app::App::trigger_datasync_location_config_load,
        Tasks "Tasks",
        Tags "Tags",
    ]
}

impl Resource for DataSyncLocation {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&DATASYNC_LOCATION_SECTIONS)
    }

    fn cli_command(&self) -> Option<String> {
        let suffix = self.kind.describe_suffix()?;
        Some(format!(
            "aws datasync describe-location-{suffix} --location-arn {}",
            shell_quote(&self.arn)
        ))
    }

    fn id(&self) -> &str {
        &self.arn
    }

    fn name(&self) -> &str {
        if self.uri.is_empty() {
            self.location_id()
        } else {
            &self.uri
        }
    }

    fn resource_type(&self) -> &str {
        "DataSync Location"
    }

    fn state(&self) -> ResourceState {
        ResourceState::stateless()
    }

    fn list_summary(&self) -> Option<String> {
        Some(format!("{} · {}", self.kind.label(), self.location_id()))
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn list_tags_slot(&mut self) -> Option<(String, &mut HashMap<String, String>)> {
        Some((self.arn.clone(), &mut self.tags))
    }

    fn search_text(&self) -> String {
        format!("{} {} {} datasync location", self.uri, self.arn, self.kind.label())
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Location".to_string(), self.uri.clone()),
            ("Type".to_string(), self.kind.label().to_string()),
            ("ARN".to_string(), self.arn.clone()),
        ]
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{region}.console.aws.amazon.com/datasync/home?region={region}#/locations/{}",
            self.location_id()
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── Agent ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct DataSyncAgent {
    pub arn: String,
    pub name: String,
    pub status: String,
    pub platform_version: Option<String>,
    pub endpoint_type: Option<String>,
    pub last_connection: Option<String>,
    pub created: Option<String>,
    pub vpc_endpoint_id: Option<String>,
    pub private_link_endpoint: Option<String>,
    /// Subnet / security group **ids** (the API returns ARNs).
    pub subnets: Vec<String>,
    pub security_groups: Vec<String>,
    pub tags: HashMap<String, String>,
}

impl DataSyncAgent {
    pub fn from_list_entry(e: &aws_sdk_datasync::types::AgentListEntry) -> Self {
        Self {
            arn: e.agent_arn().unwrap_or_default().to_string(),
            name: e.name().unwrap_or_default().to_string(),
            status: e.status().map(|s| s.as_str().to_string()).unwrap_or_default(),
            platform_version: e.platform().and_then(|p| p.version()).map(str::to_string),
            ..Default::default()
        }
    }

    pub fn from_describe(a: &aws_sdk_datasync::operation::describe_agent::DescribeAgentOutput, arn: &str) -> Self {
        let pl = a.private_link_config();
        let ids = |arns: &[String]| arns.iter().map(|a| arn_tail(a).to_string()).collect::<Vec<_>>();
        Self {
            arn: a.agent_arn().unwrap_or(arn).to_string(),
            name: a.name().unwrap_or_default().to_string(),
            status: a.status().map(|s| s.as_str().to_string()).unwrap_or_default(),
            platform_version: a.platform().and_then(|p| p.version()).map(str::to_string),
            endpoint_type: a.endpoint_type().map(|e| e.as_str().to_string()),
            last_connection: a.last_connection_time().map(fmt_dt),
            created: a.creation_time().map(fmt_dt),
            vpc_endpoint_id: pl.and_then(|p| p.vpc_endpoint_id()).map(str::to_string),
            private_link_endpoint: pl.and_then(|p| p.private_link_endpoint()).map(str::to_string),
            subnets: pl.map(|p| ids(p.subnet_arns())).unwrap_or_default(),
            security_groups: pl.map(|p| ids(p.security_group_arns())).unwrap_or_default(),
            tags: HashMap::new(),
        }
    }

    pub fn agent_id(&self) -> &str {
        arn_tail(&self.arn)
    }
}

crate::sections! {
    pub enum DataSyncAgentDetailSection,
    pub static DATASYNC_AGENT_SECTIONS = [
        Overview "Overview",
        Tags "Tags",
    ]
}

impl Resource for DataSyncAgent {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&DATASYNC_AGENT_SECTIONS)
    }

    fn security_group_ids(&self) -> Vec<String> {
        self.security_groups.clone()
    }

    fn references(&self) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = self
            .security_groups
            .iter()
            .map(|id| ("Security Group".to_string(), id.clone()))
            .collect();
        out.extend(self.subnets.iter().map(|s| ("Subnet".to_string(), s.clone())));
        if let Some(v) = &self.vpc_endpoint_id {
            out.push(("VPC Endpoint".to_string(), v.clone()));
        }
        out
    }

    fn cli_command(&self) -> Option<String> {
        Some(format!("aws datasync describe-agent --agent-arn {}", shell_quote(&self.arn)))
    }

    fn id(&self) -> &str {
        &self.arn
    }

    fn name(&self) -> &str {
        if self.name.is_empty() {
            self.agent_id()
        } else {
            &self.name
        }
    }

    fn resource_type(&self) -> &str {
        "DataSync Agent"
    }

    fn state(&self) -> ResourceState {
        match self.status.as_str() {
            "ONLINE" => ResourceState::Available,
            "OFFLINE" => ResourceState::Unavailable,
            other => ResourceState::Unknown(other.to_lowercase()),
        }
    }

    fn state_label(&self) -> String {
        native_state_label(&self.status, || self.state())
    }

    fn list_summary(&self) -> Option<String> {
        Some(match &self.endpoint_type {
            Some(e) => format!("{} · {}", self.agent_id(), e.to_lowercase()),
            None => self.agent_id().to_string(),
        })
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn list_tags_slot(&mut self) -> Option<(String, &mut HashMap<String, String>)> {
        Some((self.arn.clone(), &mut self.tags))
    }

    fn search_text(&self) -> String {
        format!(
            "{} {} {} {} datasync agent",
            self.name,
            self.arn,
            self.status,
            self.endpoint_type.as_deref().unwrap_or(""),
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Agent".to_string(), self.name().to_string()),
            ("Status".to_string(), self.state_label()),
            ("ARN".to_string(), self.arn.clone()),
        ]
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{region}.console.aws.amazon.com/datasync/home?region={region}#/agents/{}",
            self.agent_id()
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── Lazy: task executions ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct DataSyncExecution {
    pub arn: String,
    pub status: String,
    /// Epoch seconds of the start, for the newest-first sort.
    pub start_secs: i64,
    pub started: Option<String>,
    pub ended: Option<String>,
    pub files_transferred: i64,
    pub files_estimated: i64,
    pub bytes_transferred: i64,
    pub bytes_estimated: i64,
    pub bytes_written: i64,
    pub files_verified: i64,
    pub files_skipped: i64,
    pub files_deleted: i64,
    /// Files that failed in any phase (Enhanced mode only).
    pub files_failed: i64,
    /// `(phase, status, duration ms)` for prepare / transfer / verify.
    pub phases: Vec<(&'static str, String, Option<i64>)>,
    pub total_duration_ms: Option<i64>,
    pub error_code: Option<String>,
    pub error_detail: Option<String>,
    pub report_status: Option<String>,
    pub report_error: Option<String>,
}

impl DataSyncExecution {
    pub fn execution_id(&self) -> &str {
        arn_tail(&self.arn)
    }

    pub fn is_error(&self) -> bool {
        self.status == "ERROR" || self.error_code.is_some()
    }

    pub fn is_active(&self) -> bool {
        !matches!(self.status.as_str(), "SUCCESS" | "ERROR")
    }
}

#[derive(Debug, Clone, Default)]
pub struct DataSyncExecutions {
    /// Newest first.
    pub runs: Vec<DataSyncExecution>,
    /// How many executions the task lists in all (`runs` is capped at
    /// [`MAX_EXECUTIONS`]).
    pub total: usize,
}

/// A task's execution history: every listed ARN, then `DescribeTaskExecution`
/// for the most recent [`MAX_EXECUTIONS`] — the list entry has only a status.
pub async fn fetch_task_executions(client: DataSyncClient, task_arn: String) -> Result<DataSyncExecutions> {
    let sdk = |m: String| crate::error::Error::AwsSdk(m);
    let mut arns: Vec<String> = Vec::new();
    let mut pages = client.list_task_executions().task_arn(&task_arn).into_paginator().send();
    while let Some(page) = pages.next().await {
        let page = page.map_err(|e| sdk(crate::error::sdk_error_message(&e)))?;
        arns.extend(
            page.task_executions()
                .iter()
                .filter_map(|e| e.task_execution_arn().map(str::to_string)),
        );
    }
    let total = arns.len();
    // Assumes the list is oldest first (undocumented, see SERVICES.md), so
    // the tail is the recent history. Only matters past MAX_EXECUTIONS.
    let recent: Vec<String> = arns.into_iter().rev().take(MAX_EXECUTIONS).collect();
    let client = &client;
    let described: Vec<std::result::Result<DataSyncExecution, String>> = futures::stream::iter(recent)
        .map(|arn| async move {
            let out = client
                .describe_task_execution()
                .task_execution_arn(&arn)
                .send()
                .await
                .map_err(|e| crate::error::sdk_error_message(&e))?;
            let res = out.result();
            let phase = |name: &'static str,
                         status: Option<&aws_sdk_datasync::types::PhaseStatus>,
                         dur: Option<i64>| {
                status.map(|s| (name, s.as_str().to_string(), dur))
            };
            let failed = out
                .files_failed()
                .map(|f| f.prepare() + f.transfer() + f.verify() + f.delete())
                .unwrap_or(0);
            let report = out.report_result();
            Ok(DataSyncExecution {
                arn,
                status: out.status().map(|s| s.as_str().to_string()).unwrap_or_default(),
                start_secs: out.start_time().map(|t| t.secs()).unwrap_or(0),
                started: out.start_time().map(fmt_dt),
                ended: out.end_time().map(fmt_dt),
                files_transferred: out.files_transferred(),
                files_estimated: out.estimated_files_to_transfer(),
                bytes_transferred: out.bytes_transferred(),
                bytes_estimated: out.estimated_bytes_to_transfer(),
                bytes_written: out.bytes_written(),
                files_verified: out.files_verified(),
                files_skipped: out.files_skipped(),
                files_deleted: out.files_deleted(),
                files_failed: failed,
                phases: res
                    .map(|r| {
                        [
                            phase("Prepare", r.prepare_status(), r.prepare_duration()),
                            phase("Transfer", r.transfer_status(), r.transfer_duration()),
                            phase("Verify", r.verify_status(), r.verify_duration()),
                        ]
                        .into_iter()
                        .flatten()
                        .collect()
                    })
                    .unwrap_or_default(),
                total_duration_ms: res.and_then(|r| r.total_duration()),
                error_code: res
                    .and_then(|r| r.error_code())
                    .filter(|s| !s.is_empty())
                    .map(str::to_string),
                error_detail: res
                    .and_then(|r| r.error_detail())
                    .filter(|s| !s.is_empty())
                    .map(str::to_string),
                report_status: report.and_then(|r| r.status()).map(|s| s.as_str().to_string()),
                report_error: report
                    .and_then(|r| r.error_detail())
                    .filter(|s| !s.is_empty())
                    .map(str::to_string),
            })
        })
        .buffered(DESCRIBE_FANOUT)
        .collect()
        .await;
    let mut runs = Vec::with_capacity(described.len());
    let mut first_err = None;
    for d in described {
        match d {
            Ok(r) => runs.push(r),
            Err(e) => first_err = first_err.or(Some(e)),
        }
    }
    // Some executions described: show them. None did: that's the error.
    if runs.is_empty() {
        if let Some(e) = first_err {
            return Err(sdk(e));
        }
    }
    runs.sort_by_key(|r| std::cmp::Reverse(r.start_secs));
    Ok(DataSyncExecutions { runs, total })
}

// ── Lazy: location configuration ──────────────────────────────────────────────

/// Display rows for a location's Configuration section. Built here rather
/// than in the renderer because each of the eleven describe outputs is its
/// own type; the rows are the common shape.
pub type LocationConfig = Vec<(String, String)>;

struct ConfigRows(LocationConfig);

impl ConfigRows {
    fn put(&mut self, key: &str, value: impl Into<String>) {
        let value = value.into();
        if !value.is_empty() {
            self.0.push((key.to_string(), value));
        }
    }
    fn opt(&mut self, key: &str, value: Option<&str>) {
        if let Some(v) = value {
            self.put(key, v);
        }
    }
    fn created(&mut self, dt: Option<&aws_smithy_types::DateTime>) {
        if let Some(dt) = dt {
            self.put("Created", fmt_dt(dt));
        }
    }
    /// One row per ARN, full ARN kept (agents jump by it).
    fn arns(&mut self, key: &str, arns: &[String]) {
        for a in arns {
            self.put(key, a.clone());
        }
    }
    /// One row per EC2 ARN, reduced to its id so the `sg-` / `subnet-`
    /// jump follows it.
    fn ec2_ids(&mut self, key: &str, arns: &[String]) {
        for a in arns {
            self.put(key, arn_tail(a));
        }
    }
    /// Secret **references** only — never resolved.
    fn secrets(
        &mut self,
        managed: Option<&aws_sdk_datasync::types::ManagedSecretConfig>,
        cmk: Option<&aws_sdk_datasync::types::CmkSecretConfig>,
        custom: Option<&aws_sdk_datasync::types::CustomSecretConfig>,
    ) {
        self.opt("Managed Secret", managed.and_then(|m| m.secret_arn()));
        self.opt("Secret", cmk.and_then(|c| c.secret_arn()));
        self.opt("Secret KMS Key", cmk.and_then(|c| c.kms_key_arn()));
        self.opt("Secret", custom.and_then(|c| c.secret_arn()));
        self.opt("Secret Access Role", custom.and_then(|c| c.secret_access_role_arn()));
    }
}

/// The `DescribeLocation<Type>` call for `kind`, flattened to rows.
pub async fn fetch_location_config(
    client: DataSyncClient,
    arn: String,
    kind: LocationKind,
) -> Result<LocationConfig> {
    let sdk = |m: String| crate::error::Error::AwsSdk(m);
    let mut r = ConfigRows(Vec::new());
    match kind {
        LocationKind::S3 => {
            let o = client
                .describe_location_s3()
                .location_arn(&arn)
                .send()
                .await
                .map_err(|e| sdk(crate::error::sdk_error_message(&e)))?;
            r.opt("Storage Class", o.s3_storage_class().map(|c| c.as_str()));
            r.opt("Bucket Access Role", o.s3_config().map(|c| c.bucket_access_role_arn()));
            r.arns("Agent", o.agent_arns());
            r.created(o.creation_time());
        }
        LocationKind::Efs => {
            let o = client
                .describe_location_efs()
                .location_arn(&arn)
                .send()
                .await
                .map_err(|e| sdk(crate::error::sdk_error_message(&e)))?;
            r.opt("Access Point", o.access_point_arn());
            r.opt("File System Access Role", o.file_system_access_role_arn());
            r.opt("In-Transit Encryption", o.in_transit_encryption().map(|e| e.as_str()));
            if let Some(ec2) = o.ec2_config() {
                r.put("Subnet", arn_tail(ec2.subnet_arn()));
                r.ec2_ids("Security Group", ec2.security_group_arns());
            }
            r.created(o.creation_time());
        }
        LocationKind::Nfs => {
            let o = client
                .describe_location_nfs()
                .location_arn(&arn)
                .send()
                .await
                .map_err(|e| sdk(crate::error::sdk_error_message(&e)))?;
            r.opt(
                "NFS Version",
                o.mount_options().and_then(|m| m.version()).map(|v| v.as_str()),
            );
            if let Some(c) = o.on_prem_config() {
                r.arns("Agent", c.agent_arns());
            }
            r.created(o.creation_time());
        }
        LocationKind::Smb => {
            let o = client
                .describe_location_smb()
                .location_arn(&arn)
                .send()
                .await
                .map_err(|e| sdk(crate::error::sdk_error_message(&e)))?;
            r.opt("Authentication", o.authentication_type().map(|a| a.as_str()));
            r.opt("Domain", o.domain());
            r.opt("Kerberos Principal", o.kerberos_principal());
            r.opt(
                "SMB Version",
                o.mount_options().and_then(|m| m.version()).map(|v| v.as_str()),
            );
            for ip in o.dns_ip_addresses() {
                r.put("DNS Server", ip.clone());
            }
            r.arns("Agent", o.agent_arns());
            r.secrets(o.managed_secret_config(), o.cmk_secret_config(), o.custom_secret_config());
            r.created(o.creation_time());
        }
        LocationKind::Hdfs => {
            let o = client
                .describe_location_hdfs()
                .location_arn(&arn)
                .send()
                .await
                .map_err(|e| sdk(crate::error::sdk_error_message(&e)))?;
            for n in o.name_nodes() {
                r.put("Name Node", format!("{}:{}", n.hostname(), n.port()));
            }
            r.opt("Authentication", o.authentication_type().map(|a| a.as_str()));
            r.opt("Kerberos Principal", o.kerberos_principal());
            if let Some(b) = o.block_size() {
                r.put("Block Size", crate::aws::services::backup::fmt_bytes(b as i64));
            }
            if let Some(f) = o.replication_factor() {
                r.put("Replication Factor", f.to_string());
            }
            r.opt("KMS Key Provider", o.kms_key_provider_uri());
            r.arns("Agent", o.agent_arns());
            r.secrets(o.managed_secret_config(), o.cmk_secret_config(), o.custom_secret_config());
            r.created(o.creation_time());
        }
        LocationKind::ObjectStorage => {
            let o = client
                .describe_location_object_storage()
                .location_arn(&arn)
                .send()
                .await
                .map_err(|e| sdk(crate::error::sdk_error_message(&e)))?;
            r.opt("Protocol", o.server_protocol().map(|p| p.as_str()));
            if let Some(p) = o.server_port() {
                r.put("Port", p.to_string());
            }
            r.put(
                "Custom Certificate",
                if o.server_certificate().is_some() { "✓ set" } else { "" },
            );
            r.arns("Agent", o.agent_arns());
            r.secrets(o.managed_secret_config(), o.cmk_secret_config(), o.custom_secret_config());
            r.created(o.creation_time());
        }
        LocationKind::AzureBlob => {
            let o = client
                .describe_location_azure_blob()
                .location_arn(&arn)
                .send()
                .await
                .map_err(|e| sdk(crate::error::sdk_error_message(&e)))?;
            r.opt("Authentication", o.authentication_type().map(|a| a.as_str()));
            r.opt("Blob Type", o.blob_type().map(|b| b.as_str()));
            r.opt("Access Tier", o.access_tier().map(|t| t.as_str()));
            r.arns("Agent", o.agent_arns());
            r.secrets(o.managed_secret_config(), o.cmk_secret_config(), o.custom_secret_config());
            r.created(o.creation_time());
        }
        LocationKind::FsxLustre => {
            let o = client
                .describe_location_fsx_lustre()
                .location_arn(&arn)
                .send()
                .await
                .map_err(|e| sdk(crate::error::sdk_error_message(&e)))?;
            r.ec2_ids("Security Group", o.security_group_arns());
            r.created(o.creation_time());
        }
        LocationKind::FsxWindows => {
            let o = client
                .describe_location_fsx_windows()
                .location_arn(&arn)
                .send()
                .await
                .map_err(|e| sdk(crate::error::sdk_error_message(&e)))?;
            r.opt("Domain", o.domain());
            r.ec2_ids("Security Group", o.security_group_arns());
            r.secrets(o.managed_secret_config(), o.cmk_secret_config(), o.custom_secret_config());
            r.created(o.creation_time());
        }
        LocationKind::FsxOpenZfs => {
            let o = client
                .describe_location_fsx_open_zfs()
                .location_arn(&arn)
                .send()
                .await
                .map_err(|e| sdk(crate::error::sdk_error_message(&e)))?;
            r.put(
                "Protocol",
                if o.protocol().is_some_and(|p| p.nfs().is_some()) { "NFS" } else { "" },
            );
            r.ec2_ids("Security Group", o.security_group_arns());
            r.created(o.creation_time());
        }
        LocationKind::FsxOntap => {
            let o = client
                .describe_location_fsx_ontap()
                .location_arn(&arn)
                .send()
                .await
                .map_err(|e| sdk(crate::error::sdk_error_message(&e)))?;
            r.put(
                "Protocol",
                match o.protocol() {
                    Some(p) if p.smb().is_some() => "SMB",
                    Some(p) if p.nfs().is_some() => "NFS",
                    _ => "",
                },
            );
            r.opt("File System", o.fsx_filesystem_arn());
            r.opt("Storage Virtual Machine", o.storage_virtual_machine_arn());
            r.ec2_ids("Security Group", o.security_group_arns());
            r.created(o.creation_time());
        }
        LocationKind::Other => {}
    }
    Ok(r.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn location_kind_follows_the_uri_scheme() {
        for (uri, want) in [
            ("s3://bucket/prefix/", LocationKind::S3),
            ("efs://us-east-1.fs-0abc/", LocationKind::Efs),
            ("nfs://nas.corp.example/export", LocationKind::Nfs),
            ("smb://files.corp.example/share", LocationKind::Smb),
            ("hdfs://nn1:8020/data", LocationKind::Hdfs),
            ("object-storage://minio.example/bucket", LocationKind::ObjectStorage),
            ("azure-blob://acct.blob.core.windows.net/c", LocationKind::AzureBlob),
            ("fsxl://us-east-1.fs-0abc/", LocationKind::FsxLustre),
            ("fsxw://us-east-1.fs-0abc/share", LocationKind::FsxWindows),
            ("fsxz://us-east-1.fs-0abc/fsx", LocationKind::FsxOpenZfs),
            ("fsxn-nfs://us-east-1.svm-0abc/vol", LocationKind::FsxOntap),
            ("fsxn-smb://us-east-1.svm-0abc/share", LocationKind::FsxOntap),
            ("gopher://x", LocationKind::Other),
            ("", LocationKind::Other),
        ] {
            assert_eq!(LocationKind::from_uri(uri), want, "{uri}");
        }
    }

    #[test]
    fn log_group_name_comes_from_the_arn() {
        let mut t = DataSyncTask::default();
        assert_eq!(t.log_group(), None);
        t.log_group_arn = Some("arn:aws:logs:us-east-1:123456789012:log-group:/aws/datasync:*".to_string());
        assert_eq!(t.log_group().as_deref(), Some("/aws/datasync"));
        t.log_group_arn = Some("arn:aws:logs:us-east-1:123456789012:log-group:/aws/datasync".to_string());
        assert_eq!(t.log_group().as_deref(), Some("/aws/datasync"));
    }

    #[test]
    fn filter_rules_split_on_the_pipe_and_keep_every_rule() {
        use aws_sdk_datasync::types::FilterRule;
        let rules = [
            FilterRule::builder().value("/tmp|*.bak").build(),
            FilterRule::builder().value("/cache").build(),
        ];
        assert_eq!(filter_patterns(&rules), ["/tmp", "*.bak", "/cache"]);
    }
}
