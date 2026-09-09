use crate::aws::client::AwsClients;
use crate::aws::resource::{native_state_label, Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use aws_sdk_transfer::Client as TransferClient;
use futures::stream::StreamExt;
use std::any::Any;
use std::collections::HashMap;
use tokio::sync::mpsc;

/// AWS Transfer Family — single-list of SFTP/FTPS/FTP/AS2 servers with a split
/// detail pane (Details / Users / Tags). `ListServers` returns summaries only;
/// the protocols, endpoint detail, security policy, host key, logging role, and
/// tags come from a per-server `DescribeServer` (N+1, bounded concurrency).
/// Users hang off the server detail pane as a lazy section (`ListUsers` then a
/// per-user `DescribeUser`) — a server usually has a handful.
pub struct TransferService {
    client: TransferClient,
}

impl TransferService {
    pub fn new(aws_clients: &AwsClients) -> Self {
        Self {
            client: aws_clients.transfer_client(),
        }
    }
}

#[async_trait]
impl AwsService for TransferService {
    fn service_type(&self) -> ServiceType {
        ServiceType::Transfer
    }

    fn name(&self) -> &str {
        "Transfer Family"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        self.list_resources_streaming(tx, ServiceType::Transfer).await?;
        Ok(vec![])
    }

    async fn list_resources_streaming(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        let mut total = 0usize;
        let mut token: Option<String> = None;

        loop {
            let mut req = self.client.list_servers();
            if let Some(t) = &token {
                req = req.next_token(t);
            }
            let page = match req.send().await {
                Ok(p) => p,
                Err(e) => {
                    let _ = event_tx.send(Event::ResourceLoadError {
                        service: service_type,
                        error: format!(
                            "Failed to list Transfer servers: {}",
                            crate::error::sdk_error_message(&e)
                        ),
                    });
                    return Ok(());
                }
            };

            let server_ids: Vec<String> = page
                .servers()
                .iter()
                .filter_map(|s| s.server_id().map(|id| id.to_string()))
                .collect();

            // Describe each server for the full config (N+1), bounded to 8 in flight.
            let client = self.client.clone();
            let described: Vec<TransferServer> = futures::stream::iter(server_ids)
                .map(|id| {
                    let client = client.clone();
                    async move {
                        match client.describe_server().server_id(&id).send().await {
                            Ok(resp) => resp.server().map(TransferServer::from_sdk),
                            Err(_) => None,
                        }
                    }
                })
                .buffer_unordered(8)
                .filter_map(|r| async move { r })
                .collect()
                .await;

            if !described.is_empty() {
                let batch: Vec<Box<dyn Resource>> = described
                    .into_iter()
                    .map(|s| Box::new(s) as Box<dyn Resource>)
                    .collect();
                total += batch.len();
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

            token = crate::aws::pagination::next_page_token(page.next_token(), &token);
            if token.is_none() {
                break;
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

// ── TransferServer ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct TransferServer {
    pub server_id: String,
    pub arn: String,
    pub protocols: Vec<String>,
    pub endpoint_type: String,
    pub endpoint_vpc_id: Option<String>,
    pub endpoint_subnet_ids: Vec<String>,
    pub endpoint_address_allocation_ids: Vec<String>,
    pub endpoint_vpc_endpoint_id: Option<String>,
    pub domain: String,
    pub identity_provider_type: String,
    pub state: String,
    pub logging_role: Option<String>,
    pub security_policy_name: Option<String>,
    pub host_key_fingerprint: Option<String>,
    pub user_count: i32,
    pub structured_log_destinations: Vec<String>,
    pub tags: HashMap<String, String>,
}

impl TransferServer {
    pub fn from_sdk(s: &aws_sdk_transfer::types::DescribedServer) -> Self {
        let mut tags: HashMap<String, String> = HashMap::new();
        for t in s.tags() {
            tags.insert(t.key().to_string(), t.value().to_string());
        }

        let endpoint = s.endpoint_details();
        let (vpc_id, subnet_ids, alloc_ids, vpc_endpoint_id) = match endpoint {
            Some(e) => (
                e.vpc_id().map(|v| v.to_string()),
                e.subnet_ids().to_vec(),
                e.address_allocation_ids().to_vec(),
                e.vpc_endpoint_id().map(|v| v.to_string()),
            ),
            None => (None, Vec::new(), Vec::new(), None),
        };

        Self {
            server_id: s.server_id().unwrap_or_default().to_string(),
            arn: s.arn().to_string(),
            protocols: s
                .protocols()
                .iter()
                .map(|p| p.as_str().to_string())
                .collect(),
            endpoint_type: s
                .endpoint_type()
                .map(|e| e.as_str().to_string())
                .unwrap_or_default(),
            endpoint_vpc_id: vpc_id,
            endpoint_subnet_ids: subnet_ids,
            endpoint_address_allocation_ids: alloc_ids,
            endpoint_vpc_endpoint_id: vpc_endpoint_id,
            domain: s
                .domain()
                .map(|d| d.as_str().to_string())
                .unwrap_or_default(),
            identity_provider_type: s
                .identity_provider_type()
                .map(|i| i.as_str().to_string())
                .unwrap_or_default(),
            state: s
                .state()
                .map(|st| st.as_str().to_string())
                .unwrap_or_default(),
            logging_role: s.logging_role().map(|r| r.to_string()),
            security_policy_name: s.security_policy_name().map(|p| p.to_string()),
            host_key_fingerprint: s.host_key_fingerprint().map(|h| h.to_string()),
            user_count: s.user_count().unwrap_or(0),
            structured_log_destinations: s.structured_log_destinations().to_vec(),
            tags,
        }
    }

    /// Server name = a `Name` tag if present, else the server id (Transfer
    /// servers have no native name).
    pub fn display_name(&self) -> &str {
        self.tags
            .get("Name")
            .map(|s| s.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or(&self.server_id)
    }
}

crate::sections! {
    pub enum TransferServerDetailSection,
    pub static TRANSFER_SERVER_SECTIONS = [
        Details "Details",
        Users "Users" => crate::app::App::trigger_transfer_users_load,
        Tags "Tags",
    ]
}

impl Resource for TransferServer {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&TRANSFER_SERVER_SECTIONS)
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws transfer describe-server --server-id {}",
            crate::aws::resource::shell_quote(self.id())
        ))
    }

    fn id(&self) -> &str {
        &self.server_id
    }

    fn name(&self) -> &str {
        self.display_name()
    }

    fn resource_type(&self) -> &str {
        "Transfer Server"
    }

    fn state(&self) -> ResourceState {
        match self.state.as_str() {
            "ONLINE" => ResourceState::Available,
            "OFFLINE" => ResourceState::Stopped,
            "START_FAILED" | "STOP_FAILED" => ResourceState::Unavailable,
            "STARTING" | "STOPPING" => ResourceState::Pending,
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
            "{} {} {} {} {} {}",
            self.server_id,
            self.arn,
            self.protocols.join(" "),
            self.endpoint_type,
            self.domain,
            self.identity_provider_type,
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Server ID".to_string(), self.server_id.clone()),
            ("Protocols".to_string(), self.protocols.join(", ")),
            ("Endpoint Type".to_string(), self.endpoint_type.clone()),
            ("Domain".to_string(), self.domain.clone()),
            (
                "Identity Provider".to_string(),
                self.identity_provider_type.clone(),
            ),
            ("State".to_string(), self.state.clone()),
            (
                "Security Policy".to_string(),
                self.security_policy_name.clone().unwrap_or_default(),
            ),
            ("Users".to_string(), self.user_count.to_string()),
        ]
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/transfer/home?region={}#/servers/{}",
            region, region, self.server_id
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── TransferUser (lazy child) ─────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct TransferUser {
    pub user_name: String,
    pub role: Option<String>,
    pub home_directory_type: Option<String>,
    pub home_directory: Option<String>,
    pub home_directory_mappings: usize,
    pub ssh_public_key_count: usize,
    pub policy: Option<String>,
}

/// `ListUsers` for a server then a per-user `DescribeUser` (N+1, bounded
/// concurrency). AS2 servers may have no users — that's fine, returns empty.
pub async fn fetch_transfer_users(
    client: TransferClient,
    server_id: String,
) -> Result<Vec<TransferUser>> {
    // Phase 1: collect all user names (paginated).
    let mut user_names: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut req = client.list_users().server_id(&server_id);
        if let Some(t) = &token {
            req = req.next_token(t);
        }
        let page = req
            .send()
            .await
            .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
        for u in page.users() {
            if let Some(name) = u.user_name() {
                user_names.push(name.to_string());
            }
        }
        token = crate::aws::pagination::next_page_token(page.next_token(), &token);
        if token.is_none() {
            break;
        }
    }

    // Phase 2: describe each user (bounded concurrency).
    let server = server_id.clone();
    let users: Vec<TransferUser> = futures::stream::iter(user_names)
        .map(|name| {
            let client = client.clone();
            let server = server.clone();
            async move {
                match client
                    .describe_user()
                    .server_id(&server)
                    .user_name(&name)
                    .send()
                    .await
                {
                    Ok(resp) => resp.user().map(|u| TransferUser {
                        user_name: u.user_name().unwrap_or(&name).to_string(),
                        role: u.role().map(|r| r.to_string()),
                        home_directory_type: u
                            .home_directory_type()
                            .map(|t| t.as_str().to_string()),
                        home_directory: u.home_directory().map(|h| h.to_string()),
                        home_directory_mappings: u.home_directory_mappings().len(),
                        ssh_public_key_count: u.ssh_public_keys().len(),
                        policy: u
                            .policy()
                            .filter(|p| !p.is_empty())
                            .map(|p| p.to_string()),
                    }),
                    Err(_) => None,
                }
            }
        })
        .buffer_unordered(8)
        .filter_map(|r| async move { r })
        .collect()
        .await;

    Ok(users)
}

// ── CloudWatch metrics (`m` overlay) — AWS/Transfer, dimension ServerId ────────

pub use crate::aws::services::ec2::MetricsTimeRange;

#[derive(Debug, Clone)]
pub struct TransferMetricsData {
    pub time_range: MetricsTimeRange,
    pub bytes_in: Vec<(f64, f64)>,
    pub bytes_out: Vec<(f64, f64)>,
    pub files_in: Vec<(f64, f64)>,
    pub files_out: Vec<(f64, f64)>,
    pub uploads_ok: Vec<(f64, f64)>,
    pub uploads_failed: Vec<(f64, f64)>,
    pub x_max: f64,
}

#[derive(Debug, Clone)]
pub enum TransferMetricsState {
    Loading,
    Loaded(TransferMetricsData),
}

/// Pull `AWS/Transfer` throughput metrics for one server. Bytes/files transferred
/// are the headline usage signals; on-upload workflow success/failure counts
/// surface post-upload automation health (empty when no workflow is attached).
pub async fn fetch_transfer_metrics(
    cw: aws_sdk_cloudwatch::Client,
    server_id: String,
    time_range: MetricsTimeRange,
) -> Result<TransferMetricsData> {
    use aws_sdk_cloudwatch::types::{Dimension, Statistic};

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let start = now - time_range.duration_secs();
    let period = time_range.period_secs();
    let start_dt = aws_sdk_cloudwatch::primitives::DateTime::from_secs(start);
    let end_dt = aws_sdk_cloudwatch::primitives::DateTime::from_secs(now);

    let dim = || Dimension::builder().name("ServerId").value(&server_id).build();
    let metric = |name: &'static str| {
        cw.get_metric_statistics()
            .namespace("AWS/Transfer")
            .metric_name(name)
            .dimensions(dim())
            .start_time(start_dt)
            .end_time(end_dt)
            .period(period)
            .set_statistics(Some(vec![Statistic::Sum]))
            .send()
    };

    let (bytes_in, bytes_out, files_in, files_out, uploads_ok, uploads_failed) = tokio::join!(
        metric("BytesIn"),
        metric("BytesOut"),
        metric("FilesIn"),
        metric("FilesOut"),
        metric("OnUploadExecutionsSuccess"),
        metric("OnUploadExecutionsFailed"),
    );

    let parse = |resp: std::result::Result<
        aws_sdk_cloudwatch::operation::get_metric_statistics::GetMetricStatisticsOutput,
        _,
    >| -> Vec<(f64, f64)> {
        let mut pts: Vec<(f64, f64)> = match resp {
            Ok(r) => r
                .datapoints()
                .iter()
                .filter_map(|dp| Some((dp.timestamp()?.secs() as f64 - start as f64, dp.sum()?)))
                .collect(),
            Err(_) => vec![],
        };
        pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        pts
    };

    Ok(TransferMetricsData {
        time_range,
        bytes_in: parse(bytes_in),
        bytes_out: parse(bytes_out),
        files_in: parse(files_in),
        files_out: parse(files_out),
        uploads_ok: parse(uploads_ok),
        uploads_failed: parse(uploads_failed),
        x_max: time_range.duration_secs() as f64,
    })
}
