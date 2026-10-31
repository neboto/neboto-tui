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
                        error: format!("Failed to list transit gateways: {}", crate::error::sdk_error_message(&e)),
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
                        warning: format!("TGW attachments: {}", crate::error::sdk_error_message(&e)),
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
                        warning: format!("TGW route tables: {}", crate::error::sdk_error_message(&e)),
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
    /// Any account the gateway is shared with (RAM) can attach without the
    /// owner accepting.
    pub auto_accept_shared_attachments: bool,
    pub vpn_ecmp_support: bool,
    pub multicast_support: bool,
    pub security_group_referencing_support: bool,
    /// The gateway's own CIDRs (Connect peers' inside addresses come from them).
    pub cidr_blocks: Vec<String>,
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
            auto_accept_shared_attachments: opts
                .and_then(|o| o.auto_accept_shared_attachments())
                .map(|v| v.as_str() == "enable")
                .unwrap_or(false),
            vpn_ecmp_support: opts
                .and_then(|o| o.vpn_ecmp_support())
                .map(|v| v.as_str() == "enable")
                .unwrap_or(false),
            multicast_support: opts
                .and_then(|o| o.multicast_support())
                .map(|v| v.as_str() == "enable")
                .unwrap_or(false),
            security_group_referencing_support: opts
                .and_then(|o| o.security_group_referencing_support())
                .map(|v| v.as_str() == "enable")
                .unwrap_or(false),
            cidr_blocks: opts
                .map(|o| o.transit_gateway_cidr_blocks().to_vec())
                .unwrap_or_default(),
            creation_time: t.creation_time().map(|dt| fmt_epoch_secs(dt.secs())),
            tags,
        }
    }
}

crate::sections! {
    pub enum TgwDetailSection,
    pub static TGW_SECTIONS = [
        Overview "Overview",
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
    fn list_summary(&self) -> Option<String> {
        let summary = if !self.description.is_empty() {
            self.description.clone()
        } else {
            format!("ASN {}", self.amazon_side_asn?)
        };
        Some(crate::aws::resource::summary_with_id(self, &summary))
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
        Overview "Overview" => crate::app::App::trigger_tgw_attachment_load,
        Association "Association" => crate::app::App::trigger_tgw_attachment_load,
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
    /// What it attaches: `vpc · vpc-0abc…`.
    fn list_summary(&self) -> Option<String> {
        match (self.resource_kind.is_empty(), self.resource_id.is_empty()) {
            (true, true) => None,
            (false, true) => Some(self.resource_kind.clone()),
            (true, false) => Some(self.resource_id.clone()),
            (false, false) => Some(format!("{} · {}", self.resource_kind, self.resource_id)),
        }
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{region}.console.aws.amazon.com/vpc/home?region={region}#TransitGatewayAttachmentDetails:transitGatewayAttachmentId={}",
            self.id
        ))
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
        Overview "Overview" => crate::app::App::trigger_tgw_route_table_load,
        Routes "Routes" => crate::app::App::trigger_tgw_routes_load,
        Propagations "Propagations" => crate::app::App::trigger_tgw_propagations_load,
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
    /// Its gateway, and whether new attachments land here by default.
    fn list_summary(&self) -> Option<String> {
        let default = match (self.default_association, self.default_propagation) {
            (true, true) => " · default assoc + prop",
            (true, false) => " · default assoc",
            (false, true) => " · default prop",
            (false, false) => "",
        };
        Some(format!("{}{default}", self.tgw_id))
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws ec2 describe-transit-gateway-route-tables --transit-gateway-route-table-ids {}",
            crate::aws::resource::shell_quote(self.id())
        ))
    }
    fn cli_actions(&self) -> Vec<crate::aws::cli_actions::CliAction> {
        use crate::aws::cli_actions::{CliAction, CliTier};
        vec![CliAction::new(
            CliTier::Inspect,
            "search-transit-gateway-routes",
            format!(
                "aws ec2 search-transit-gateway-routes --transit-gateway-route-table-id {} --filters Name=state,Values=active,blackhole",
                crate::aws::resource::shell_quote(self.id())
            ),
        )]
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{region}.console.aws.amazon.com/vpc/home?region={region}#TransitGatewayRouteTableDetails:transitGatewayRouteTableId={}",
            self.id
        ))
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
    /// The destination CIDR, or the managed prefix list (`pl-…`) for a
    /// prefix-list route — those carry no CIDR.
    pub cidr: String,
    pub state: String,       // active / blackhole
    pub route_type: String,  // static / propagated
    pub attachment_id: String,
    pub resource_id: String,
    pub resource_type: String,
    /// Further attachments an ECMP route spreads across (VPN / Connect),
    /// beyond the first one above.
    pub extra_targets: usize,
}

