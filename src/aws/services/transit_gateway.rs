use crate::aws::client::AwsClients;
use crate::aws::resource::{native_state_label, Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use aws_sdk_ec2::Client as Ec2Client;
use std::any::Any;
use std::collections::HashMap;
use tokio::sync::mpsc;

/// Transit Gateway service — reuses the EC2 client (TGW lives in the EC2 API).
/// Three resource types over one heterogeneous list: gateways, attachments, and
/// route tables. Tags come inline from each describe; only route-table *routes*
/// need a lazy `SearchTransitGatewayRoutes` call.
pub struct TransitGatewayService {
    client: Ec2Client,
}

impl TransitGatewayService {
    pub fn new(aws_clients: &AwsClients) -> Self {
        Self {
            client: aws_clients.ec2_client(),
        }
    }
}

#[async_trait]
impl AwsService for TransitGatewayService {
    fn service_type(&self) -> ServiceType {
        ServiceType::TransitGateway
    }

    fn name(&self) -> &str {
        "Transit Gateway"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        self.list_resources_streaming(tx, ServiceType::TransitGateway)
            .await?;
        Ok(vec![])
    }

    async fn list_resources_streaming(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        let mut total = 0usize;

        // Phase 1: Transit Gateways (paginated; fatal on error — they're the core)
        let mut next: Option<String> = None;
        loop {
            let mut req = self.client.describe_transit_gateways();
            if let Some(t) = &next {
                req = req.next_token(t);
            }
            match req.send().await {
                Ok(resp) => {
                    let batch: Vec<Box<dyn Resource>> = resp
                        .transit_gateways()
                        .iter()
                        .map(|t| Box::new(TransitGateway::from_sdk(t)) as Box<dyn Resource>)
                        .collect();
                    total += batch.len();
                    let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                        service: service_type,
                        resources: batch,
                        progress: LoadProgress {
                            loaded_count: total,
                            total_count: None,
                            status_message: Some("Loading transit gateways…".to_string()),
                        },
                    });
                    next = crate::aws::pagination::next_page_token(resp.next_token(), &next);
                    if next.is_none() {
                        break;
                    }
                }
                Err(e) => {
                    let _ = event_tx.send(Event::ResourceLoadError {
                        service: service_type,
                        error: format!("Failed to list transit gateways: {}", e),
                    });
                    return Ok(());
                }
            }
        }

        // Phase 2: Attachments (paginated; surfaces errors instead of swallowing)
        let mut next: Option<String> = None;
        loop {
            let mut req = self.client.describe_transit_gateway_attachments();
            if let Some(t) = &next {
                req = req.next_token(t);
            }
            match req.send().await {
                Ok(resp) => {
                    let batch: Vec<Box<dyn Resource>> = resp
                        .transit_gateway_attachments()
                        .iter()
                        .map(|a| Box::new(TgwAttachment::from_sdk(a)) as Box<dyn Resource>)
                        .collect();
                    total += batch.len();
                    let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                        service: service_type,
                        resources: batch,
                        progress: LoadProgress {
                            loaded_count: total,
                            total_count: None,
                            status_message: Some("Loading attachments…".to_string()),
                        },
                    });
                    next = crate::aws::pagination::next_page_token(resp.next_token(), &next);
                    if next.is_none() {
                        break;
                    }
                }
                Err(e) => {
                    let _ = event_tx.send(Event::ResourceLoadWarning {
                        service: service_type,
                        warning: format!("TGW attachments: {}", e),
                    });
                    break;
                }
            }
        }

        // Phase 3: Route tables (paginated; surfaces errors instead of swallowing)
        let mut next: Option<String> = None;
        loop {
            let mut req = self.client.describe_transit_gateway_route_tables();
            if let Some(t) = &next {
                req = req.next_token(t);
            }
            match req.send().await {
                Ok(resp) => {
                    let batch: Vec<Box<dyn Resource>> = resp
                        .transit_gateway_route_tables()
                        .iter()
                        .map(|rt| Box::new(TgwRouteTable::from_sdk(rt)) as Box<dyn Resource>)
                        .collect();
                    total += batch.len();
                    let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                        service: service_type,
                        resources: batch,
                        progress: LoadProgress {
                            loaded_count: total,
                            total_count: None,
                            status_message: Some("Loading route tables…".to_string()),
                        },
                    });
                    next = crate::aws::pagination::next_page_token(resp.next_token(), &next);
                    if next.is_none() {
                        break;
                    }
                }
                Err(e) => {
                    let _ = event_tx.send(Event::ResourceLoadWarning {
                        service: service_type,
                        warning: format!("TGW route tables: {}", e),
                    });
                    break;
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

// ── helpers ───────────────────────────────────────────────────────────────────

fn tags_of(tags: &[aws_sdk_ec2::types::Tag]) -> HashMap<String, String> {
    tags.iter()
        .filter_map(|t| match (t.key(), t.value()) {
            (Some(k), Some(v)) => Some((k.to_string(), v.to_string())),
            _ => None,
        })
        .collect()
}

/// Append every tag key+value to a search string so fuzzy search matches the
/// Name (and any other tag) shown in the list, not just the ids.
fn append_tags(mut s: String, tags: &HashMap<String, String>) -> String {
    for (k, v) in tags {
        s.push(' ');
        s.push_str(k);
        s.push(' ');
        s.push_str(v);
    }
    s
}

/// `available`/`pending`/… → a `ResourceState` for the list dot.
fn state_from(raw: &str) -> ResourceState {
    match raw {
        "available" => ResourceState::Available,
        "pending" | "modifying" | "initiating" | "initiatingRequest" | "pendingAcceptance" => {
            ResourceState::Pending
        }
        "deleting" => ResourceState::Deleting,
        "deleted" | "rejected" | "rejecting" | "failed" | "failing" => ResourceState::Terminated,
        other => ResourceState::Unknown(other.to_string()),
    }
}

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

// ── TransitGateway ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct TransitGateway {
    pub id: String,
    pub arn: String,
    pub state: String,
    pub owner_id: String,
    pub description: String,
    pub amazon_side_asn: Option<i64>,
    pub dns_support: bool,
    pub default_route_table_association: bool,
    pub default_route_table_propagation: bool,
    pub association_default_route_table_id: Option<String>,
    pub propagation_default_route_table_id: Option<String>,
    pub creation_time: Option<String>,
    pub tags: HashMap<String, String>,
}

