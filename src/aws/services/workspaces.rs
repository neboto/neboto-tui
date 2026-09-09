use crate::aws::client::AwsClients;
use crate::aws::resource::{native_state_label, Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::aws::services::cloudwatch::fmt_epoch_secs;
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use aws_sdk_workspaces::Client as WorkspacesClient;
use std::any::Any;
use std::collections::HashMap;
use tokio::sync::mpsc;

pub struct WorkspacesService {
    client: WorkspacesClient,
}

impl WorkspacesService {
    pub fn new(aws_clients: &AwsClients) -> Self {
        Self {
            client: aws_clients.workspaces_client(),
        }
    }

    /// Resolve bundle ids → names up front (KMS-alias pattern). Queries both the
    /// account-owned/shared bundles (no owner) and the AWS-provided ones
    /// (`AMAZON`), merging both result sets. Tolerant of errors → sparse map.
    async fn fetch_bundle_map(&self) -> HashMap<String, String> {
        let mut map = HashMap::new();
        for owner in [None, Some("AMAZON")] {
            let mut token: Option<String> = None;
            loop {
                let mut req = self.client.describe_workspace_bundles();
                if let Some(o) = owner {
                    req = req.owner(o);
                }
                if let Some(t) = &token {
                    req = req.next_token(t);
                }
                let resp = match req.send().await {
                    Ok(r) => r,
                    Err(_) => break,
                };
                let next = crate::aws::pagination::next_page_token(resp.next_token(), &token);
                for b in resp.bundles() {
                    if let (Some(id), Some(name)) = (b.bundle_id(), b.name()) {
                        map.insert(id.to_string(), name.to_string());
                    }
                }
                match next {
                    Some(t) => token = Some(t),
                    None => break,
                }
            }
        }
        map
    }

    /// Resolve directory ids → alias/name up front. Tolerant of errors.
    async fn fetch_directory_map(&self) -> HashMap<String, String> {
        let mut map = HashMap::new();
        let mut token: Option<String> = None;
        loop {
            let mut req = self.client.describe_workspace_directories();
            if let Some(t) = &token {
                req = req.next_token(t);
            }
            let resp = match req.send().await {
                Ok(r) => r,
                Err(_) => break,
            };
            let next = crate::aws::pagination::next_page_token(resp.next_token(), &token);
            for d in resp.directories() {
                if let Some(id) = d.directory_id() {
                    // Prefer DirectoryName, fall back to Alias, then the raw id.
                    let name = d
                        .directory_name()
                        .filter(|s| !s.is_empty())
                        .or_else(|| d.alias().filter(|s| !s.is_empty()))
                        .unwrap_or(id);
                    map.insert(id.to_string(), name.to_string());
                }
            }
            match next {
                Some(t) => token = Some(t),
                None => break,
            }
        }
        map
    }
}

#[async_trait]
impl AwsService for WorkspacesService {
    fn service_type(&self) -> ServiceType {
        ServiceType::Workspaces
    }

    fn name(&self) -> &str {
        "WorkSpaces"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        self.list_resources_streaming(tx, ServiceType::Workspaces)
            .await?;
        Ok(vec![])
    }

    async fn list_resources_streaming(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        // Phase 1: resolve the bundle + directory name maps up front.
        let bundle_map = self.fetch_bundle_map().await;
        let directory_map = self.fetch_directory_map().await;

        // Phase 2: paginate describe_workspaces, resolving names from the maps.
        let mut token: Option<String> = None;
        let mut total = 0usize;

        loop {
            let mut req = self.client.describe_workspaces();
            if let Some(t) = &token {
                req = req.next_token(t);
            }
            let resp = match req.send().await {
                Ok(r) => r,
                Err(e) => {
                    let _ = event_tx.send(Event::ResourceLoadError {
                        service: service_type,
                        error: format!(
                            "Failed to list WorkSpaces: {}",
                            crate::error::sdk_error_message(&e)
                        ),
                    });
                    return Ok(());
                }
            };

            let next = crate::aws::pagination::next_page_token(resp.next_token(), &token);

            let batch: Vec<Box<dyn Resource>> = resp
                .workspaces()
                .iter()
                .map(|w| {
                    Box::new(Workspace::from_sdk(w, &bundle_map, &directory_map))
                        as Box<dyn Resource>
                })
                .collect();

            let count = batch.len();
            if count > 0 {
                total += count;
                let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                    service: service_type,
                    resources: batch,
                    progress: LoadProgress {
                        loaded_count: total,
                        total_count: None,
                        status_message: None,
                    },
                });
            }

            match next {
                Some(t) => token = Some(t),
                None => break,
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

// ── Workspace ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Workspace {
    pub id: String,
    pub user_name: String,
    pub directory_id: String,
    pub directory_alias: String,
    pub bundle_id: String,
    pub bundle_name: String,
    pub state: String,
    pub computer_name: String,
    pub ip_address: String,
    pub running_mode: String,
    pub running_mode_timeout_min: Option<i32>,
    pub compute_type: String,
    pub root_volume_gb: Option<i32>,
    pub user_volume_gb: Option<i32>,
    pub root_volume_encryption: bool,
    pub user_volume_encryption: bool,
    pub volume_encryption_key: Option<String>,
    pub subnet_id: String,
    pub modification_states: Vec<String>,
    pub tags: HashMap<String, String>,
}

impl Workspace {
    pub fn from_sdk(
        w: &aws_sdk_workspaces::types::Workspace,
        bundle_map: &HashMap<String, String>,
        directory_map: &HashMap<String, String>,
    ) -> Self {
        let bundle_id = w.bundle_id().unwrap_or_default().to_string();
        let directory_id = w.directory_id().unwrap_or_default().to_string();

        let props = w.workspace_properties();
        let running_mode = props
            .and_then(|p| p.running_mode())
            .map(|m| m.as_str().to_string())
            .unwrap_or_default();
        let running_mode_timeout_min =
            props.and_then(|p| p.running_mode_auto_stop_timeout_in_minutes());
        let compute_type = props
            .and_then(|p| p.compute_type_name())
            .map(|c| c.as_str().to_string())
            .unwrap_or_default();
        let root_volume_gb = props.and_then(|p| p.root_volume_size_gib());
        let user_volume_gb = props.and_then(|p| p.user_volume_size_gib());

        let modification_states = w
            .modification_states()
            .iter()
            .map(|m| {
                let res = m.resource().map(|r| r.as_str()).unwrap_or("?");
                let st = m.state().map(|s| s.as_str()).unwrap_or("?");
                format!("{}: {}", res, st)
            })
            .collect();

        Self {
            id: w.workspace_id().unwrap_or_default().to_string(),
            user_name: w.user_name().unwrap_or_default().to_string(),
            directory_alias: directory_map
                .get(&directory_id)
                .cloned()
                .unwrap_or_else(|| directory_id.clone()),
            directory_id,
            bundle_name: bundle_map
                .get(&bundle_id)
                .cloned()
                .unwrap_or_else(|| bundle_id.clone()),
            bundle_id,
            state: w
                .state()
                .map(|s| s.as_str().to_string())
                .unwrap_or_default(),
            computer_name: w.computer_name().unwrap_or_default().to_string(),
            ip_address: w.ip_address().unwrap_or_default().to_string(),
            running_mode,
            running_mode_timeout_min,
            compute_type,
            root_volume_gb,
            user_volume_gb,
            root_volume_encryption: w.root_volume_encryption_enabled().unwrap_or(false),
            user_volume_encryption: w.user_volume_encryption_enabled().unwrap_or(false),
            volume_encryption_key: w
                .volume_encryption_key()
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string()),
            subnet_id: w.subnet_id().unwrap_or_default().to_string(),
            modification_states,
            tags: HashMap::new(),
        }
    }
}

crate::sections! {
    pub enum WorkspaceDetailSection,
    pub static WORKSPACE_SECTIONS = [
        Details "Details",
        Connection "Connection" => crate::app::App::trigger_workspace_connection_load,
        Tags "Tags" => crate::app::App::trigger_workspace_tags_load,
    ]
}

impl Resource for Workspace {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&WORKSPACE_SECTIONS)
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws workspaces describe-workspaces --workspace-ids {}",
            crate::aws::resource::shell_quote(self.id())
        ))
    }

    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        if !self.user_name.is_empty() {
            &self.user_name
        } else {
            &self.id
        }
    }

    fn resource_type(&self) -> &str {
        "WorkSpace"
    }

    fn state(&self) -> ResourceState {
        match self.state.as_str() {
            "AVAILABLE" => ResourceState::Available,
            "STOPPED" | "SUSPENDED" => ResourceState::Stopped,
            "ERROR" | "UNHEALTHY" | "IMPAIRED" => ResourceState::Unavailable,
            "PENDING" | "REBUILDING" | "STARTING" | "RESTORING" | "MIGRATING" | "UPDATING"
            | "REBOOTING" | "STOPPING" | "MAINTENANCE" | "ADMIN_MAINTENANCE" => {
                ResourceState::Pending
            }
            "TERMINATING" | "TERMINATED" => ResourceState::Deleting,
            other => ResourceState::Unknown(other.to_string()),
        }
    }

    fn state_label(&self) -> String {
        native_state_label(&self.state, || self.state())
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!(
            "{} {} {} {} {} {} {}",
            self.id,
            self.user_name,
            self.computer_name,
            self.ip_address,
            self.directory_alias,
            self.bundle_name,
            self.state,
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        let dash = || "—".to_string();
        vec![
            ("WorkSpace ID".to_string(), self.id.clone()),
            ("User".to_string(), self.user_name.clone()),
            ("Directory".to_string(), self.directory_alias.clone()),
            ("Bundle".to_string(), self.bundle_name.clone()),
            ("State".to_string(), self.state.clone()),
            ("Running Mode".to_string(), self.running_mode.clone()),
            ("Compute Type".to_string(), self.compute_type.clone()),
            ("IP Address".to_string(), self.ip_address.clone()),
            ("Computer Name".to_string(), self.computer_name.clone()),
            (
                "Root Volume (GiB)".to_string(),
                self.root_volume_gb
                    .map(|v| v.to_string())
                    .unwrap_or_else(dash),
            ),
            (
                "User Volume (GiB)".to_string(),
                self.user_volume_gb
                    .map(|v| v.to_string())
                    .unwrap_or_else(dash),
            ),
            (
                "Root Volume Encrypted".to_string(),
                self.root_volume_encryption.to_string(),
            ),
            (
                "User Volume Encrypted".to_string(),
                self.user_volume_encryption.to_string(),
            ),
        ]
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/workspaces/v2/workspaces?region={}#/workspaces/{}",
            region, region, self.id
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── Lazy-loaded state types ───────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct WorkspaceConnection {
    pub connection_state: String,
    pub last_active: Option<String>,
    pub last_known_user_connection: Option<String>,
}

// ── Lazy fetch functions ──────────────────────────────────────────────────────

/// Fetch a workspace's live connection status via
/// `DescribeWorkspacesConnectionStatus(WorkspaceIds=[id])`.
pub async fn fetch_workspace_connection(
    client: WorkspacesClient,
    id: String,
) -> Result<WorkspaceConnection> {
    let resp = client
        .describe_workspaces_connection_status()
        .workspace_ids(&id)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;

    let status = resp
        .workspaces_connection_status()
        .iter()
        .find(|s| s.workspace_id() == Some(id.as_str()))
        .or_else(|| resp.workspaces_connection_status().first());

    let conn = match status {
        Some(s) => WorkspaceConnection {
            connection_state: s
                .connection_state()
                .map(|c| c.as_str().to_string())
                .unwrap_or_else(|| "UNKNOWN".to_string()),
            last_active: s
                .connection_state_check_timestamp()
                .map(|d| fmt_epoch_secs(d.secs())),
            last_known_user_connection: s
                .last_known_user_connection_timestamp()
                .map(|d| fmt_epoch_secs(d.secs())),
        },
        None => WorkspaceConnection {
            connection_state: "UNKNOWN".to_string(),
            last_active: None,
            last_known_user_connection: None,
        },
    };
    Ok(conn)
}

/// Fetch a workspace's tags via `DescribeTags(ResourceId=id)`. NOTE: WorkSpaces
/// `DescribeTags` takes the **workspace id**, not an ARN.
pub async fn fetch_workspace_tags(
    client: WorkspacesClient,
    id: String,
) -> Result<HashMap<String, String>> {
    let resp = client
        .describe_tags()
        .resource_id(&id)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;

    let mut tags = HashMap::new();
    for t in resp.tag_list() {
        tags.insert(
            t.key().to_string(),
            t.value().unwrap_or_default().to_string(),
        );
    }
    Ok(tags)
}

// ── CloudWatch metrics (`m` overlay) — AWS/WorkSpaces, dimension WorkspaceId ──

pub use crate::aws::services::ec2::{parse_metric_datapoints, MetricsTimeRange};

#[derive(Debug, Clone)]
pub struct WsMetricsData {
    pub time_range: MetricsTimeRange,
    pub connection_success: Vec<(f64, f64)>,
    pub connection_failure: Vec<(f64, f64)>,
    pub user_connected: Vec<(f64, f64)>,
    pub in_session_latency: Vec<(f64, f64)>,
    pub session_launch_time: Vec<(f64, f64)>,
    pub unhealthy: Vec<(f64, f64)>,
    pub x_max: f64,
}

#[derive(Debug, Clone)]
pub enum WsMetricsState {
    Loading,
    Loaded(WsMetricsData),
}

/// Pull `AWS/WorkSpaces` metrics for one workspace. Connection success/failure
/// and Unhealthy are the "can the user get in" signals; in-session latency and
/// session launch time are the experience signals.
pub async fn fetch_workspace_metrics(
    cw: aws_sdk_cloudwatch::Client,
    workspace_id: String,
    time_range: MetricsTimeRange,
) -> crate::error::Result<WsMetricsData> {
    use aws_sdk_cloudwatch::types::{Dimension, Statistic};

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let start = now - time_range.duration_secs();
    let period = time_range.period_secs();
    let start_dt = aws_sdk_cloudwatch::primitives::DateTime::from_secs(start);
    let end_dt = aws_sdk_cloudwatch::primitives::DateTime::from_secs(now);

    let metric = |name: &'static str, stat: Statistic| {
        cw.get_metric_statistics()
            .namespace("AWS/WorkSpaces")
            .metric_name(name)
            .dimensions(
                Dimension::builder()
                    .name("WorkspaceId")
                    .value(&workspace_id)
                    .build(),
            )
            .start_time(start_dt)
            .end_time(end_dt)
            .period(period)
            .set_statistics(Some(vec![stat]))
            .send()
    };

    let (conn_ok, conn_fail, connected, latency, launch, unhealthy) = tokio::join!(
        metric("ConnectionSuccess", Statistic::Sum),
        metric("ConnectionFailure", Statistic::Sum),
        metric("UserConnected", Statistic::Maximum),
        metric("InSessionLatency", Statistic::Average),
        metric("SessionLaunchTime", Statistic::Average),
        metric("Unhealthy", Statistic::Maximum),
    );

    Ok(WsMetricsData {
        time_range,
        connection_success: parse_metric_datapoints(conn_ok, start),
        connection_failure: parse_metric_datapoints(conn_fail, start),
        user_connected: parse_metric_datapoints(connected, start),
        in_session_latency: parse_metric_datapoints(latency, start),
        session_launch_time: parse_metric_datapoints(launch, start),
        unhealthy: parse_metric_datapoints(unhealthy, start),
        x_max: time_range.duration_secs() as f64,
    })
}
