use crate::aws::client::AwsClients;
use crate::aws::resource::{native_state_label, Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use aws_sdk_globalaccelerator::Client as GaClient;
use std::any::Any;
use std::collections::HashMap;
use tokio::sync::mpsc;

/// Global Accelerator service — single-list of accelerators with a split detail
/// pane. The client is pinned to **us-west-2** (GA's only control-plane
/// endpoint); `is_global()` keys the *cache* on us-east-1 (those are
/// independent and both correct). Listeners / endpoint groups / tags are
/// fetched lazily on first view.
pub struct GlobalAcceleratorService {
    client: GaClient,
}

impl GlobalAcceleratorService {
    pub fn new(aws_clients: &AwsClients) -> Self {
        Self {
            client: aws_clients.globalaccelerator_client(),
        }
    }
}

#[async_trait]
impl AwsService for GlobalAcceleratorService {
    fn service_type(&self) -> ServiceType {
        ServiceType::GlobalAccelerator
    }

    fn name(&self) -> &str {
        "Global Accelerator"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        self.list_resources_streaming(tx, ServiceType::GlobalAccelerator)
            .await?;
        Ok(vec![])
    }

    async fn list_resources_streaming(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        let mut total = 0usize;
        let mut next: Option<String> = None;

        loop {
            let mut req = self.client.list_accelerators();
            if let Some(t) = &next {
                req = req.next_token(t);
            }
            match req.send().await {
                Ok(resp) => {
                    let batch: Vec<Box<dyn Resource>> = resp
                        .accelerators()
                        .iter()
                        .map(|a| Box::new(GaAccelerator::from_sdk(a)) as Box<dyn Resource>)
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
                    next = crate::aws::pagination::next_page_token(resp.next_token(), &next);
                    if next.is_none() {
                        break;
                    }
                }
                Err(e) => {
                    let _ = event_tx.send(Event::ResourceLoadError {
                        service: service_type,
                        error: format!("Failed to list accelerators: {}", e),
                    });
                    return Ok(());
                }
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

// ── GaAccelerator ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct GaAccelerator {
    pub arn: String,
    pub name: String,
    pub status: String, // DEPLOYED / IN_PROGRESS
    pub enabled: bool,
    pub ip_address_type: String, // IPV4 / DUAL_STACK
    pub dns_name: String,
    pub static_ips: Vec<String>,
    pub created: Option<String>,
    pub last_modified: Option<String>,
    pub tags: HashMap<String, String>,
}

impl GaAccelerator {
    pub fn from_sdk(a: &aws_sdk_globalaccelerator::types::Accelerator) -> Self {
        let static_ips = a
            .ip_sets()
            .iter()
            .flat_map(|s| s.ip_addresses().iter().cloned())
            .collect();
        Self {
            arn: a.accelerator_arn().unwrap_or_default().to_string(),
            name: a.name().unwrap_or_default().to_string(),
            status: a
                .status()
                .map(|s| s.as_str().to_string())
                .unwrap_or_default(),
            enabled: a.enabled().unwrap_or(false),
            ip_address_type: a
                .ip_address_type()
                .map(|t| t.as_str().to_string())
                .unwrap_or_default(),
            dns_name: a.dns_name().unwrap_or_default().to_string(),
            static_ips,
            created: a.created_time().map(|d| fmt_epoch_secs(d.secs())),
            last_modified: a.last_modified_time().map(|d| fmt_epoch_secs(d.secs())),
            tags: HashMap::new(),
        }
    }
}

crate::sections! {
    pub enum GaAcceleratorDetailSection,
    pub static GA_ACCELERATOR_SECTIONS = [
        Details "Details",
        Listeners "Listeners" => crate::app::App::trigger_ga_listeners_load,
        EndpointGroups "Endpoint Groups" => crate::app::App::trigger_ga_endpoint_groups_load,
        Tags "Tags" => crate::app::App::trigger_ga_tags_load,
    ]
}

impl Resource for GaAccelerator {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&GA_ACCELERATOR_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.arn
    }

    fn name(&self) -> &str {
        if self.name.is_empty() {
            // Fall back to the arn's last segment.
            self.arn.rsplit('/').next().unwrap_or(&self.arn)
        } else {
            &self.name
        }
    }

    fn resource_type(&self) -> &str {
        "Global Accelerator"
    }

    fn state(&self) -> ResourceState {
        if !self.enabled {
            return ResourceState::Stopped;
        }
        match self.status.as_str() {
            "DEPLOYED" => ResourceState::Available,
            "IN_PROGRESS" => ResourceState::Pending,
            other => ResourceState::Unknown(other.to_string()),
        }
    }

    fn state_label(&self) -> String {
        if !self.enabled {
            return "disabled".to_string();
        }
        native_state_label(&self.status, || self.state())
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!(
            "{} {} {} {}",
            self.name,
            self.dns_name,
            self.static_ips.join(" "),
            self.arn,
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Name".to_string(), self.name.clone()),
            ("Status".to_string(), self.status.clone()),
            ("Enabled".to_string(), self.enabled.to_string()),
            ("IP Type".to_string(), self.ip_address_type.clone()),
            ("DNS Name".to_string(), self.dns_name.clone()),
            ("Static IPs".to_string(), self.static_ips.join(", ")),
        ]
    }

    fn console_url(&self, _region: &str) -> Option<String> {
        Some(format!(
            "https://us-west-2.console.aws.amazon.com/globalaccelerator/home#GlobalAccelerator:AcceleratorDetails:{}",
            self.arn
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── Children (rendered in the split pane) ─────────────────────────────────────

#[derive(Debug, Clone)]
pub struct GaListener {
    pub protocol: String,
    pub port_ranges: String, // "80, 443" / "8000-8100"
    pub client_affinity: String,
}

#[derive(Debug, Clone)]
pub struct GaEndpoint {
    pub id: String,
    pub weight: i32,
    pub health_state: String,
    pub health_reason: String,
}

#[derive(Debug, Clone)]
pub struct GaEndpointGroup {
    pub region: String,
    pub listener_protocol: String,
    pub traffic_dial: f32,
    pub health_check: String, // "TCP:80" / "HTTP:80/path"
    pub endpoints: Vec<GaEndpoint>,
}

// ── Lazy fetch functions ──────────────────────────────────────────────────────

fn port_ranges_str(ranges: &[aws_sdk_globalaccelerator::types::PortRange]) -> String {
    ranges
        .iter()
        .map(|r| {
            let from = r.from_port().unwrap_or(0);
            let to = r.to_port().unwrap_or(0);
            if from == to {
                from.to_string()
            } else {
                format!("{}-{}", from, to)
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

pub async fn fetch_ga_listeners(client: GaClient, accelerator_arn: String) -> Result<Vec<GaListener>> {
    let resp = client
        .list_listeners()
        .accelerator_arn(&accelerator_arn)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;

    Ok(resp
        .listeners()
        .iter()
        .map(|l| GaListener {
            protocol: l
                .protocol()
                .map(|p| p.as_str().to_string())
                .unwrap_or_default(),
            port_ranges: port_ranges_str(l.port_ranges()),
            client_affinity: l
                .client_affinity()
                .map(|c| c.as_str().to_string())
                .unwrap_or_default(),
        })
        .collect())
}

/// Endpoint groups are per-listener — walk the listeners first, then fetch each
/// listener's groups, so the Endpoint Groups tab renders in one shot.
pub async fn fetch_ga_endpoint_groups(
    client: GaClient,
    accelerator_arn: String,
) -> Result<Vec<GaEndpointGroup>> {
    let listeners = client
        .list_listeners()
        .accelerator_arn(&accelerator_arn)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;

    let mut out = Vec::new();
    for l in listeners.listeners() {
        let Some(listener_arn) = l.listener_arn() else {
            continue;
        };
        let protocol = l
            .protocol()
            .map(|p| p.as_str().to_string())
            .unwrap_or_default();
        let groups = client
            .list_endpoint_groups()
            .listener_arn(listener_arn)
            .send()
            .await
            .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;

        for g in groups.endpoint_groups() {
            let hc_proto = g
                .health_check_protocol()
                .map(|p| p.as_str().to_string())
                .unwrap_or_default();
            let hc_port = g.health_check_port().unwrap_or(0);
            let hc_path = g.health_check_path().unwrap_or_default();
            let health_check = if hc_path.is_empty() || hc_path == "/" {
                format!("{}:{}", hc_proto, hc_port)
            } else {
                format!("{}:{}{}", hc_proto, hc_port, hc_path)
            };

            let endpoints = g
                .endpoint_descriptions()
                .iter()
                .map(|e| GaEndpoint {
                    id: e.endpoint_id().unwrap_or_default().to_string(),
                    weight: e.weight().unwrap_or(0),
                    health_state: e
                        .health_state()
                        .map(|h| h.as_str().to_string())
                        .unwrap_or_default(),
                    health_reason: e.health_reason().unwrap_or_default().to_string(),
                })
                .collect();

            out.push(GaEndpointGroup {
                region: g.endpoint_group_region().unwrap_or_default().to_string(),
                listener_protocol: protocol.clone(),
                traffic_dial: g.traffic_dial_percentage().unwrap_or(100.0),
                health_check,
                endpoints,
            });
        }
    }
    Ok(out)
}

pub async fn fetch_ga_tags(
    client: GaClient,
    accelerator_arn: String,
) -> Result<HashMap<String, String>> {
    let resp = client
        .list_tags_for_resource()
        .resource_arn(&accelerator_arn)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;

    Ok(resp
        .tags()
        .iter()
        .map(|t| (t.key().to_string(), t.value().to_string()))
        .collect())
}

// ── Timestamp helpers ─────────────────────────────────────────────────────────

fn fmt_epoch_secs(secs: i64) -> String {
    let days = secs / 86400;
    let rem = secs % 86400;
    let (h, mi) = (rem / 3600, (rem % 3600) / 60);
    let (y, mo, d) = epoch_days_to_ymd(days);
    format!("{:04}-{:02}-{:02} {:02}:{:02} UTC", y, mo, d, h, mi)
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
        31, 30, 31, 30, 31, 31, 30, 31, 30, 31,
    ];
    let mut month = 1u8;
    for &len in &dm {
        if days < len as i64 {
            break;
        }
        days -= len as i64;
        month += 1;
    }
    (year, month, (days + 1) as u8)
}

fn is_leap(y: i32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

// ── CloudWatch metrics (`m` overlay) — AWS/GlobalAccelerator ──────────────────

pub use crate::aws::services::ec2::{parse_metric_datapoints, MetricsTimeRange};

#[derive(Debug, Clone)]
pub struct GaMetricsData {
    pub time_range: MetricsTimeRange,
    pub new_flows: Vec<(f64, f64)>,
    pub bytes_in: Vec<(f64, f64)>,
    pub bytes_out: Vec<(f64, f64)>,
    pub x_max: f64,
}

#[derive(Debug, Clone)]
pub enum GaMetricsState {
    Loading,
    Loaded(GaMetricsData),
}

/// Pull `AWS/GlobalAccelerator` metrics for one accelerator. The `Accelerator`
/// dimension value is the accelerator **ARN**, and GA publishes metrics only in
/// its us-west-2 home region — callers must pass the us-west-2 CloudWatch
/// client (`cloudwatch_us_west_2_client`), like CloudFront does with us-east-1.
pub async fn fetch_ga_metrics(
    cw: aws_sdk_cloudwatch::Client,
    accelerator_arn: String,
    time_range: MetricsTimeRange,
) -> crate::error::Result<GaMetricsData> {
    use aws_sdk_cloudwatch::types::{Dimension, Statistic};

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let start = now - time_range.duration_secs();
    let period = time_range.period_secs();
    let start_dt = aws_sdk_cloudwatch::primitives::DateTime::from_secs(start);
    let end_dt = aws_sdk_cloudwatch::primitives::DateTime::from_secs(now);

    let metric = |name: &'static str| {
        cw.get_metric_statistics()
            .namespace("AWS/GlobalAccelerator")
            .metric_name(name)
            .dimensions(
                Dimension::builder()
                    .name("Accelerator")
                    .value(&accelerator_arn)
                    .build(),
            )
            .start_time(start_dt)
            .end_time(end_dt)
            .period(period)
            .set_statistics(Some(vec![Statistic::Sum]))
            .send()
    };

    let (flows, bytes_in, bytes_out) = tokio::join!(
        metric("NewFlowCount"),
        metric("ProcessedBytesIn"),
        metric("ProcessedBytesOut"),
    );

    Ok(GaMetricsData {
        time_range,
        new_flows: parse_metric_datapoints(flows, start),
        bytes_in: parse_metric_datapoints(bytes_in, start),
        bytes_out: parse_metric_datapoints(bytes_out, start),
        x_max: time_range.duration_secs() as f64,
    })
}