impl TransitGateway {
    pub fn from_sdk(t: &aws_sdk_ec2::types::TransitGateway) -> Self {
        let tags = tags_of(t.tags());
        let opts = t.options();
        Self {
            id: t.transit_gateway_id().unwrap_or_default().to_string(),
            arn: t.transit_gateway_arn().unwrap_or_default().to_string(),
            state: t.state().map(|s| s.as_str().to_string()).unwrap_or_default(),
            owner_id: t.owner_id().unwrap_or_default().to_string(),
            description: t.description().unwrap_or_default().to_string(),
            amazon_side_asn: opts.and_then(|o| o.amazon_side_asn()),
            dns_support: opts
                .and_then(|o| o.dns_support())
                .map(|v| v.as_str() == "enable")
                .unwrap_or(false),
            default_route_table_association: opts
                .and_then(|o| o.default_route_table_association())
                .map(|v| v.as_str() == "enable")
                .unwrap_or(false),
            default_route_table_propagation: opts
                .and_then(|o| o.default_route_table_propagation())
                .map(|v| v.as_str() == "enable")
                .unwrap_or(false),
            association_default_route_table_id: opts
                .and_then(|o| o.association_default_route_table_id())
                .map(|s| s.to_string()),
            propagation_default_route_table_id: opts
                .and_then(|o| o.propagation_default_route_table_id())
                .map(|s| s.to_string()),
            creation_time: t.creation_time().map(|dt| fmt_epoch_secs(dt.secs())),
            tags,
        }
    }
}

crate::sections! {
    pub enum TgwDetailSection,
    pub static TGW_SECTIONS = [
        Details "Details",
        Attachments "Attachments",
        RouteTables "Route Tables",
        Tags "Tags",
    ]
}