/// A route table's routes. `more`: the table holds more than one search
/// returns (`AdditionalRoutesAvailable`) — the API has no page token.
#[derive(Debug, Clone, Default)]
pub struct TgwRoutes {
    pub routes: Vec<TgwRoute>,
    pub more: bool,
}

/// The most routes one `SearchTransitGatewayRoutes` returns (also its
/// default).
pub const TGW_ROUTES_MAX: i32 = 1000;

/// TGW routes can't be plain-listed — `SearchTransitGatewayRoutes` requires at
/// least one filter, so we query active + blackhole routes for the table. It
/// returns at most `TGW_ROUTES_MAX` and can't page, so a fuller table comes
/// back flagged `more`; when that page holds no default route, one exact
/// `0.0.0.0/0` search settles whether the table has one.
pub async fn fetch_tgw_routes(client: Ec2Client, route_table_id: String) -> Result<TgwRoutes> {
    let state = aws_sdk_ec2::types::Filter::builder()
        .name("state")
        .values("active")
        .values("blackhole")
        .build();
    let page = search_tgw_routes(&client, &route_table_id, state).await?;
    let mut routes = page.routes;
    if page.more && !routes.iter().any(|r| r.cidr == "0.0.0.0/0") {
        let exact = aws_sdk_ec2::types::Filter::builder()
            .name("route-search.exact-match")
            .values("0.0.0.0/0")
            .build();
        routes.extend(search_tgw_routes(&client, &route_table_id, exact).await?.routes);
    }
    Ok(TgwRoutes { routes, more: page.more })
}

async fn search_tgw_routes(
    client: &Ec2Client,
    route_table_id: &str,
    filter: aws_sdk_ec2::types::Filter,
) -> Result<TgwRoutes> {
    let resp = client
        .search_transit_gateway_routes()
        .transit_gateway_route_table_id(route_table_id)
        .filters(filter)
        .max_results(TGW_ROUTES_MAX)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
    Ok(TgwRoutes {
        routes: resp.routes().iter().map(TgwRoute::from_sdk).collect(),
        more: resp.additional_routes_available().unwrap_or(false),
    })
}

impl TgwRoute {
    pub fn from_sdk(r: &aws_sdk_ec2::types::TransitGatewayRoute) -> Self {
        let targets = r.transit_gateway_attachments();
        let (attachment_id, resource_id, resource_type) = targets
            .first()
            .map(|a| {
                (
                    a.transit_gateway_attachment_id().unwrap_or_default().to_string(),
                    a.resource_id().unwrap_or_default().to_string(),
                    a.resource_type().map(|t| t.as_str().to_string()).unwrap_or_default(),
                )
            })
            .unwrap_or_default();
        Self {
            cidr: r
                .destination_cidr_block()
                .or(r.prefix_list_id())
                .unwrap_or_default()
                .to_string(),
            state: r.state().map(|s| s.as_str().to_string()).unwrap_or_default(),
            route_type: r.r#type().map(|t| t.as_str().to_string()).unwrap_or_default(),
            attachment_id,
            resource_id,
            resource_type,
            extra_targets: targets.len().saturating_sub(1),
        }
    }
}

