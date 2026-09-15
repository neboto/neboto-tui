use crate::aws::client::AwsClients;
use crate::aws::resource::{native_state_label, Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use aws_sdk_directconnect::Client as DxClient;
use std::any::Any;
use std::collections::HashMap;
use tokio::sync::mpsc;

/// Direct Connect service — four resource types over one heterogeneous list:
/// connections, virtual interfaces, LAGs, and DX gateways. The three
/// `describe_*` calls for connections/VIFs/LAGs return everything in one call
/// (no pagination) with tags inline; `DescribeDirectConnectGateways` paginates
/// via `next_token` and returns **no tags**. The operational signal is VIF
/// **BGP state** and connection/LAG **state**; gateways are control-plane
/// anchors (their traffic shows on the VIFs attached to them).
pub struct DirectConnectService {
    client: DxClient,
}

impl DirectConnectService {
    pub fn new(aws_clients: &AwsClients) -> Self {
        Self {
            client: aws_clients.directconnect_client(),
        }
    }
}

#[async_trait]
impl AwsService for DirectConnectService {
    fn service_type(&self) -> ServiceType {
        ServiceType::DirectConnect
    }

    fn name(&self) -> &str {
        "Direct Connect"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        self.list_resources_streaming(tx, ServiceType::DirectConnect)
            .await?;
        Ok(vec![])
    }

    async fn list_resources_streaming(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        let mut total = 0usize;

        // Phase 1: Connections (the core — fatal on error).
        match self.client.describe_connections().send().await {
            Ok(resp) => {
                let batch: Vec<Box<dyn Resource>> = resp
                    .connections()
                    .iter()
                    .map(|c| Box::new(DxConnection::from_sdk(c)) as Box<dyn Resource>)
                    .collect();
                total += batch.len();
                if !batch.is_empty() {
                    let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                        service: service_type,
                        resources: batch,
                        progress: LoadProgress {
                            loaded_count: total,
                            total_count: None,
                            status_message: Some("Loading connections…".to_string()),
                        },
                    });
                }
            }
            Err(e) => {
                let _ = event_tx.send(Event::ResourceLoadError {
                    service: service_type,
                    error: format!("Failed to list Direct Connect connections: {}", e),
                });
                return Ok(());
            }
        }

        // Phase 2: Virtual interfaces (surfaces errors instead of swallowing).
        match self.client.describe_virtual_interfaces().send().await {
            Ok(resp) => {
                let batch: Vec<Box<dyn Resource>> = resp
                    .virtual_interfaces()
                    .iter()
                    .map(|v| Box::new(DxVirtualInterface::from_sdk(v)) as Box<dyn Resource>)
                    .collect();
                total += batch.len();
                if !batch.is_empty() {
                    let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                        service: service_type,
                        resources: batch,
                        progress: LoadProgress {
                            loaded_count: total,
                            total_count: None,
                            status_message: Some("Loading virtual interfaces…".to_string()),
                        },
                    });
                }
            }
            Err(e) => {
                let _ = event_tx.send(Event::ResourceLoadWarning {
                    service: service_type,
                    warning: format!("virtual interfaces: {}", crate::error::sdk_error_message(&e)),
                });
            }
        }

        // Phase 3: LAGs (surfaces errors instead of swallowing).
        match self.client.describe_lags().send().await {
            Ok(resp) => {
                let batch: Vec<Box<dyn Resource>> = resp
                    .lags()
                    .iter()
                    .map(|l| Box::new(DxLag::from_sdk(l)) as Box<dyn Resource>)
                    .collect();
                total += batch.len();
                if !batch.is_empty() {
                    let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                        service: service_type,
                        resources: batch,
                        progress: LoadProgress {
                            loaded_count: total,
                            total_count: None,
                            status_message: Some("Loading LAGs…".to_string()),
                        },
                    });
                }
            }
            Err(e) => {
                let _ = event_tx.send(Event::ResourceLoadWarning {
                    service: service_type,
                    warning: format!("LAGs: {}", crate::error::sdk_error_message(&e)),
                });
            }
        }

        // Phase 4: DX gateways (paginated; surfaces errors instead of swallowing).
        let mut token: Option<String> = None;
        loop {
            let mut req = self.client.describe_direct_connect_gateways();
            if let Some(t) = &token {
                req = req.next_token(t);
            }
            match req.send().await {
                Ok(resp) => {
                    let batch: Vec<Box<dyn Resource>> = resp
                        .direct_connect_gateways()
                        .iter()
                        .map(|g| Box::new(DxGateway::from_sdk(g)) as Box<dyn Resource>)
                        .collect();
                    total += batch.len();
                    if !batch.is_empty() {
                        let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                            service: service_type,
                            resources: batch,
                            progress: LoadProgress {
                                loaded_count: total,
                                total_count: None,
                                status_message: Some("Loading DX gateways…".to_string()),
                            },
                        });
                    }
                    token = crate::aws::pagination::next_page_token(resp.next_token(), &token);
                    if token.is_none() {
                        break;
                    }
                }
                Err(e) => {
                    let _ = event_tx.send(Event::ResourceLoadWarning {
                        service: service_type,
                        warning: format!("DX gateways: {}", crate::error::sdk_error_message(&e)),
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

fn tags_of(tags: &[aws_sdk_directconnect::types::Tag]) -> HashMap<String, String> {
    tags.iter()
        .map(|t| (t.key().to_string(), t.value().unwrap_or_default().to_string()))
        .collect()
}

/// Append every tag key+value to a search string so fuzzy search matches the
/// Name tag shown in the list, not just the ids.
fn append_tags(mut s: String, tags: &HashMap<String, String>) -> String {
    for (k, v) in tags {
        s.push(' ');
        s.push_str(k);
        s.push(' ');
        s.push_str(v);
    }
    s
}

/// Name from the `Name` tag, else fall back to the supplied resource name/id.
fn name_or<'a>(tags: &'a HashMap<String, String>, fallback: &'a str) -> &'a str {
    tags.get("Name")
        .filter(|n| !n.is_empty())
        .map(|s| s.as_str())
        .unwrap_or(fallback)
}

// ── DxConnection ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct DxConnection {
    pub id: String,
    pub name: String,
    pub state: String, // available/down/pending/deleting/ordering/requested/…
    pub bandwidth: String,
    pub location: String,
    pub region: String,
    pub vlan: Option<i32>,
    pub partner: String,
    pub lag_id: Option<String>,
    pub jumbo_capable: bool,
    pub encryption_mode: String,
    pub mac_sec_capable: bool,
    pub aws_device: String,
    pub tags: HashMap<String, String>,
}

impl DxConnection {
    pub fn from_sdk(c: &aws_sdk_directconnect::types::Connection) -> Self {
        let tags = tags_of(c.tags());
        let id = c.connection_id().unwrap_or_default().to_string();
        let name = c.connection_name().unwrap_or_default().to_string();
        Self {
            name: if name.is_empty() { id.clone() } else { name },
            id,
            state: c
                .connection_state()
                .map(|s| s.as_str().to_string())
                .unwrap_or_default(),
            bandwidth: c.bandwidth().unwrap_or_default().to_string(),
            location: c.location().unwrap_or_default().to_string(),
            region: c.region().unwrap_or_default().to_string(),
            vlan: Some(c.vlan()).filter(|v| *v != 0),
            partner: c.partner_name().unwrap_or_default().to_string(),
            lag_id: c.lag_id().map(|s| s.to_string()).filter(|s| !s.is_empty()),
            jumbo_capable: c.jumbo_frame_capable().unwrap_or(false),
            encryption_mode: c.encryption_mode().unwrap_or_default().to_string(),
            mac_sec_capable: c.mac_sec_capable().unwrap_or(false),
            aws_device: c.aws_device_v2().unwrap_or_default().to_string(),
            tags,
        }
    }
}

crate::sections! {
    pub enum DxConnectionDetailSection,
    pub static DX_CONNECTION_SECTIONS = [
        Details "Details",
        VirtualInterfaces "Virtual Interfaces",
        Tags "Tags",
    ]
}

impl Resource for DxConnection {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&DX_CONNECTION_SECTIONS)
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws directconnect describe-connections --connection-id {}",
            crate::aws::resource::shell_quote(self.id())
        ))
    }

    fn id(&self) -> &str {
        &self.id
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn resource_type(&self) -> &str {
        "DX Connection"
    }
    fn state(&self) -> ResourceState {
        match self.state.as_str() {
            "available" => ResourceState::Available,
            "down" => ResourceState::Unavailable,
            "pending" | "requested" | "ordering" => ResourceState::Pending,
            "deleting" => ResourceState::Deleting,
            "deleted" | "rejected" => ResourceState::Terminated,
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
        append_tags(
            format!(
                "{} {} {} {} {}",
                self.id, self.name, self.location, self.partner, self.bandwidth
            ),
            &self.tags,
        )
    }
    fn details(&self) -> Vec<(String, String)> {
        let mut d = vec![
            ("ID".to_string(), self.id.clone()),
            ("Name".to_string(), self.name.clone()),
            ("State".to_string(), self.state.clone()),
            ("Bandwidth".to_string(), self.bandwidth.clone()),
            ("Location".to_string(), self.location.clone()),
        ];
        if let Some(v) = self.vlan {
            d.push(("VLAN".to_string(), v.to_string()));
        }
        if let Some(lag) = &self.lag_id {
            d.push(("LAG".to_string(), lag.clone()));
        }
        d
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{region}.console.aws.amazon.com/directconnect/v2/home?region={region}#/connections/{}",
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

// ── DxVirtualInterface ────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct DxBgpPeer {
    pub asn: i64,
    pub status: String, // up / down
    pub peer_state: String,
    pub address_family: String,
    pub amazon_address: String,
    pub customer_address: String,
    pub has_auth_key: bool,
}

#[derive(Debug, Clone)]
pub struct DxVirtualInterface {
    pub id: String,
    pub name: String,
    pub kind: String, // private / public / transit
    pub state: String,
    pub connection_id: String,
    pub vlan: Option<i32>,
    pub bgp_asn: Option<i64>,
    pub bgp_status: String, // up / down (rolled up from peers)
    pub address_family: String,
    pub amazon_address: String,
    pub customer_address: String,
    pub mtu: Option<i32>,
    pub gateway_id: String,
    pub peers: Vec<DxBgpPeer>,
    pub tags: HashMap<String, String>,
}

impl DxVirtualInterface {
    pub fn from_sdk(v: &aws_sdk_directconnect::types::VirtualInterface) -> Self {
        let tags = tags_of(v.tags());
        let id = v.virtual_interface_id().unwrap_or_default().to_string();
        let name = v.virtual_interface_name().unwrap_or_default().to_string();

        let peers: Vec<DxBgpPeer> = v
            .bgp_peers()
            .iter()
            .map(|p| DxBgpPeer {
                asn: p.asn_long().unwrap_or(p.asn() as i64),
                status: p
                    .bgp_status()
                    .map(|s| s.as_str().to_string())
                    .unwrap_or_default(),
                peer_state: p
                    .bgp_peer_state()
                    .map(|s| s.as_str().to_string())
                    .unwrap_or_default(),
                address_family: p
                    .address_family()
                    .map(|a| a.as_str().to_string())
                    .unwrap_or_default(),
                amazon_address: p.amazon_address().unwrap_or_default().to_string(),
                customer_address: p.customer_address().unwrap_or_default().to_string(),
                has_auth_key: p.auth_key().map(|k| !k.is_empty()).unwrap_or(false),
            })
            .collect();

        // Roll peer status up: "up" iff there's at least one peer and all are up.
        let bgp_status = if peers.is_empty() {
            String::new()
        } else if peers.iter().all(|p| p.status == "up") {
            "up".to_string()
        } else {
            "down".to_string()
        };

        // A directly-attached gateway (DX gateway or virtual gateway).
        let gateway_id = v
            .direct_connect_gateway_id()
            .filter(|s| !s.is_empty())
            .or_else(|| v.virtual_gateway_id().filter(|s| !s.is_empty()))
            .unwrap_or_default()
            .to_string();

        Self {
            name: if name.is_empty() { id.clone() } else { name },
            id,
            kind: v.virtual_interface_type().unwrap_or_default().to_string(),
            state: v
                .virtual_interface_state()
                .map(|s| s.as_str().to_string())
                .unwrap_or_default(),
            connection_id: v.connection_id().unwrap_or_default().to_string(),
            vlan: Some(v.vlan()).filter(|x| *x != 0),
            bgp_asn: v.asn_long().or_else(|| Some(v.asn() as i64)).filter(|x| *x != 0),
            bgp_status,
            address_family: v
                .address_family()
                .map(|a| a.as_str().to_string())
                .unwrap_or_default(),
            amazon_address: v.amazon_address().unwrap_or_default().to_string(),
            customer_address: v.customer_address().unwrap_or_default().to_string(),
            mtu: v.mtu(),
            gateway_id,
            peers,
            tags,
        }
    }
}

crate::sections! {
    pub enum DxVifDetailSection,
    pub static DX_VIF_SECTIONS = [
        Details "Details",
        Bgp "BGP",
        Tags "Tags",
    ]
}

impl Resource for DxVirtualInterface {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&DX_VIF_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.id
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn resource_type(&self) -> &str {
        "DX Virtual Interface"
    }
    fn state(&self) -> ResourceState {
        // BGP down on an "available" VIF is the thing to catch — degrade the dot.
        match self.state.as_str() {
            "available" => {
                if self.bgp_status == "up" || self.peers.is_empty() {
                    ResourceState::Available
                } else {
                    ResourceState::Unavailable
                }
            }
            "down" => ResourceState::Unavailable,
            "pending" | "confirming" | "verifying" => ResourceState::Pending,
            "deleting" => ResourceState::Deleting,
            "deleted" | "rejected" => ResourceState::Terminated,
            other => ResourceState::Unknown(other.to_string()),
        }
    }

    fn state_label(&self) -> String {
        // "available" with BGP down is the degraded case state() flags red.
        if self.state == "available" && !(self.bgp_status == "up" || self.peers.is_empty()) {
            return "bgp down".to_string();
        }
        native_state_label(&self.state, || self.state())
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        append_tags(
            format!(
                "{} {} {} {} {}",
                self.id, self.name, self.kind, self.connection_id, self.gateway_id
            ),
            &self.tags,
        )
    }
    fn details(&self) -> Vec<(String, String)> {
        let mut d = vec![
            ("ID".to_string(), self.id.clone()),
            ("Name".to_string(), self.name.clone()),
            ("Type".to_string(), self.kind.clone()),
            ("State".to_string(), self.state.clone()),
            ("BGP".to_string(), self.bgp_status.clone()),
            ("Connection".to_string(), self.connection_id.clone()),
        ];
        if let Some(v) = self.vlan {
            d.push(("VLAN".to_string(), v.to_string()));
        }
        if let Some(asn) = self.bgp_asn {
            d.push(("BGP ASN".to_string(), asn.to_string()));
        }
        d
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{region}.console.aws.amazon.com/directconnect/v2/home?region={region}#/virtual-interfaces/{}",
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

// ── DxLag ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct DxLag {
    pub id: String,
    pub name: String,
    pub state: String,
    pub bandwidth: String,
    pub location: String,
    pub region: String,
    pub connections_count: usize,
    pub minimum_links: i32,
    pub jumbo_capable: bool,
    pub tags: HashMap<String, String>,
}

impl DxLag {
    pub fn from_sdk(l: &aws_sdk_directconnect::types::Lag) -> Self {
        let tags = tags_of(l.tags());
        let id = l.lag_id().unwrap_or_default().to_string();
        let name = l.lag_name().unwrap_or_default().to_string();
        Self {
            name: if name.is_empty() { id.clone() } else { name },
            id,
            state: l
                .lag_state()
                .map(|s| s.as_str().to_string())
                .unwrap_or_default(),
            bandwidth: l.connections_bandwidth().unwrap_or_default().to_string(),
            location: l.location().unwrap_or_default().to_string(),
            region: l.region().unwrap_or_default().to_string(),
            connections_count: l.connections().len(),
            minimum_links: l.minimum_links(),
            jumbo_capable: l.jumbo_frame_capable().unwrap_or(false),
            tags,
        }
    }
}

crate::sections! {
    pub enum DxLagDetailSection,
    pub static DX_LAG_SECTIONS = [
        Overview "Overview",
        Connections "Connections",
        Tags "Tags",
    ]
}

impl Resource for DxLag {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&DX_LAG_SECTIONS)
    }

    fn id(&self) -> &str {
        &self.id
    }
    fn name(&self) -> &str {
        name_or(&self.tags, &self.name)
    }
    fn resource_type(&self) -> &str {
        "DX LAG"
    }
    fn state(&self) -> ResourceState {
        match self.state.as_str() {
            "available" => ResourceState::Available,
            "down" => ResourceState::Unavailable,
            "pending" | "requested" => ResourceState::Pending,
            "deleting" => ResourceState::Deleting,
            "deleted" => ResourceState::Terminated,
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
        append_tags(
            format!("{} {} {}", self.id, self.name, self.location),
            &self.tags,
        )
    }
    fn details(&self) -> Vec<(String, String)> {
        let mut d = vec![
            ("ID".to_string(), self.id.clone()),
            ("Name".to_string(), self.name.clone()),
            ("State".to_string(), self.state.clone()),
            ("Bandwidth".to_string(), self.bandwidth.clone()),
            ("Location".to_string(), self.location.clone()),
            ("Region".to_string(), self.region.clone()),
            ("Connections".to_string(), self.connections_count.to_string()),
            ("Minimum Links".to_string(), self.minimum_links.to_string()),
        ];
        if self.jumbo_capable {
            d.push(("Jumbo Frames".to_string(), "capable".to_string()));
        }
        d
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{region}.console.aws.amazon.com/directconnect/v2/home?region={region}#/lags/{}",
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

// ── DxGateway ─────────────────────────────────────────────────────────────────

/// A Direct Connect gateway — the global anchor VIFs attach to and
/// VGWs/TGWs associate with. `DescribeDirectConnectGateways` returns no
/// tags, so `tags` is always empty.
#[derive(Debug, Clone)]
pub struct DxGateway {
    pub id: String,
    pub name: String,
    pub state: String, // pending / available / deleting / deleted
    pub amazon_side_asn: Option<i64>,
    pub owner_account: String,
    pub state_change_error: Option<String>,
    pub tags: HashMap<String, String>,
}

impl DxGateway {
    pub fn from_sdk(g: &aws_sdk_directconnect::types::DirectConnectGateway) -> Self {
        let id = g.direct_connect_gateway_id().unwrap_or_default().to_string();
        let name = g
            .direct_connect_gateway_name()
            .unwrap_or_default()
            .to_string();
        Self {
            name: if name.is_empty() { id.clone() } else { name },
            id,
            state: g
                .direct_connect_gateway_state()
                .map(|s| s.as_str().to_string())
                .unwrap_or_default(),
            amazon_side_asn: g.amazon_side_asn().filter(|a| *a != 0),
            owner_account: g.owner_account().unwrap_or_default().to_string(),
            state_change_error: g
                .state_change_error()
                .map(|s| s.to_string())
                .filter(|s| !s.is_empty()),
            tags: HashMap::new(),
        }
    }
}

crate::sections! {
    pub enum DxGatewayDetailSection,
    pub static DX_GATEWAY_SECTIONS = [
        Overview "Overview",
        Associations "Associations" => crate::app::App::trigger_dx_gw_associations_load,
        Attachments "Attachments" => crate::app::App::trigger_dx_gw_attachments_load,
    ]
}

impl Resource for DxGateway {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&DX_GATEWAY_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.id
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn resource_type(&self) -> &str {
        "DX Gateway"
    }
    fn state(&self) -> ResourceState {
        match self.state.as_str() {
            "available" => ResourceState::Available,
            "pending" => ResourceState::Pending,
            "deleting" => ResourceState::Deleting,
            "deleted" => ResourceState::Terminated,
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
        format!("{} {} {}", self.id, self.name, self.owner_account)
    }
    fn details(&self) -> Vec<(String, String)> {
        let mut d = vec![
            ("ID".to_string(), self.id.clone()),
            ("Name".to_string(), self.name.clone()),
            ("State".to_string(), self.state.clone()),
            ("Owner Account".to_string(), self.owner_account.clone()),
        ];
        if let Some(asn) = self.amazon_side_asn {
            d.push(("Amazon-side ASN".to_string(), asn.to_string()));
        }
        d
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{region}.console.aws.amazon.com/directconnect/v2/home?region={region}#/dxgateways/{}",
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

// ── DX gateway lazy sections: associations + attachments ─────────────────────

/// One VGW/TGW association on a DX gateway (the route side of the graph).
#[derive(Debug, Clone)]
pub struct DxGwAssociation {
    pub association_id: String,
    pub state: String, // associating / associated / disassociating / updating / …
    pub gateway_type: String, // virtualPrivateGateway / transitGateway
    pub gateway_id: String,
    pub gateway_owner: String,
    pub gateway_region: String,
    pub allowed_prefixes: Vec<String>,
    pub state_change_error: Option<String>,
}


pub async fn fetch_dx_gateway_associations(
    client: DxClient,
    gateway_id: String,
) -> Result<Vec<DxGwAssociation>> {
    let mut out = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut req = client
            .describe_direct_connect_gateway_associations()
            .direct_connect_gateway_id(&gateway_id);
        if let Some(t) = &token {
            req = req.next_token(t);
        }
        let resp = req.send().await.map_err(|e| {
            crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e))
        })?;
        for a in resp.direct_connect_gateway_associations() {
            // New-style associations carry an AssociatedGateway; legacy VGW
            // associations only fill the virtual_gateway_* fields.
            let (gw_type, gw_id, gw_owner, gw_region) = match a.associated_gateway() {
                Some(g) => (
                    g.r#type().map(|t| t.as_str().to_string()).unwrap_or_default(),
                    g.id().unwrap_or_default().to_string(),
                    g.owner_account().unwrap_or_default().to_string(),
                    g.region().unwrap_or_default().to_string(),
                ),
                None => (
                    "virtualPrivateGateway".to_string(),
                    a.virtual_gateway_id().unwrap_or_default().to_string(),
                    a.virtual_gateway_owner_account()
                        .unwrap_or_default()
                        .to_string(),
                    a.virtual_gateway_region().unwrap_or_default().to_string(),
                ),
            };
            out.push(DxGwAssociation {
                association_id: a.association_id().unwrap_or_default().to_string(),
                state: a
                    .association_state()
                    .map(|s| s.as_str().to_string())
                    .unwrap_or_default(),
                gateway_type: gw_type,
                gateway_id: gw_id,
                gateway_owner: gw_owner,
                gateway_region: gw_region,
                allowed_prefixes: a
                    .allowed_prefixes_to_direct_connect_gateway()
                    .iter()
                    .filter_map(|p| p.cidr().map(|c| c.to_string()))
                    .collect(),
                state_change_error: a
                    .state_change_error()
                    .map(|s| s.to_string())
                    .filter(|s| !s.is_empty()),
            });
        }
        token = crate::aws::pagination::next_page_token(resp.next_token(), &token);
        if token.is_none() {
            break;
        }
    }
    Ok(out)
}

/// One VIF attached to a DX gateway (the traffic side of the graph).
#[derive(Debug, Clone)]
pub struct DxGwAttachment {
    pub vif_id: String,
    pub vif_region: String,
    pub vif_owner: String,
    pub state: String, // attaching / attached / detaching / detached
    pub attachment_type: String, // TransitVirtualInterface / PrivateVirtualInterface
    pub state_change_error: Option<String>,
}


pub async fn fetch_dx_gateway_attachments(
    client: DxClient,
    gateway_id: String,
) -> Result<Vec<DxGwAttachment>> {
    let mut out = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut req = client
            .describe_direct_connect_gateway_attachments()
            .direct_connect_gateway_id(&gateway_id);
        if let Some(t) = &token {
            req = req.next_token(t);
        }
        let resp = req.send().await.map_err(|e| {
            crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e))
        })?;
        for a in resp.direct_connect_gateway_attachments() {
            out.push(DxGwAttachment {
                vif_id: a.virtual_interface_id().unwrap_or_default().to_string(),
                vif_region: a.virtual_interface_region().unwrap_or_default().to_string(),
                vif_owner: a
                    .virtual_interface_owner_account()
                    .unwrap_or_default()
                    .to_string(),
                state: a
                    .attachment_state()
                    .map(|s| s.as_str().to_string())
                    .unwrap_or_default(),
                attachment_type: a
                    .attachment_type()
                    .map(|s| s.as_str().to_string())
                    .unwrap_or_default(),
                state_change_error: a
                    .state_change_error()
                    .map(|s| s.to_string())
                    .filter(|s| !s.is_empty()),
            });
        }
        token = crate::aws::pagination::next_page_token(resp.next_token(), &token);
        if token.is_none() {
            break;
        }
    }
    Ok(out)
}

// ── CloudWatch metrics (`m` overlay) — AWS/DX, dimension ConnectionId ─────────

pub use crate::aws::services::ec2::{parse_metric_datapoints, MetricsTimeRange};

#[derive(Debug, Clone)]
pub struct DxMetricsData {
    pub time_range: MetricsTimeRange,
    pub state: Vec<(f64, f64)>,
    pub bps_egress: Vec<(f64, f64)>,
    pub bps_ingress: Vec<(f64, f64)>,
    pub pps_egress: Vec<(f64, f64)>,
    pub pps_ingress: Vec<(f64, f64)>,
    pub error_count: Vec<(f64, f64)>,
    pub light_level_tx: Vec<(f64, f64)>,
    pub light_level_rx: Vec<(f64, f64)>,
    pub x_max: f64,
}

#[derive(Debug, Clone)]
pub enum DxMetricsState {
    Loading,
    Loaded(Box<DxMetricsData>),
}

/// Pull `AWS/DX` metrics for one physical connection. `ConnectionState`
/// (Minimum — any 0 sample in the period means the link dropped) and error
/// count are the health signals; bps in/out is utilization vs the port speed;
/// light levels (dBm, fiber only) foreshadow physical-layer degradation.
pub async fn fetch_dx_metrics(
    cw: aws_sdk_cloudwatch::Client,
    connection_id: String,
    time_range: MetricsTimeRange,
) -> crate::error::Result<DxMetricsData> {
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
            .namespace("AWS/DX")
            .metric_name(name)
            .dimensions(
                Dimension::builder()
                    .name("ConnectionId")
                    .value(&connection_id)
                    .build(),
            )
            .start_time(start_dt)
            .end_time(end_dt)
            .period(period)
            .set_statistics(Some(vec![stat]))
            .send()
    };

    let (state, egress, ingress, pps_e, pps_i, errors, light_tx, light_rx) = tokio::join!(
        metric("ConnectionState", Statistic::Minimum),
        metric("ConnectionBpsEgress", Statistic::Average),
        metric("ConnectionBpsIngress", Statistic::Average),
        metric("ConnectionPpsEgress", Statistic::Average),
        metric("ConnectionPpsIngress", Statistic::Average),
        metric("ConnectionErrorCount", Statistic::Sum),
        metric("ConnectionLightLevelTx", Statistic::Average),
        metric("ConnectionLightLevelRx", Statistic::Average),
    );

    // ConnectionState is queried with Minimum, which parse_metric_datapoints
    // doesn't read — map it separately.
    let parse_min = |resp: std::result::Result<
        aws_sdk_cloudwatch::operation::get_metric_statistics::GetMetricStatisticsOutput,
        _,
    >|
     -> Vec<(f64, f64)> {
        let mut pts: Vec<(f64, f64)> = match resp {
            Ok(r) => r
                .datapoints()
                .iter()
                .filter_map(|dp| {
                    Some((dp.timestamp()?.secs() as f64 - start as f64, dp.minimum()?))
                })
                .collect(),
            Err(_) => vec![],
        };
        pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        pts
    };

    Ok(DxMetricsData {
        time_range,
        state: parse_min(state),
        bps_egress: parse_metric_datapoints(egress, start),
        bps_ingress: parse_metric_datapoints(ingress, start),
        pps_egress: parse_metric_datapoints(pps_e, start),
        pps_ingress: parse_metric_datapoints(pps_i, start),
        error_count: parse_metric_datapoints(errors, start),
        light_level_tx: parse_metric_datapoints(light_tx, start),
        light_level_rx: parse_metric_datapoints(light_rx, start),
        x_max: time_range.duration_secs() as f64,
    })
}