impl Resource for TransitGateway {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&TGW_SECTIONS)
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws ec2 describe-transit-gateways --transit-gateway-ids {}",
            crate::aws::resource::shell_quote(self.id())
        ))
    }

    fn id(&self) -> &str {
        &self.id
    }
    fn name(&self) -> &str {
        // Cache-free: prefer the Name tag, else the id.
        self.tags
            .get("Name")
            .filter(|n| !n.is_empty())
            .map(|s| s.as_str())
            .unwrap_or(&self.id)
    }
    fn resource_type(&self) -> &str {
        "Transit Gateway"
    }
    fn state(&self) -> ResourceState {
        state_from(&self.state)
    }

    fn state_label(&self) -> String {
        native_state_label(&self.state, || self.state())
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        append_tags(
            format!("{} {} {}", self.id, self.description, self.owner_id),
            &self.tags,
        )
    }
    fn details(&self) -> Vec<(String, String)> {
        let mut d = vec![
            ("ID".to_string(), self.id.clone()),
            ("State".to_string(), self.state.clone()),
            ("Owner".to_string(), self.owner_id.clone()),
        ];
        if let Some(asn) = self.amazon_side_asn {
            d.push(("Amazon Side ASN".to_string(), asn.to_string()));
        }
        if !self.description.is_empty() {
            d.push(("Description".to_string(), self.description.clone()));
        }
        d
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{region}.console.aws.amazon.com/vpc/home?region={region}#TransitGatewayDetails:transitGatewayId={}",
            self.id
        ))
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── TgwAttachment ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct TgwAttachment {
    pub id: String,
    pub tgw_id: String,
    pub resource_kind: String, // vpc / vpn / direct-connect-gateway / peering / connect
    pub resource_id: String,
    pub resource_owner_id: String,
    pub state: String,
    pub association_route_table_id: Option<String>,
    pub association_state: Option<String>,
    pub creation_time: Option<String>,
    pub tags: HashMap<String, String>,
}

impl TgwAttachment {
    pub fn from_sdk(a: &aws_sdk_ec2::types::TransitGatewayAttachment) -> Self {
        let tags = tags_of(a.tags());
        let assoc = a.association();
        Self {
            id: a
                .transit_gateway_attachment_id()
                .unwrap_or_default()
                .to_string(),
            tgw_id: a.transit_gateway_id().unwrap_or_default().to_string(),
            resource_kind: a
                .resource_type()
                .map(|r| r.as_str().to_string())
                .unwrap_or_default(),
            resource_id: a.resource_id().unwrap_or_default().to_string(),
            resource_owner_id: a.resource_owner_id().unwrap_or_default().to_string(),
            state: a.state().map(|s| s.as_str().to_string()).unwrap_or_default(),
            association_route_table_id: assoc
                .and_then(|x| x.transit_gateway_route_table_id())
                .map(|s| s.to_string()),
            association_state: assoc
                .and_then(|x| x.state())
                .map(|s| s.as_str().to_string()),
            creation_time: a.creation_time().map(|dt| fmt_epoch_secs(dt.secs())),
            tags,
        }
    }
}

crate::sections! {
    pub enum TgwAttachmentDetailSection,
    pub static TGW_ATTACHMENT_SECTIONS = [
        Overview "Overview",
        Association "Association",
        Tags "Tags",
    ]
}