// ── Lazy propagations + per-kind attachment detail ────────────────────────────

/// An attachment whose routes propagate into a route table
/// (`GetTransitGatewayRouteTablePropagations`) — where a table's
/// `propagated` routes come from.
#[derive(Debug, Clone)]
pub struct TgwPropagation {
    pub attachment_id: String,
    pub resource_id: String,
    pub resource_type: String,
    pub state: String,
}

pub async fn fetch_tgw_route_table_propagations(
    client: Ec2Client,
    route_table_id: String,
) -> Result<Vec<TgwPropagation>> {
    let mut out = Vec::new();
    let mut next: Option<String> = None;
    loop {
        let mut req = client
            .get_transit_gateway_route_table_propagations()
            .transit_gateway_route_table_id(&route_table_id);
        if let Some(t) = &next {
            req = req.next_token(t);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
        out.extend(resp.transit_gateway_route_table_propagations().iter().map(|p| TgwPropagation {
            attachment_id: p.transit_gateway_attachment_id().unwrap_or_default().to_string(),
            resource_id: p.resource_id().unwrap_or_default().to_string(),
            resource_type: p.resource_type().map(|t| t.as_str().to_string()).unwrap_or_default(),
            state: p.state().map(|s| s.as_str().to_string()).unwrap_or_default(),
        }));
        next = crate::aws::pagination::next_page_token(resp.next_token(), &next);
        if next.is_none() {
            return Ok(out);
        }
    }
}

/// A route table an attachment propagates into.
#[derive(Debug, Clone)]
pub struct TgwAttachmentPropagation {
    pub route_table_id: String,
    pub state: String,
}

/// What an attachment's Overview and Association sections fetch on first
/// view: where its routes propagate, plus what only its kind's own describe
/// carries. Each half fails on its own — a denied peering describe still
/// shows the propagations.
#[derive(Debug, Clone)]
pub struct TgwAttachmentDetail {
    pub propagations: std::result::Result<Vec<TgwAttachmentPropagation>, String>,
    pub kind: std::result::Result<TgwKindDetail, String>,
}

#[derive(Debug, Clone)]
pub enum TgwKindDetail {
    /// A kind with nothing past the attachment list (VPN, DX gateway…).
    None,
    Vpc(TgwVpcDetail),
    Peering(TgwPeeringDetail),
    Connect(TgwConnectDetail),
}

#[derive(Debug, Clone)]
pub struct TgwVpcDetail {
    pub subnet_ids: Vec<String>,
    /// Keeps a flow on one AZ's path through the attachment — what an
    /// inspection (firewall) VPC needs for return traffic.
    pub appliance_mode: bool,
    pub dns_support: bool,
    pub sg_referencing: bool,
}

#[derive(Debug, Clone, Default)]
pub struct TgwPeeringSide {
    pub tgw_id: String,
    pub owner_id: String,
    pub region: String,
    pub core_network_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TgwPeeringDetail {
    pub requester: TgwPeeringSide,
    pub accepter: TgwPeeringSide,
    pub accepter_attachment_id: Option<String>,
    pub status_code: String,
    pub status_message: String,
    pub dynamic_routing: Option<bool>,
}

impl TgwPeeringDetail {
    /// The far side, from the point of view of the gateway `own_tgw`.
    pub fn peer(&self, own_tgw: &str) -> (&TgwPeeringSide, &'static str) {
        if self.requester.tgw_id == own_tgw {
            (&self.accepter, "requester")
        } else {
            (&self.requester, "accepter")
        }
    }
}

#[derive(Debug, Clone)]
pub struct TgwConnectDetail {
    /// The VPC / DX attachment the GRE tunnels ride on.
    pub transport_attachment_id: String,
    pub protocol: String,
    pub peers: std::result::Result<Vec<TgwConnectPeer>, String>,
}

#[derive(Debug, Clone)]
pub struct TgwConnectPeer {
    pub id: String,
    pub state: String,
    pub tgw_address: String,
    pub peer_address: String,
    pub inside_cidrs: Vec<String>,
    pub bgp: Vec<TgwBgpSession>,
}

#[derive(Debug, Clone)]
pub struct TgwBgpSession {
    pub tgw_asn: Option<i64>,
    pub peer_asn: Option<i64>,
    pub tgw_address: String,
    pub peer_address: String,
    pub status: String,
}

/// The attachment bundle: propagations and the kind's describe, in parallel.
pub async fn fetch_tgw_attachment_detail(
    client: Ec2Client,
    attachment_id: String,
    resource_kind: String,
) -> TgwAttachmentDetail {
    let (propagations, kind) = tokio::join!(
        fetch_attachment_propagations(&client, &attachment_id),
        fetch_kind_detail(&client, &attachment_id, &resource_kind),
    );
    TgwAttachmentDetail { propagations, kind }
}

async fn fetch_attachment_propagations(
    client: &Ec2Client,
    attachment_id: &str,
) -> std::result::Result<Vec<TgwAttachmentPropagation>, String> {
    let mut out = Vec::new();
    let mut next: Option<String> = None;
    loop {
        let mut req = client
            .get_transit_gateway_attachment_propagations()
            .transit_gateway_attachment_id(attachment_id);
        if let Some(t) = &next {
            req = req.next_token(t);
        }
        let resp = req.send().await.map_err(|e| crate::error::sdk_error_message(&e))?;
        out.extend(resp.transit_gateway_attachment_propagations().iter().map(|p| TgwAttachmentPropagation {
            route_table_id: p.transit_gateway_route_table_id().unwrap_or_default().to_string(),
            state: p.state().map(|s| s.as_str().to_string()).unwrap_or_default(),
        }));
        next = crate::aws::pagination::next_page_token(resp.next_token(), &next);
        if next.is_none() {
            return Ok(out);
        }
    }
}

fn enabled<T: AsRef<str>>(v: Option<T>) -> bool {
    v.is_some_and(|v| v.as_ref() == "enable")
}

async fn fetch_kind_detail(
    client: &Ec2Client,
    attachment_id: &str,
    resource_kind: &str,
) -> std::result::Result<TgwKindDetail, String> {
    match resource_kind {
        "vpc" => {
            let resp = client
                .describe_transit_gateway_vpc_attachments()
                .transit_gateway_attachment_ids(attachment_id)
                .send()
                .await
                .map_err(|e| crate::error::sdk_error_message(&e))?;
            let Some(a) = resp.transit_gateway_vpc_attachments().first() else {
                return Ok(TgwKindDetail::None);
            };
            let opts = a.options();
            Ok(TgwKindDetail::Vpc(TgwVpcDetail {
                subnet_ids: a.subnet_ids().to_vec(),
                appliance_mode: enabled(opts.and_then(|o| o.appliance_mode_support()).map(|v| v.as_str())),
                dns_support: enabled(opts.and_then(|o| o.dns_support()).map(|v| v.as_str())),
                sg_referencing: enabled(opts.and_then(|o| o.security_group_referencing_support()).map(|v| v.as_str())),
            }))
        }
        "peering" | "tgw-peering" => {
            let resp = client
                .describe_transit_gateway_peering_attachments()
                .transit_gateway_attachment_ids(attachment_id)
                .send()
                .await
                .map_err(|e| crate::error::sdk_error_message(&e))?;
            let Some(a) = resp.transit_gateway_peering_attachments().first() else {
                return Ok(TgwKindDetail::None);
            };
            let side = |i: Option<&aws_sdk_ec2::types::PeeringTgwInfo>| TgwPeeringSide {
                tgw_id: i.and_then(|i| i.transit_gateway_id()).unwrap_or_default().to_string(),
                owner_id: i.and_then(|i| i.owner_id()).unwrap_or_default().to_string(),
                region: i.and_then(|i| i.region()).unwrap_or_default().to_string(),
                core_network_id: i.and_then(|i| i.core_network_id()).map(str::to_string),
            };
            Ok(TgwKindDetail::Peering(TgwPeeringDetail {
                requester: side(a.requester_tgw_info()),
                accepter: side(a.accepter_tgw_info()),
                accepter_attachment_id: a.accepter_transit_gateway_attachment_id().map(str::to_string),
                status_code: a.status().and_then(|s| s.code()).unwrap_or_default().to_string(),
                status_message: a.status().and_then(|s| s.message()).unwrap_or_default().to_string(),
                dynamic_routing: a
                    .options()
                    .and_then(|o| o.dynamic_routing())
                    .map(|v| v.as_str() == "enable"),
            }))
        }
        "connect" => {
            let resp = client
                .describe_transit_gateway_connects()
                .transit_gateway_attachment_ids(attachment_id)
                .send()
                .await
                .map_err(|e| crate::error::sdk_error_message(&e))?;
            let Some(c) = resp.transit_gateway_connects().first() else {
                return Ok(TgwKindDetail::None);
            };
            Ok(TgwKindDetail::Connect(TgwConnectDetail {
                transport_attachment_id: c
                    .transport_transit_gateway_attachment_id()
                    .unwrap_or_default()
                    .to_string(),
                protocol: c
                    .options()
                    .and_then(|o| o.protocol())
                    .map(|p| p.as_str().to_string())
                    .unwrap_or_default(),
                peers: fetch_connect_peers(client, attachment_id).await,
            }))
        }
        _ => Ok(TgwKindDetail::None),
    }
}

async fn fetch_connect_peers(
    client: &Ec2Client,
    attachment_id: &str,
) -> std::result::Result<Vec<TgwConnectPeer>, String> {
    let filter = aws_sdk_ec2::types::Filter::builder()
        .name("transit-gateway-attachment-id")
        .values(attachment_id)
        .build();
    let mut out = Vec::new();
    let mut next: Option<String> = None;
    loop {
        let mut req = client.describe_transit_gateway_connect_peers().filters(filter.clone());
        if let Some(t) = &next {
            req = req.next_token(t);
        }
        let resp = req.send().await.map_err(|e| crate::error::sdk_error_message(&e))?;
        out.extend(resp.transit_gateway_connect_peers().iter().map(|p| {
            let cfg = p.connect_peer_configuration();
            TgwConnectPeer {
                id: p.transit_gateway_connect_peer_id().unwrap_or_default().to_string(),
                state: p.state().map(|s| s.as_str().to_string()).unwrap_or_default(),
                tgw_address: cfg.and_then(|c| c.transit_gateway_address()).unwrap_or_default().to_string(),
                peer_address: cfg.and_then(|c| c.peer_address()).unwrap_or_default().to_string(),
                inside_cidrs: cfg.map(|c| c.inside_cidr_blocks().to_vec()).unwrap_or_default(),
                bgp: cfg
                    .map(|c| c.bgp_configurations())
                    .unwrap_or_default()
                    .iter()
                    .map(|b| TgwBgpSession {
                        tgw_asn: b.transit_gateway_asn(),
                        peer_asn: b.peer_asn(),
                        tgw_address: b.transit_gateway_address().unwrap_or_default().to_string(),
                        peer_address: b.peer_address().unwrap_or_default().to_string(),
                        status: b.bgp_status().map(|s| s.as_str().to_string()).unwrap_or_default(),
                    })
                    .collect(),
            }
        }));
        next = crate::aws::pagination::next_page_token(resp.next_token(), &next);
        if next.is_none() {
            return Ok(out);
        }
    }
}

// ── CloudWatch metrics (`m` overlay) ──────────────────────────────────────────