// ── Per-VIF traffic metrics — AWS/DX, dims ConnectionId + VirtualInterfaceId ──

#[derive(Debug, Clone)]
pub struct DxVifMetricsData {
    pub time_range: MetricsTimeRange,
    pub bps_egress: Vec<(f64, f64)>,
    pub bps_ingress: Vec<(f64, f64)>,
    pub pps_egress: Vec<(f64, f64)>,
    pub pps_ingress: Vec<(f64, f64)>,
    pub x_max: f64,
}

#[derive(Debug, Clone)]
pub enum DxVifMetricsState {
    Loading,
    Loaded(DxVifMetricsData),
}

/// Pull per-virtual-interface traffic (`AWS/DX`, dims `ConnectionId` +
/// `VirtualInterfaceId`) — bps/pps in both directions. This is where "data
/// flowing" actually shows per workload: the connection-level series
/// aggregates every VIF on the port. For a VIF on a LAG, `connection_id`
/// is the LAG id, which is exactly what CloudWatch expects in the dimension.
pub async fn fetch_dx_vif_metrics(
    cw: aws_sdk_cloudwatch::Client,
    connection_id: String,
    vif_id: String,
    time_range: MetricsTimeRange,
) -> crate::error::Result<DxVifMetricsData> {
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
            .namespace("AWS/DX")
            .metric_name(name)
            .dimensions(
                Dimension::builder()
                    .name("ConnectionId")
                    .value(&connection_id)
                    .build(),
            )
            .dimensions(
                Dimension::builder()
                    .name("VirtualInterfaceId")
                    .value(&vif_id)
                    .build(),
            )
            .start_time(start_dt)
            .end_time(end_dt)
            .period(period)
            .set_statistics(Some(vec![Statistic::Average]))
            .send()
    };

    let (bps_e, bps_i, pps_e, pps_i) = tokio::join!(
        metric("VirtualInterfaceBpsEgress"),
        metric("VirtualInterfaceBpsIngress"),
        metric("VirtualInterfacePpsEgress"),
        metric("VirtualInterfacePpsIngress"),
    );

    Ok(DxVifMetricsData {
        time_range,
        bps_egress: parse_metric_datapoints(bps_e, start),
        bps_ingress: parse_metric_datapoints(bps_i, start),
        pps_egress: parse_metric_datapoints(pps_e, start),
        pps_ingress: parse_metric_datapoints(pps_i, start),
        x_max: time_range.duration_secs() as f64,
    })
}