impl Resource for TgwAttachment {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&TGW_ATTACHMENT_SECTIONS)
    }

    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws ec2 describe-transit-gateway-attachments --transit-gateway-attachment-ids {}",
            crate::aws::resource::shell_quote(self.id())
        ))
    }

    fn id(&self) -> &str {
        &self.id
    }
    fn name(&self) -> &str {
        self.tags
            .get("Name")
            .filter(|n| !n.is_empty())
            .map(|s| s.as_str())
            .unwrap_or(&self.id)
    }
    fn resource_type(&self) -> &str {
        "TGW Attachment"
    }
    fn state(&self) -> ResourceState {
        state_from(&self.state)
    }

    fn state_label(&self) -> String {
        native_state_label(&self.state, || self.state())
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        append_tags(
            format!(
                "{} {} {} {} {}",
                self.id, self.tgw_id, self.resource_kind, self.resource_id, self.resource_owner_id
            ),
            &self.tags,
        )
    }
    fn details(&self) -> Vec<(String, String)> {
        let mut d = vec![
            ("ID".to_string(), self.id.clone()),
            ("Transit Gateway".to_string(), self.tgw_id.clone()),
            ("Resource Type".to_string(), self.resource_kind.clone()),
            ("Resource ID".to_string(), self.resource_id.clone()),
            ("State".to_string(), self.state.clone()),
        ];
        if !self.resource_owner_id.is_empty() {
            d.push(("Resource Owner".to_string(), self.resource_owner_id.clone()));
        }
        if let Some(rt) = &self.association_route_table_id {
            d.push(("Assoc Route Table".to_string(), rt.clone()));
        }
        if let Some(st) = &self.association_state {
            d.push(("Assoc State".to_string(), st.clone()));
        }
        if let Some(ct) = &self.creation_time {
            d.push(("Created".to_string(), ct.clone()));
        }
        d
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── TgwRouteTable ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct TgwRouteTable {
    pub id: String,
    pub tgw_id: String,
    pub state: String,
    pub default_association: bool,
    pub default_propagation: bool,
    pub creation_time: Option<String>,
    pub tags: HashMap<String, String>,
}

impl TgwRouteTable {
    pub fn from_sdk(rt: &aws_sdk_ec2::types::TransitGatewayRouteTable) -> Self {
        Self {
            id: rt
                .transit_gateway_route_table_id()
                .unwrap_or_default()
                .to_string(),
            tgw_id: rt.transit_gateway_id().unwrap_or_default().to_string(),
            state: rt.state().map(|s| s.as_str().to_string()).unwrap_or_default(),
            default_association: rt.default_association_route_table().unwrap_or(false),
            default_propagation: rt.default_propagation_route_table().unwrap_or(false),
            creation_time: rt.creation_time().map(|dt| fmt_epoch_secs(dt.secs())),
            tags: tags_of(rt.tags()),
        }
    }
}

crate::sections! {
    pub enum TgwRouteTableDetailSection,
    pub static TGW_ROUTE_TABLE_SECTIONS = [
        Routes "Routes" => crate::app::App::trigger_tgw_routes_load,
        Tags "Tags",
    ]
}

impl Resource for TgwRouteTable {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&TGW_ROUTE_TABLE_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.id
    }
    fn name(&self) -> &str {
        self.tags
            .get("Name")
            .filter(|n| !n.is_empty())
            .map(|s| s.as_str())
            .unwrap_or(&self.id)
    }
    fn resource_type(&self) -> &str {
        "TGW Route Table"
    }
    fn state(&self) -> ResourceState {
        state_from(&self.state)
    }

    fn state_label(&self) -> String {
        native_state_label(&self.state, || self.state())
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        append_tags(format!("{} {}", self.id, self.tgw_id), &self.tags)
    }
    fn details(&self) -> Vec<(String, String)> {
        let mut d = vec![
            ("ID".to_string(), self.id.clone()),
            ("Transit Gateway".to_string(), self.tgw_id.clone()),
            ("State".to_string(), self.state.clone()),
            (
                "Default Association".to_string(),
                self.default_association.to_string(),
            ),
            (
                "Default Propagation".to_string(),
                self.default_propagation.to_string(),
            ),
        ];
        if let Some(ct) = &self.creation_time {
            d.push(("Created".to_string(), ct.clone()));
        }
        d
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── Lazy routes (SearchTransitGatewayRoutes) ──────────────────────────────────

#[derive(Debug, Clone)]
pub struct TgwRoute {
    pub cidr: String,
    pub state: String,       // active / blackhole
    pub route_type: String,  // static / propagated
    pub attachment_id: String,
    pub resource_id: String,
    pub resource_type: String,
}

/// TGW routes can't be plain-listed — `SearchTransitGatewayRoutes` requires at
/// least one filter, so we query active + blackhole routes for the table.
pub async fn fetch_tgw_routes(
    client: Ec2Client,
    route_table_id: String,
) -> Result<Vec<TgwRoute>> {
    let filter = aws_sdk_ec2::types::Filter::builder()
        .name("state")
        .values("active")
        .values("blackhole")
        .build();

    let resp = client
        .search_transit_gateway_routes()
        .transit_gateway_route_table_id(&route_table_id)
        .filters(filter)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;

    let mut routes = Vec::new();
    for r in resp.routes() {
        let (attachment_id, resource_id, resource_type) = r
            .transit_gateway_attachments()
            .first()
            .map(|a| {
                (
                    a.transit_gateway_attachment_id().unwrap_or_default().to_string(),
                    a.resource_id().unwrap_or_default().to_string(),
                    a.resource_type().map(|t| t.as_str().to_string()).unwrap_or_default(),
                )
            })
            .unwrap_or_default();
        routes.push(TgwRoute {
            cidr: r.destination_cidr_block().unwrap_or_default().to_string(),
            state: r.state().map(|s| s.as_str().to_string()).unwrap_or_default(),
            route_type: r.r#type().map(|t| t.as_str().to_string()).unwrap_or_default(),
            attachment_id,
            resource_id,
            resource_type,
        });
    }
    Ok(routes)
}

// ── CloudWatch metrics (`m` overlay) ──────────────────────────────────────────

use crate::aws::services::ec2::{parse_metric_datapoints, MetricsTimeRange};

#[derive(Debug, Clone)]
pub struct TgwMetricsData {
    pub time_range: MetricsTimeRange,
    pub bytes_in: Vec<(f64, f64)>,
    pub bytes_out: Vec<(f64, f64)>,
    pub packets_in: Vec<(f64, f64)>,
    pub packets_out: Vec<(f64, f64)>,
    pub drops_blackhole: Vec<(f64, f64)>,
    pub drops_no_route: Vec<(f64, f64)>,
    pub x_max: f64,
}

#[derive(Debug, Clone)]
pub enum TgwMetricsState {
    Loading,
    Loaded(TgwMetricsData),
}

/// Pull `AWS/TransitGateway` metrics for a transit gateway, or — when
/// `attachment_id` is set — one attachment (the per-attachment series carry
/// both the `TransitGateway` and `TransitGatewayAttachment` dimensions).
/// Bytes/packets are the traffic headline; the two drop counters are the
/// routing-health signals (blackhole route hit / no route matched).
pub async fn fetch_tgw_metrics(
    cw: aws_sdk_cloudwatch::Client,
    tgw_id: String,
    attachment_id: Option<String>,
    time_range: MetricsTimeRange,
) -> crate::error::Result<TgwMetricsData> {
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
        let mut dims = vec![Dimension::builder()
            .name("TransitGateway")
            .value(&tgw_id)
            .build()];
        if let Some(att) = &attachment_id {
            dims.push(
                Dimension::builder()
                    .name("TransitGatewayAttachment")
                    .value(att)
                    .build(),
            );
        }
        cw.get_metric_statistics()
            .namespace("AWS/TransitGateway")
            .metric_name(name)
            .set_dimensions(Some(dims))
            .start_time(start_dt)
            .end_time(end_dt)
            .period(period)
            .set_statistics(Some(vec![Statistic::Sum]))
            .send()
    };

    let (bytes_in, bytes_out, packets_in, packets_out, blackhole, no_route) = tokio::join!(
        metric("BytesIn"),
        metric("BytesOut"),
        metric("PacketsIn"),
        metric("PacketsOut"),
        metric("PacketDropCountBlackhole"),
        metric("PacketDropCountNoRoute"),
    );

    Ok(TgwMetricsData {
        time_range,
        bytes_in: parse_metric_datapoints(bytes_in, start),
        bytes_out: parse_metric_datapoints(bytes_out, start),
        packets_in: parse_metric_datapoints(packets_in, start),
        packets_out: parse_metric_datapoints(packets_out, start),
        drops_blackhole: parse_metric_datapoints(blackhole, start),
        drops_no_route: parse_metric_datapoints(no_route, start),
        x_max: time_range.duration_secs() as f64,
    })
}
