use crate::aws::client::AwsClients;
use crate::aws::resource::sg_refs;
use crate::aws::resource::{native_state_label, Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use aws_sdk_ec2::Client as Ec2Client;
use std::any::Any;
use std::collections::HashMap;
use tokio::sync::mpsc;

pub struct VpcService {
    client: Ec2Client,
}

impl VpcService {
    pub fn new(aws_clients: &AwsClients) -> Self {
        Self {
            client: aws_clients.ec2_vpc_client(),
        }
    }
}

#[async_trait]
impl AwsService for VpcService {
    fn service_type(&self) -> ServiceType {
        ServiceType::VPC
    }

    fn name(&self) -> &str {
        "VPC Resources"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        let mut resources: Vec<Box<dyn Resource>> = Vec::new();

        // Paginated like the streaming path — a bare `.send()` caps at 1000.
        // The first two types propagate errors; the rest are best-effort
        // (stop quietly on the first failed page).
        let mut vpc_pages = self.client.describe_vpcs().into_paginator().send();
        while let Some(page) = vpc_pages.next().await {
            for vpc in page?.vpcs() {
                resources.push(Box::new(Vpc::from_sdk(vpc)) as Box<dyn Resource>);
            }
        }

        let mut subnet_pages = self.client.describe_subnets().into_paginator().send();
        while let Some(page) = subnet_pages.next().await {
            for subnet in page?.subnets() {
                resources.push(Box::new(Subnet::from_sdk(subnet)) as Box<dyn Resource>);
            }
        }

        macro_rules! collect_pages {
            ($req:expr, $items:ident, $type:ty) => {{
                let mut pages = $req.into_paginator().send();
                while let Some(Ok(p)) = pages.next().await {
                    for x in p.$items() {
                        resources.push(Box::new(<$type>::from_sdk(x)) as Box<dyn Resource>);
                    }
                }
            }};
        }

        collect_pages!(self.client.describe_route_tables(), route_tables, RouteTable);
        collect_pages!(
            self.client.describe_internet_gateways(),
            internet_gateways,
            InternetGateway
        );
        collect_pages!(self.client.describe_nat_gateways(), nat_gateways, NatGateway);
        collect_pages!(
            self.client.describe_security_groups(),
            security_groups,
            VpcSecurityGroup
        );
        collect_pages!(self.client.describe_network_acls(), network_acls, NetworkAcl);

        Ok(resources)
    }

    async fn list_resources_streaming(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        let mut total_loaded = 0;

        // One paginated, warn-on-error phase: streams every page as its own
        // batch. The EC2 describes return at most 1000 items per page — a
        // bare `.send()` silently truncated big accounts (>1000 SGs/subnets).
        macro_rules! stream_phase {
            ($req:expr, $items:ident, $type:ty, $label:expr) => {{
                let mut pages = $req.into_paginator().send();
                while let Some(page) = pages.next().await {
                    match page {
                        Ok(p) => {
                            let batch: Vec<Box<dyn Resource>> = p
                                .$items()
                                .iter()
                                .map(|x| Box::new(<$type>::from_sdk(x)) as Box<dyn Resource>)
                                .collect();
                            let count = batch.len();
                            total_loaded += count;
                            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                                service: service_type,
                                resources: batch,
                                progress: LoadProgress {
                                    loaded_count: total_loaded,
                                    total_count: None,
                                    status_message: Some(format!(
                                        "Loaded {} {}...",
                                        count, $label
                                    )),
                                },
                            });
                        }
                        Err(e) => {
                            let _ = event_tx.send(Event::ResourceLoadWarning {
                                service: service_type,
                                warning: format!(
                                    "{}: {}",
                                    $label,
                                    crate::error::sdk_error_message(&e)
                                ),
                            });
                            break;
                        }
                    }
                }
            }};
        }

        // Phase 1: VPCs (paginated). A nothing-loaded failure is fatal — no
        // later phase can stream; a later-page failure degrades to a warning.
        {
            let mut pages = self.client.describe_vpcs().into_paginator().send();
            let mut vpcs_loaded = 0usize;
            while let Some(page) = pages.next().await {
                match page {
                    Ok(p) => {
                        let batch: Vec<Box<dyn Resource>> = p
                            .vpcs()
                            .iter()
                            .map(|v| Box::new(Vpc::from_sdk(v)) as Box<dyn Resource>)
                            .collect();
                        vpcs_loaded += batch.len();
                        total_loaded += batch.len();
                        let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                            service: service_type,
                            resources: batch,
                            progress: LoadProgress {
                                loaded_count: total_loaded,
                                total_count: None,
                                status_message: Some(format!(
                                    "Loaded {} VPCs. Loading subnets...",
                                    vpcs_loaded
                                )),
                            },
                        });
                    }
                    Err(e) => {
                        if vpcs_loaded == 0 {
                            let _ = event_tx.send(Event::ResourceLoadError {
                                service: service_type,
                                error: crate::error::sdk_error_message(&e),
                            });
                            return Err(e.into());
                        }
                        let _ = event_tx.send(Event::ResourceLoadWarning {
                            service: service_type,
                            warning: format!("VPCs: {}", crate::error::sdk_error_message(&e)),
                        });
                        break;
                    }
                }
            }
        }

        // Phase 2: Subnets
        stream_phase!(self.client.describe_subnets(), subnets, Subnet, "subnets");

        // Phase 3: Route Tables
        stream_phase!(
            self.client.describe_route_tables(),
            route_tables,
            RouteTable,
            "route tables"
        );

        // Phase 4: Internet Gateways
        stream_phase!(
            self.client.describe_internet_gateways(),
            internet_gateways,
            InternetGateway,
            "internet gateways"
        );

        // Phase 5: NAT Gateways
        stream_phase!(
            self.client.describe_nat_gateways(),
            nat_gateways,
            NatGateway,
            "NAT gateways"
        );

        // Phase 6: Security Groups
        stream_phase!(
            self.client.describe_security_groups(),
            security_groups,
            VpcSecurityGroup,
            "security groups"
        );

        // Phase 7: Network ACLs
        stream_phase!(
            self.client.describe_network_acls(),
            network_acls,
            NetworkAcl,
            "network ACLs"
        );

        // Phase 8: VPC Endpoints (PrivateLink)
        stream_phase!(
            self.client.describe_vpc_endpoints(),
            vpc_endpoints,
            VpcEndpoint,
            "VPC endpoints"
        );

        // Phase 9: Site-to-Site VPN — customer gateways stream as their own
        // rows AND feed the ip/asn map the connections join against.
        let mut cgw_map: HashMap<String, (Option<String>, Option<String>)> = HashMap::new();
        match self.client.describe_customer_gateways().send().await {
            Ok(resp) => {
                let mut batch: Vec<Box<dyn Resource>> = Vec::new();
                for c in resp.customer_gateways() {
                    if let Some(id) = c.customer_gateway_id() {
                        cgw_map.insert(
                            id.to_string(),
                            (
                                c.ip_address().map(|s| s.to_string()),
                                c.bgp_asn().map(|s| s.to_string()),
                            ),
                        );
                    }
                    batch.push(Box::new(CustomerGateway::from_sdk(c)) as Box<dyn Resource>);
                }
                let count = batch.len();
                total_loaded += count;
                let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                    service: service_type,
                    resources: batch,
                    progress: LoadProgress {
                        loaded_count: total_loaded,
                        total_count: None,
                        status_message: Some(format!("Loaded {} customer gateways...", count)),
                    },
                });
            }
            Err(e) => {
                let _ = event_tx.send(Event::ResourceLoadWarning {
                    service: service_type,
                    warning: format!("Customer gateways: {}", crate::error::sdk_error_message(&e)),
                });
            }
        }
        match self.client.describe_vpn_connections().send().await {
            Ok(resp) => {
                let batch: Vec<Box<dyn Resource>> = resp
                    .vpn_connections()
                    .iter()
                    .map(|v| Box::new(VpnConnection::from_sdk(v, &cgw_map)) as Box<dyn Resource>)
                    .collect();
                let count = batch.len();
                total_loaded += count;
                let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                    service: service_type,
                    resources: batch,
                    progress: LoadProgress {
                        loaded_count: total_loaded,
                        total_count: None,
                        status_message: Some(format!("Loaded {} VPN connections...", count)),
                    },
                });
            }
            Err(e) => {
                let _ = event_tx.send(Event::ResourceLoadWarning {
                    service: service_type,
                    warning: format!("VPN connections: {}", crate::error::sdk_error_message(&e)),
                });
            }
        }

        // Phase 10: DHCP options sets (best-effort)
        stream_phase!(
            self.client.describe_dhcp_options(),
            dhcp_options,
            DhcpOptionsSet,
            "DHCP options sets"
        );

        // Phase 11: VPC peering connections (best-effort)
        stream_phase!(
            self.client.describe_vpc_peering_connections(),
            vpc_peering_connections,
            VpcPeering,
            "peering connections"
        );

        // Phase 12: Managed prefix lists (best-effort — includes the
        // AWS-managed com.amazonaws.* lists, marked is_noise)
        stream_phase!(
            self.client.describe_managed_prefix_lists(),
            prefix_lists,
            PrefixList,
            "prefix lists"
        );

        // Phase 13: Egress-only internet gateways (best-effort, IPv6 outbound)
        stream_phase!(
            self.client.describe_egress_only_internet_gateways(),
            egress_only_internet_gateways,
            EgressOnlyIgw,
            "egress-only IGWs"
        );

        // Phase 14: Virtual private gateways (best-effort)
        match self.client.describe_vpn_gateways().send().await {
            Ok(resp) => {
                let batch: Vec<Box<dyn Resource>> = resp
                    .vpn_gateways()
                    .iter()
                    .map(|g| Box::new(VpnGateway::from_sdk(g)) as Box<dyn Resource>)
                    .collect();
                let count = batch.len();
                total_loaded += count;
                let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                    service: service_type,
                    resources: batch,
                    progress: LoadProgress {
                        loaded_count: total_loaded,
                        total_count: None,
                        status_message: Some(format!("Loaded {} VPN gateways...", count)),
                    },
                });
            }
            Err(e) => {
                let _ = event_tx.send(Event::ResourceLoadWarning {
                    service: service_type,
                    warning: format!("VPN gateways: {}", crate::error::sdk_error_message(&e)),
                });
            }
        }

        // Send completion event
        let _ = event_tx.send(Event::ResourcesFullyLoaded {
            service: service_type,
            total_count: total_loaded,
        });

        Ok(())
    }

    async fn get_resource_details(&self, id: &str) -> Result<Box<dyn Resource>> {
        // Try to find as VPC first
        let vpcs_response = self.client.describe_vpcs().vpc_ids(id).send().await;
        if let Ok(response) = vpcs_response {
            if let Some(vpc) = response.vpcs().first() {
                return Ok(Box::new(Vpc::from_sdk(vpc)) as Box<dyn Resource>);
            }
        }

        // Try as subnet
        let subnets_response = self.client.describe_subnets().subnet_ids(id).send().await;
        if let Ok(response) = subnets_response {
            if let Some(subnet) = response.subnets().first() {
                return Ok(Box::new(Subnet::from_sdk(subnet)) as Box<dyn Resource>);
            }
        }

        Err(crate::error::Error::ResourceNotFound(id.to_string()))
    }
}

// VPC Resource
#[derive(Clone, Debug)]
pub struct Vpc {
    pub vpc_id: String,
    pub cidr_block: String,
    /// Secondary IPv4 CIDRs (primary excluded): (cidr, association state).
    pub secondary_cidrs: Vec<(String, String)>,
    /// IPv6 CIDRs: (cidr, association state).
    pub ipv6_cidrs: Vec<(String, String)>,
    pub state: String,
    pub is_default: bool,
    /// Owning account — differs from the browsed account for RAM-shared VPCs.
    pub owner_id: Option<String>,
    pub tags: HashMap<String, String>,
    pub dhcp_options_id: Option<String>,
    pub instance_tenancy: Option<String>,
}

impl Vpc {
    pub fn from_sdk(vpc: &aws_sdk_ec2::types::Vpc) -> Self {
        let vpc_id = vpc.vpc_id().unwrap_or("unknown").to_string();
        let cidr_block = vpc.cidr_block().unwrap_or("unknown").to_string();
        // The association sets ride the DescribeVpcs response — no extra call.
        // Non-"associated" states (associating/failed/disassociating) are kept
        // so a broken secondary-CIDR association is visible.
        let secondary_cidrs = vpc
            .cidr_block_association_set()
            .iter()
            .filter(|a| a.cidr_block() != vpc.cidr_block())
            .filter_map(|a| {
                a.cidr_block().map(|c| {
                    (
                        c.to_string(),
                        a.cidr_block_state()
                            .and_then(|s| s.state())
                            .map(|s| s.as_str().to_string())
                            .unwrap_or_else(|| "associated".to_string()),
                    )
                })
            })
            .collect();
        let ipv6_cidrs = vpc
            .ipv6_cidr_block_association_set()
            .iter()
            .filter_map(|a| {
                a.ipv6_cidr_block().map(|c| {
                    (
                        c.to_string(),
                        a.ipv6_cidr_block_state()
                            .and_then(|s| s.state())
                            .map(|s| s.as_str().to_string())
                            .unwrap_or_else(|| "associated".to_string()),
                    )
                })
            })
            .collect();
        let state = vpc
            .state()
            .map(|s| s.as_str().to_string())
            .unwrap_or_else(|| "unknown".to_string());
        let is_default = vpc.is_default().unwrap_or(false);
        let owner_id = vpc.owner_id().map(|s| s.to_string());
        let dhcp_options_id = vpc.dhcp_options_id().map(|s| s.to_string());
        let instance_tenancy = vpc.instance_tenancy().map(|t| t.as_str().to_string());

        // Parse tags
        let mut tags = HashMap::new();
        for tag in vpc.tags() {
            if let (Some(key), Some(value)) = (tag.key(), tag.value()) {
                tags.insert(key.to_string(), value.to_string());
            }
        }

        Self {
            vpc_id,
            cidr_block,
            secondary_cidrs,
            ipv6_cidrs,
            state,
            is_default,
            owner_id,
            tags,
            dhcp_options_id,
            instance_tenancy,
        }
    }

    /// One display row per non-primary CIDR: ("Secondary CIDR"/"IPv6 CIDR",
    /// the cidr — annotated with a ⚠ state when the association isn't healthy).
    pub fn cidr_rows(&self) -> Vec<(String, String)> {
        let fmt = |cidr: &str, state: &str| {
            if state == "associated" {
                cidr.to_string()
            } else {
                format!("{} — ⚠ {}", cidr, state)
            }
        };
        let mut rows = Vec::new();
        for (cidr, state) in &self.secondary_cidrs {
            rows.push(("Secondary CIDR".to_string(), fmt(cidr, state)));
        }
        for (cidr, state) in &self.ipv6_cidrs {
            rows.push(("IPv6 CIDR".to_string(), fmt(cidr, state)));
        }
        rows
    }
}

crate::sections! {
    pub enum VpcDetailSection,
    pub static VPC_SECTIONS = [
        // Overview's DNS / DHCP group needs DescribeVpcAttribute.
        Overview "Overview" => crate::app::App::trigger_vpc_dns_attrs_load,
        Subnets "Subnets",
        Gateways "Gateways",
        FlowLogs "Flow Logs" => crate::app::App::trigger_vpc_flow_logs_load,
        Tags "Tags",
    ]
}

impl Resource for Vpc {
    fn references(&self) -> Vec<(String, String)> {
        let mut v = Vec::new();
        if let Some(x) = &self.dhcp_options_id { v.push(("DHCP Options".to_string(), x.clone())); }
        v
    }

    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&VPC_SECTIONS)
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws ec2 describe-vpcs --vpc-ids {}",
            crate::aws::resource::shell_quote(self.id())
        ))
    }

    fn id(&self) -> &str {
        &self.vpc_id
    }

    fn name(&self) -> &str {
        self.tags
            .get("Name")
            .map(|s| s.as_str())
            .unwrap_or(&self.vpc_id)
    }

    fn resource_type(&self) -> &str {
        "VPC"
    }

    fn state(&self) -> ResourceState {
        match self.state.as_str() {
            "available" => ResourceState::Available,
            "pending" => ResourceState::Pending,
            _ => ResourceState::Unknown(self.state.clone()),
        }
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        // Include all searchable fields for VPCs
        let mut search_parts = vec![
            self.vpc_id.clone(),
            self.name().to_string(),
            self.cidr_block.clone(),
            self.state.clone(),
        ];
        for (cidr, _) in &self.secondary_cidrs {
            search_parts.push(cidr.clone());
        }
        for (cidr, _) in &self.ipv6_cidrs {
            search_parts.push(cidr.clone());
        }
        if let Some(o) = &self.owner_id {
            search_parts.push(o.clone());
        }

        // Add tags
        for (k, v) in &self.tags {
            search_parts.push(format!("{}:{}", k, v));
        }

        search_parts.join(" ")
    }

    fn details(&self) -> Vec<(String, String)> {
        let mut details = vec![
            ("VPC ID".to_string(), self.vpc_id.clone()),
            ("CIDR Block".to_string(), self.cidr_block.clone()),
        ];
        details.extend(self.cidr_rows());
        details.push(("State".to_string(), self.state.clone()));
        details.push((
            "Default VPC".to_string(),
            if self.is_default { "Yes" } else { "No" }.to_string(),
        ));
        if let Some(owner) = &self.owner_id {
            details.push(("Owner".to_string(), owner.clone()));
        }

        if let Some(dhcp) = &self.dhcp_options_id {
            details.push(("DHCP Options".to_string(), dhcp.clone()));
        }

        if let Some(tenancy) = &self.instance_tenancy {
            details.push(("Instance Tenancy".to_string(), tenancy.clone()));
        }

        details
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{region}.console.aws.amazon.com/vpc/home?region={region}#VpcDetails:VpcId={}",
            self.vpc_id
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── VPC split-pane helper types ───────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct RouteEntry {
    pub destination: String,
    pub target: String,
    pub state: String,
    pub origin: String, // "Static" | "Built-in" | "Propagated"
}

#[derive(Clone, Debug)]
pub struct RouteAssociation {
    pub subnet_id: Option<String>,
    /// Edge association — the table is associated with an IGW/VGW instead of
    /// a subnet (ingress routing).
    pub gateway_id: Option<String>,
    #[allow(dead_code)]
    pub is_main: bool,
    pub state: String,
}

#[derive(Clone, Debug)]
pub struct AclEntry {
    pub rule_number: i32,
    pub protocol: String,
    pub action: String,
    pub cidr: String,
    pub port_range: String,
    #[allow(dead_code)]
    pub egress: bool,
}

#[derive(Clone, Debug)]
pub struct AclAssociation {
    pub subnet_id: String,
    pub association_id: String,
}

// Subnet Resource
#[derive(Clone, Debug)]
pub struct Subnet {
    pub subnet_id: String,
    pub vpc_id: String,
    pub cidr_block: String,
    pub availability_zone: String,
    /// Stable cross-account AZ name (e.g. use1-az4).
    pub availability_zone_id: Option<String>,
    pub available_ip_count: i32,
    pub state: String,
    /// Owning account — differs from the browsed account for RAM-shared subnets.
    pub owner_id: Option<String>,
    pub tags: HashMap<String, String>,
    pub map_public_ip: bool,
    pub default_for_az: bool,
    pub assign_ipv6_on_creation: bool,
    pub ipv6_cidr_blocks: Vec<String>,
    pub enable_dns64: bool,
    pub ipv6_native: bool,
}

impl Subnet {
    pub fn from_sdk(subnet: &aws_sdk_ec2::types::Subnet) -> Self {
        let subnet_id = subnet.subnet_id().unwrap_or("unknown").to_string();
        let vpc_id = subnet.vpc_id().unwrap_or("unknown").to_string();
        let cidr_block = subnet.cidr_block().unwrap_or("unknown").to_string();
        let availability_zone = subnet.availability_zone().unwrap_or("unknown").to_string();
        let availability_zone_id = subnet.availability_zone_id().map(|s| s.to_string());
        let available_ip_count = subnet.available_ip_address_count().unwrap_or(0);
        let owner_id = subnet.owner_id().map(|s| s.to_string());
        let state = subnet
            .state()
            .map(|s| s.as_str().to_string())
            .unwrap_or_else(|| "unknown".to_string());
        let map_public_ip = subnet.map_public_ip_on_launch().unwrap_or(false);
        let default_for_az = subnet.default_for_az().unwrap_or(false);
        let assign_ipv6_on_creation = subnet.assign_ipv6_address_on_creation().unwrap_or(false);
        let ipv6_cidr_blocks = subnet
            .ipv6_cidr_block_association_set()
            .iter()
            .filter_map(|a| a.ipv6_cidr_block().map(|s| s.to_string()))
            .collect();
        let enable_dns64 = subnet.enable_dns64().unwrap_or(false);
        let ipv6_native = subnet.ipv6_native().unwrap_or(false);

        // Parse tags
        let mut tags = HashMap::new();
        for tag in subnet.tags() {
            if let (Some(key), Some(value)) = (tag.key(), tag.value()) {
                tags.insert(key.to_string(), value.to_string());
            }
        }

        Self {
            subnet_id,
            vpc_id,
            cidr_block,
            availability_zone,
            availability_zone_id,
            available_ip_count,
            state,
            owner_id,
            tags,
            map_public_ip,
            default_for_az,
            assign_ipv6_on_creation,
            ipv6_cidr_blocks,
            enable_dns64,
            ipv6_native,
        }
    }
}

crate::sections! {
    pub enum SubnetDetailSection,
    pub static SUBNET_SECTIONS = [
        Details "Details",
        Network "Network",
        Routes "Routes",
        Tags "Tags",
    ]
}

impl Resource for Subnet {
    fn references(&self) -> Vec<(String, String)> {
        vec![("VPC".to_string(), self.vpc_id.clone())]
    }

    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&SUBNET_SECTIONS)
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws ec2 describe-subnets --subnet-ids {}",
            crate::aws::resource::shell_quote(self.id())
        ))
    }

    fn id(&self) -> &str {
        &self.subnet_id
    }

    fn name(&self) -> &str {
        self.tags
            .get("Name")
            .map(|s| s.as_str())
            .unwrap_or(&self.subnet_id)
    }

    fn resource_type(&self) -> &str {
        "Subnet"
    }

    fn state(&self) -> ResourceState {
        match self.state.as_str() {
            "available" => ResourceState::Available,
            "pending" => ResourceState::Pending,
            _ => ResourceState::Unknown(self.state.clone()),
        }
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        // Include all searchable fields for Subnets
        let mut search_parts = vec![
            self.subnet_id.clone(),
            self.name().to_string(),
            self.vpc_id.clone(),
            self.cidr_block.clone(),
            self.availability_zone.clone(),
            self.state.clone(),
        ];
        search_parts.extend(self.ipv6_cidr_blocks.iter().cloned());
        if let Some(z) = &self.availability_zone_id {
            search_parts.push(z.clone());
        }
        if let Some(o) = &self.owner_id {
            search_parts.push(o.clone());
        }

        // Add tags
        for (k, v) in &self.tags {
            search_parts.push(format!("{}:{}", k, v));
        }

        search_parts.join(" ")
    }

    fn details(&self) -> Vec<(String, String)> {
        // IPv6-native subnets have no IPv4 CIDR at all.
        let cidr = if self.ipv6_native && self.cidr_block == "unknown" {
            "— (IPv6-native)".to_string()
        } else {
            self.cidr_block.clone()
        };
        let mut details = vec![
            ("Subnet ID".to_string(), self.subnet_id.clone()),
            ("VPC ID".to_string(), self.vpc_id.clone()),
            ("CIDR Block".to_string(), cidr),
        ];
        for c in &self.ipv6_cidr_blocks {
            details.push(("IPv6 CIDR".to_string(), c.clone()));
        }
        details.push((
            "Availability Zone".to_string(),
            self.availability_zone.clone(),
        ));
        if let Some(z) = &self.availability_zone_id {
            details.push(("AZ ID".to_string(), z.clone()));
        }
        details.push((
            "Available IPs".to_string(),
            self.available_ip_count.to_string(),
        ));
        details.push(("State".to_string(), self.state.clone()));
        if let Some(owner) = &self.owner_id {
            details.push(("Owner".to_string(), owner.clone()));
        }
        details.push((
            "Auto-assign Public IP".to_string(),
            if self.map_public_ip { "Yes" } else { "No" }.to_string(),
        ));
        if self.assign_ipv6_on_creation {
            details.push(("Auto-assign IPv6".to_string(), "Yes".to_string()));
        }
        if self.ipv6_native {
            details.push(("IPv6 Native".to_string(), "Yes".to_string()));
        }
        if self.enable_dns64 {
            details.push(("DNS64".to_string(), "Enabled".to_string()));
        }
        if self.default_for_az {
            details.push(("Default for AZ".to_string(), "Yes".to_string()));
        }
        details
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{region}.console.aws.amazon.com/vpc/home?region={region}#SubnetDetails:subnetId={}",
            self.subnet_id
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// Route Table Resource
#[derive(Clone, Debug)]
pub struct RouteTable {
    pub route_table_id: String,
    pub vpc_id: String,
    pub is_main: bool,
    pub routes: Vec<RouteEntry>,
    pub associations: Vec<RouteAssociation>,
    /// VGWs propagating routes into this table (VPN/DX route propagation).
    pub propagating_vgws: Vec<String>,
    pub tags: HashMap<String, String>,
}

impl RouteTable {
    pub fn from_sdk(rt: &aws_sdk_ec2::types::RouteTable) -> Self {
        let route_table_id = rt.route_table_id().unwrap_or("unknown").to_string();
        let vpc_id = rt.vpc_id().unwrap_or("").to_string();

        let routes = rt
            .routes()
            .iter()
            .map(|r| {
                let destination = r
                    .destination_cidr_block()
                    .or(r.destination_ipv6_cidr_block())
                    .or(r.destination_prefix_list_id())
                    .unwrap_or("—")
                    .to_string();
                let target = r
                    .gateway_id()
                    .or(r.nat_gateway_id())
                    .or(r.transit_gateway_id())
                    .or(r.vpc_peering_connection_id())
                    .or(r.instance_id())
                    .or(r.network_interface_id())
                    .unwrap_or("—")
                    .to_string();
                let state = r
                    .state()
                    .map(|s| s.as_str().to_string())
                    .unwrap_or_else(|| "active".to_string());
                let origin = match r.origin().map(|o| o.as_str()).unwrap_or("") {
                    "CreateRoute" => "Static",
                    "CreateRouteTable" => "Built-in",
                    "EnableVgwRoutePropagation" => "Propagated",
                    other => other,
                }
                .to_string();
                RouteEntry { destination, target, state, origin }
            })
            .collect();

        let raw_assocs = rt.associations();
        let is_main = raw_assocs.iter().any(|a| a.main().unwrap_or(false));
        let associations = raw_assocs
            .iter()
            .map(|a| RouteAssociation {
                subnet_id: a.subnet_id().map(|s| s.to_string()),
                gateway_id: a.gateway_id().map(|s| s.to_string()),
                is_main: a.main().unwrap_or(false),
                state: a
                    .association_state()
                    .and_then(|s| s.state())
                    .map(|s| s.as_str().to_string())
                    .unwrap_or_else(|| "associated".to_string()),
            })
            .collect();

        let propagating_vgws: Vec<String> = rt
            .propagating_vgws()
            .iter()
            .filter_map(|v| v.gateway_id().map(|s| s.to_string()))
            .collect();

        let mut tags = HashMap::new();
        for tag in rt.tags() {
            if let (Some(k), Some(v)) = (tag.key(), tag.value()) {
                tags.insert(k.to_string(), v.to_string());
            }
        }

        Self {
            route_table_id,
            vpc_id,
            is_main,
            routes,
            associations,
            propagating_vgws,
            tags,
        }
    }
}

crate::sections! {
    pub enum RouteTableDetailSection,
    pub static ROUTE_TABLE_SECTIONS = [
        Routes "Routes",
        Associations "Associations",
        Tags "Tags",
    ]
}

impl Resource for RouteTable {
    fn references(&self) -> Vec<(String, String)> {
        let mut v = vec![("VPC".to_string(), self.vpc_id.clone())];
        let mut r = |l: &str, x: &str| v.push((l.to_string(), x.to_string()));
        for rt in &self.routes { r("Route Target", &rt.target); }
        for a in &self.associations {
            if let Some(x) = &a.subnet_id { r("Associated Subnet", x); }
            if let Some(x) = &a.gateway_id { r("Associated Gateway", x); }
        }
        for x in &self.propagating_vgws { r("Propagating VGW", x); }
        v
    }

    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&ROUTE_TABLE_SECTIONS)
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws ec2 describe-route-tables --route-table-ids {}",
            crate::aws::resource::shell_quote(self.id())
        ))
    }

    fn id(&self) -> &str {
        &self.route_table_id
    }

    fn name(&self) -> &str {
        self.tags
            .get("Name")
            .map(|s| s.as_str())
            .unwrap_or(&self.route_table_id)
    }

    fn resource_type(&self) -> &str {
        "RouteTable"
    }

    fn state(&self) -> ResourceState {
        if self.is_main {
            ResourceState::Available
        } else {
            ResourceState::Unknown("active".to_string())
        }
    }

    fn state_label(&self) -> String {
        if self.is_main { "main" } else { "active" }.to_string()
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        let mut parts = vec![
            self.route_table_id.clone(),
            self.name().to_string(),
            self.vpc_id.clone(),
        ];
        if self.is_main {
            parts.push("main".to_string());
        }
        // Edge-associated gateways + propagating VGWs — "which table serves
        // igw-…" / "where does vgw-… propagate" as plain fuzzy searches.
        for a in &self.associations {
            if let Some(g) = &a.gateway_id {
                parts.push(g.clone());
            }
        }
        parts.extend(self.propagating_vgws.iter().cloned());
        for (k, v) in &self.tags {
            parts.push(format!("{}:{}", k, v));
        }
        parts.join(" ")
    }

    fn details(&self) -> Vec<(String, String)> {
        let mut details = vec![
            ("Route Table ID".to_string(), self.route_table_id.clone()),
            ("VPC ID".to_string(), self.vpc_id.clone()),
            (
                "Main".to_string(),
                if self.is_main { "Yes" } else { "No" }.to_string(),
            ),
            ("Routes".to_string(), self.routes.len().to_string()),
            (
                "Subnet Associations".to_string(),
                self.associations
                    .iter()
                    .filter(|a| a.subnet_id.is_some())
                    .count()
                    .to_string(),
            ),
        ];
        for a in &self.associations {
            if let Some(g) = &a.gateway_id {
                details.push(("Edge Association".to_string(), g.clone()));
            }
        }
        for v in &self.propagating_vgws {
            details.push(("Propagating VGW".to_string(), v.clone()));
        }
        details
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{region}.console.aws.amazon.com/vpc/home?region={region}#RouteTableDetails:RouteTableId={}",
            self.route_table_id
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// Internet Gateway Resource
#[derive(Clone, Debug)]
pub struct InternetGateway {
    pub igw_id: String,
    pub state: String,
    pub vpc_id: Option<String>,
    pub tags: HashMap<String, String>,
}

impl InternetGateway {
    pub fn from_sdk(igw: &aws_sdk_ec2::types::InternetGateway) -> Self {
        let igw_id = igw.internet_gateway_id().unwrap_or("unknown").to_string();
        let attachments = igw.attachments();
        let (state, vpc_id) = if let Some(att) = attachments.first() {
            (
                att.state()
                    .map(|s| s.as_str().to_string())
                    .unwrap_or_else(|| "unknown".to_string()),
                att.vpc_id().map(|s| s.to_string()),
            )
        } else {
            ("detached".to_string(), None)
        };

        let mut tags = HashMap::new();
        for tag in igw.tags() {
            if let (Some(k), Some(v)) = (tag.key(), tag.value()) {
                tags.insert(k.to_string(), v.to_string());
            }
        }

        Self {
            igw_id,
            state,
            vpc_id,
            tags,
        }
    }
}

impl Resource for InternetGateway {
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws ec2 describe-internet-gateways --internet-gateway-ids {}",
            crate::aws::resource::shell_quote(self.id())
        ))
    }

    fn id(&self) -> &str {
        &self.igw_id
    }

    fn name(&self) -> &str {
        self.tags
            .get("Name")
            .map(|s| s.as_str())
            .unwrap_or(&self.igw_id)
    }

    fn resource_type(&self) -> &str {
        "InternetGateway"
    }

    fn state(&self) -> ResourceState {
        match self.state.as_str() {
            "available" | "attached" => ResourceState::Available,
            "detached" => ResourceState::Stopped,
            "attaching" | "detaching" => ResourceState::Pending,
            _ => ResourceState::Unknown(self.state.clone()),
        }
    }

    fn state_label(&self) -> String {
        native_state_label(&self.state, || self.state())
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        let mut parts = vec![
            self.igw_id.clone(),
            self.name().to_string(),
            self.state.clone(),
        ];
        if let Some(vpc) = &self.vpc_id {
            parts.push(vpc.clone());
        }
        for (k, v) in &self.tags {
            parts.push(format!("{}:{}", k, v));
        }
        parts.join(" ")
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("IGW ID".to_string(), self.igw_id.clone()),
            ("State".to_string(), self.state.clone()),
            (
                "Attached VPC".to_string(),
                self.vpc_id.clone().unwrap_or_else(|| "—".to_string()),
            ),
        ]
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{region}.console.aws.amazon.com/vpc/home?region={region}#InternetGateway:internetGatewayId={}",
            self.igw_id
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// NAT Gateway Resource
#[derive(Clone, Debug)]
pub struct NatGateway {
    pub nat_gateway_id: String,
    pub vpc_id: String,
    pub subnet_id: String,
    pub state: String,
    pub connectivity_type: String,
    /// First address — kept as the list-row / summary IP.
    pub public_ip: Option<String>,
    pub private_ip: Option<String>,
    /// Every (public, private) address pair — scaled NAT GWs carry several EIPs.
    pub addresses: Vec<(Option<String>, Option<String>)>,
    pub failure_code: Option<String>,
    pub failure_message: Option<String>,
    pub create_time: Option<String>,
    pub tags: HashMap<String, String>,
}

impl NatGateway {
    pub fn from_sdk(nat: &aws_sdk_ec2::types::NatGateway) -> Self {
        let nat_gateway_id = nat.nat_gateway_id().unwrap_or("unknown").to_string();
        let vpc_id = nat.vpc_id().unwrap_or("").to_string();
        let subnet_id = nat.subnet_id().unwrap_or("").to_string();
        let state = nat
            .state()
            .map(|s| s.as_str().to_string())
            .unwrap_or_else(|| "unknown".to_string());
        let connectivity_type = nat
            .connectivity_type()
            .map(|c| c.as_str().to_string())
            .unwrap_or_else(|| "public".to_string());

        let addresses: Vec<(Option<String>, Option<String>)> = nat
            .nat_gateway_addresses()
            .iter()
            .map(|a| {
                (
                    a.public_ip().map(|s| s.to_string()),
                    a.private_ip().map(|s| s.to_string()),
                )
            })
            .collect();
        let (public_ip, private_ip) = addresses.first().cloned().unwrap_or((None, None));

        let mut tags = HashMap::new();
        for tag in nat.tags() {
            if let (Some(k), Some(v)) = (tag.key(), tag.value()) {
                tags.insert(k.to_string(), v.to_string());
            }
        }

        Self {
            nat_gateway_id,
            vpc_id,
            subnet_id,
            state,
            connectivity_type,
            public_ip,
            private_ip,
            addresses,
            failure_code: nat.failure_code().map(|s| s.to_string()),
            failure_message: nat.failure_message().map(|s| s.to_string()),
            create_time: nat.create_time().map(fmt_ts),
            tags,
        }
    }

    /// Public/Private IP rows for every address — numbered only when a scaled
    /// NAT GW carries more than one EIP.
    pub fn address_rows(&self) -> Vec<(String, String)> {
        let mut rows = Vec::new();
        let numbered = self.addresses.len() > 1;
        for (i, (public, private)) in self.addresses.iter().enumerate() {
            let suffix = if numbered {
                format!(" {}", i + 1)
            } else {
                String::new()
            };
            if let Some(ip) = public {
                rows.push((format!("Public IP (EIP){}", suffix), ip.clone()));
            }
            if let Some(ip) = private {
                rows.push((format!("Private IP{}", suffix), ip.clone()));
            }
        }
        rows
    }
}

crate::sections! {
    pub enum NatGatewayDetailSection,
    pub static NAT_GATEWAY_SECTIONS = [
        Overview "Overview",
        Network "Network",
        Tags "Tags",
    ]
}

impl Resource for NatGateway {
    fn references(&self) -> Vec<(String, String)> {
        vec![
            ("VPC".to_string(), self.vpc_id.clone()),
            ("Subnet".to_string(), self.subnet_id.clone()),
        ]
    }

    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&NAT_GATEWAY_SECTIONS)
    }

    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws ec2 describe-nat-gateways --nat-gateway-ids {}",
            crate::aws::resource::shell_quote(self.id())
        ))
    }

    fn id(&self) -> &str {
        &self.nat_gateway_id
    }

    fn name(&self) -> &str {
        self.tags
            .get("Name")
            .map(|s| s.as_str())
            .unwrap_or(&self.nat_gateway_id)
    }

    fn resource_type(&self) -> &str {
        "NatGateway"
    }

    fn state(&self) -> ResourceState {
        match self.state.as_str() {
            "available" => ResourceState::Available,
            "pending" => ResourceState::Pending,
            // Failed is an error to investigate, not a lifecycle end-state.
            "failed" => ResourceState::Unavailable,
            "deleting" | "deleted" => ResourceState::Terminated,
            _ => ResourceState::Unknown(self.state.clone()),
        }
    }

    fn state_label(&self) -> String {
        native_state_label(&self.state, || self.state())
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        let mut parts = vec![
            self.nat_gateway_id.clone(),
            self.name().to_string(),
            self.vpc_id.clone(),
            self.subnet_id.clone(),
            self.state.clone(),
            self.connectivity_type.clone(),
        ];
        for (public, private) in &self.addresses {
            if let Some(ip) = public {
                parts.push(ip.clone());
            }
            if let Some(ip) = private {
                parts.push(ip.clone());
            }
        }
        for (k, v) in &self.tags {
            parts.push(format!("{}:{}", k, v));
        }
        parts.join(" ")
    }

    fn details(&self) -> Vec<(String, String)> {
        let mut details = vec![
            ("NAT Gateway ID".to_string(), self.nat_gateway_id.clone()),
            ("VPC ID".to_string(), self.vpc_id.clone()),
            ("Subnet ID".to_string(), self.subnet_id.clone()),
            ("State".to_string(), self.state.clone()),
            (
                "Connectivity Type".to_string(),
                self.connectivity_type.clone(),
            ),
        ];
        if let Some(t) = &self.create_time {
            details.push(("Created".to_string(), t.clone()));
        }
        details.extend(self.address_rows());
        if let Some(code) = &self.failure_code {
            details.push((
                "Failure".to_string(),
                format!(
                    "⚠ {}: {}",
                    code,
                    self.failure_message.as_deref().unwrap_or("no message")
                ),
            ));
        }
        details
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{region}.console.aws.amazon.com/vpc/home?region={region}#NatGatewayDetails:natGatewayId={}",
            self.nat_gateway_id
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// VPC Security Group Resource
#[derive(Clone, Debug)]
pub struct VpcSecurityGroup {
    pub group_id: String,
    pub group_name: String,
    pub vpc_id: Option<String>,
    pub description: String,
    pub inbound_rules: Vec<crate::aws::services::ec2::SgRule>,
    pub outbound_rules: Vec<crate::aws::services::ec2::SgRule>,
    pub tags: HashMap<String, String>,
}

impl VpcSecurityGroup {
    pub fn from_sdk(sg: &aws_sdk_ec2::types::SecurityGroup) -> Self {
        let group_id = sg.group_id().unwrap_or("unknown").to_string();
        let group_name = sg.group_name().unwrap_or("").to_string();
        let vpc_id = sg.vpc_id().map(|s| s.to_string());
        let description = sg.description().unwrap_or("").to_string();
        let inbound_rules = crate::aws::services::ec2::parse_sg_rules(sg.ip_permissions());
        let outbound_rules =
            crate::aws::services::ec2::parse_sg_rules(sg.ip_permissions_egress());

        let mut tags = HashMap::new();
        for tag in sg.tags() {
            if let (Some(k), Some(v)) = (tag.key(), tag.value()) {
                tags.insert(k.to_string(), v.to_string());
            }
        }

        Self {
            group_id,
            group_name,
            vpc_id,
            description,
            inbound_rules,
            outbound_rules,
            tags,
        }
    }
}

impl Resource for VpcSecurityGroup {
    fn security_group_ids(&self) -> Vec<String> {
        vec![self.group_id.clone()]
    }

    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&crate::aws::services::ec2::SECURITY_GROUP_SECTIONS)
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws ec2 describe-security-groups --group-ids {}",
            crate::aws::resource::shell_quote(self.id())
        ))
    }

    fn id(&self) -> &str {
        &self.group_id
    }

    fn name(&self) -> &str {
        let tag_name = self.tags.get("Name").map(|s| s.as_str());
        tag_name.unwrap_or(&self.group_name)
    }

    fn resource_type(&self) -> &str {
        "SecurityGroup"
    }

    fn state(&self) -> ResourceState {
        ResourceState::stateless()
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        let mut parts = vec![
            self.group_id.clone(),
            self.group_name.clone(),
            self.description.clone(),
        ];
        if let Some(vpc) = &self.vpc_id {
            parts.push(vpc.clone());
        }
        for (k, v) in &self.tags {
            parts.push(format!("{}:{}", k, v));
        }
        parts.join(" ")
    }

    fn details(&self) -> Vec<(String, String)> {
        let mut details = vec![
            ("Group ID".to_string(), self.group_id.clone()),
            ("Name".to_string(), self.group_name.clone()),
            (
                "VPC ID".to_string(),
                self.vpc_id.clone().unwrap_or_else(|| "—".to_string()),
            ),
            ("Description".to_string(), self.description.clone()),
        ];
        details.push(("".to_string(), "".to_string()));
        details.extend(crate::aws::services::ec2::sg_rule_rows(
            "Inbound",
            &self.inbound_rules,
        ));
        details.push(("".to_string(), "".to_string()));
        details.extend(crate::aws::services::ec2::sg_rule_rows(
            "Outbound",
            &self.outbound_rules,
        ));
        if !self.tags.is_empty() {
            details.push(("".to_string(), "".to_string()));
            details.push(("Tags".to_string(), "".to_string()));
            let mut sorted: Vec<(&String, &String)> = self.tags.iter().collect();
            sorted.sort_by_key(|(k, _)| k.as_str());
            for (k, v) in sorted {
                details.push((format!("  {}", k), v.clone()));
            }
        }
        details
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{region}.console.aws.amazon.com/ec2/home?region={region}#SecurityGroup:groupId={}",
            self.group_id
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// Network ACL Resource
#[derive(Clone, Debug)]
pub struct NetworkAcl {
    pub acl_id: String,
    pub vpc_id: String,
    pub is_default: bool,
    pub inbound_entries: Vec<AclEntry>,
    pub outbound_entries: Vec<AclEntry>,
    pub associations: Vec<AclAssociation>,
    pub tags: HashMap<String, String>,
}

impl NetworkAcl {
    pub fn from_sdk(acl: &aws_sdk_ec2::types::NetworkAcl) -> Self {
        let acl_id = acl.network_acl_id().unwrap_or("unknown").to_string();
        let vpc_id = acl.vpc_id().unwrap_or("").to_string();
        let is_default = acl.is_default().unwrap_or(false);

        let mut inbound_entries: Vec<AclEntry> = Vec::new();
        let mut outbound_entries: Vec<AclEntry> = Vec::new();

        for entry in acl.entries() {
            let protocol_raw = entry.protocol().unwrap_or("-1");
            let protocol = match protocol_raw {
                "-1" => "All".to_string(),
                "6" => "TCP".to_string(),
                "17" => "UDP".to_string(),
                "1" => "ICMP".to_string(),
                other => other.to_string(),
            };
            let action = entry
                .rule_action()
                .map(|a| a.as_str().to_string())
                .unwrap_or_else(|| "allow".to_string());
            let cidr = entry
                .cidr_block()
                .or(entry.ipv6_cidr_block())
                .unwrap_or("—")
                .to_string();
            let port_range = if protocol_raw == "-1" {
                "All".to_string()
            } else if let Some(pr) = entry.port_range() {
                match (pr.from(), pr.to()) {
                    (Some(f), Some(t)) if f == t => f.to_string(),
                    (Some(f), Some(t)) => format!("{}-{}", f, t),
                    _ => "All".to_string(),
                }
            } else {
                "All".to_string()
            };
            let egress = entry.egress().unwrap_or(false);
            let acl_entry = AclEntry {
                rule_number: entry.rule_number().unwrap_or(0),
                protocol,
                action,
                cidr,
                port_range,
                egress,
            };
            if egress {
                outbound_entries.push(acl_entry);
            } else {
                inbound_entries.push(acl_entry);
            }
        }
        inbound_entries.sort_by_key(|e| e.rule_number);
        outbound_entries.sort_by_key(|e| e.rule_number);

        let associations = acl
            .associations()
            .iter()
            .map(|a| AclAssociation {
                subnet_id: a.subnet_id().unwrap_or("unknown").to_string(),
                association_id: a
                    .network_acl_association_id()
                    .unwrap_or("")
                    .to_string(),
            })
            .collect();

        let mut tags = HashMap::new();
        for tag in acl.tags() {
            if let (Some(k), Some(v)) = (tag.key(), tag.value()) {
                tags.insert(k.to_string(), v.to_string());
            }
        }

        Self {
            acl_id,
            vpc_id,
            is_default,
            inbound_entries,
            outbound_entries,
            associations,
            tags,
        }
    }
}

crate::sections! {
    pub enum NetworkAclDetailSection,
    pub static NETWORK_ACL_SECTIONS = [
        Inbound "Inbound",
        Outbound "Outbound",
        Associations "Associations",
        Tags "Tags",
    ]
}

impl Resource for NetworkAcl {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&NETWORK_ACL_SECTIONS)
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws ec2 describe-network-acls --network-acl-ids {}",
            crate::aws::resource::shell_quote(self.id())
        ))
    }

    fn id(&self) -> &str {
        &self.acl_id
    }

    fn name(&self) -> &str {
        self.tags
            .get("Name")
            .map(|s| s.as_str())
            .unwrap_or(&self.acl_id)
    }

    fn resource_type(&self) -> &str {
        "NetworkAcl"
    }

    fn state(&self) -> ResourceState {
        ResourceState::stateless()
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        let mut parts = vec![
            self.acl_id.clone(),
            self.name().to_string(),
            self.vpc_id.clone(),
        ];
        if self.is_default {
            parts.push("default".to_string());
        }
        for (k, v) in &self.tags {
            parts.push(format!("{}:{}", k, v));
        }
        parts.join(" ")
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("ACL ID".to_string(), self.acl_id.clone()),
            ("VPC ID".to_string(), self.vpc_id.clone()),
            (
                "Default".to_string(),
                if self.is_default { "Yes" } else { "No" }.to_string(),
            ),
            (
                "Associated Subnets".to_string(),
                self.associations.len().to_string(),
            ),
            (
                "Inbound Rules".to_string(),
                self.inbound_entries.len().to_string(),
            ),
            (
                "Outbound Rules".to_string(),
                self.outbound_entries.len().to_string(),
            ),
        ]
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{region}.console.aws.amazon.com/vpc/home?region={region}#NetworkAclDetails:networkAclId={}",
            self.acl_id
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── shared helpers for the VPC extensions (endpoints + VPN) ───────────────────

fn ec2_tags(tags: &[aws_sdk_ec2::types::Tag]) -> HashMap<String, String> {
    tags.iter()
        .filter_map(|t| match (t.key(), t.value()) {
            (Some(k), Some(v)) => Some((k.to_string(), v.to_string())),
            _ => None,
        })
        .collect()
}

fn fmt_ts(dt: &aws_sdk_ec2::primitives::DateTime) -> String {
    let secs = dt.secs();
    let days = secs / 86400;
    let rem = secs % 86400;
    let (h, mi) = (rem / 3600, (rem % 3600) / 60);
    // Reuse the calendar math via a minimal inline conversion.
    let mut y = 1970i32;
    let mut d = days;
    loop {
        let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
        let dy = if leap { 366 } else { 365 };
        if d < dy {
            break;
        }
        d -= dy;
        y += 1;
    }
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let months = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut mo = 0usize;
    for (i, &len) in months.iter().enumerate() {
        if d < len {
            mo = i;
            break;
        }
        d -= len;
    }
    format!("{:04}-{:02}-{:02} {:02}:{:02} UTC", y, mo + 1, d + 1, h, mi)
}

fn name_or<'a>(tags: &'a HashMap<String, String>, fallback: &'a str) -> &'a str {
    tags.get("Name")
        .filter(|n| !n.is_empty())
        .map(|s| s.as_str())
        .unwrap_or(fallback)
}

fn append_tags(mut s: String, tags: &HashMap<String, String>) -> String {
    for (k, v) in tags {
        s.push(' ');
        s.push_str(k);
        s.push(' ');
        s.push_str(v);
    }
    s
}

// ── VpcEndpoint (PrivateLink) ─────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct VpcEndpoint {
    pub id: String,
    pub vpc_id: String,
    pub service_name: String,
    pub kind: String, // Interface / Gateway / GatewayLoadBalancer
    pub state: String,
    pub private_dns_enabled: bool,
    pub subnet_ids: Vec<String>,
    pub security_group_ids: Vec<String>,
    pub route_table_ids: Vec<String>,
    pub network_interface_ids: Vec<String>,
    pub dns_entries: Vec<String>,
    pub policy_document: Option<String>,
    pub creation: Option<String>,
    pub tags: HashMap<String, String>,
}

impl VpcEndpoint {
    pub fn from_sdk(e: &aws_sdk_ec2::types::VpcEndpoint) -> Self {
        let tags = ec2_tags(e.tags());
        Self {
            id: e.vpc_endpoint_id().unwrap_or_default().to_string(),
            vpc_id: e.vpc_id().unwrap_or_default().to_string(),
            service_name: e.service_name().unwrap_or_default().to_string(),
            kind: e
                .vpc_endpoint_type()
                .map(|t| t.as_str().to_string())
                .unwrap_or_default(),
            state: e.state().map(|s| s.as_str().to_string()).unwrap_or_default(),
            private_dns_enabled: e.private_dns_enabled().unwrap_or(false),
            subnet_ids: e.subnet_ids().to_vec(),
            security_group_ids: e
                .groups()
                .iter()
                .filter_map(|g| g.group_id().map(|s| s.to_string()))
                .collect(),
            route_table_ids: e.route_table_ids().to_vec(),
            network_interface_ids: e.network_interface_ids().to_vec(),
            dns_entries: e
                .dns_entries()
                .iter()
                .filter_map(|d| d.dns_name().map(|s| s.to_string()))
                .collect(),
            policy_document: e
                .policy_document()
                .filter(|p| !p.is_empty())
                .map(|s| s.to_string()),
            creation: e.creation_timestamp().map(fmt_ts),
            tags,
        }
    }

    /// Short service label (last `.`-segment, e.g. `s3`, `ecr.api`).
    fn service_short(&self) -> &str {
        self.service_name.rsplit('.').next().unwrap_or(&self.service_name)
    }
}

crate::sections! {
    pub enum VpcEndpointDetailSection,
    pub static VPC_ENDPOINT_SECTIONS = [
        Details "Details",
        Network "Network",
        Policy "Policy",
        Tags "Tags",
    ]
}

impl Resource for VpcEndpoint {
    fn references(&self) -> Vec<(String, String)> {
        let mut v = sg_refs(self.security_group_ids());
        let mut r = |l: &str, x: &str| v.push((l.to_string(), x.to_string()));
        r("VPC", &self.vpc_id);
        for x in &self.subnet_ids { r("Subnet", x); }
        for x in &self.route_table_ids { r("Route Table", x); }
        for x in &self.network_interface_ids { r("Network Interface", x); }
        v
    }

    fn security_group_ids(&self) -> Vec<String> {
        self.security_group_ids.clone()
    }

    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&VPC_ENDPOINT_SECTIONS)
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws ec2 describe-vpc-endpoints --vpc-endpoint-ids {}",
            crate::aws::resource::shell_quote(self.id())
        ))
    }

    fn id(&self) -> &str {
        &self.id
    }
    fn name(&self) -> &str {
        name_or(&self.tags, self.service_short())
    }
    fn resource_type(&self) -> &str {
        "VPC Endpoint"
    }
    fn state(&self) -> ResourceState {
        match self.state.as_str() {
            "available" => ResourceState::Available,
            "pending" | "pendingAcceptance" => ResourceState::Pending,
            "deleting" => ResourceState::Deleting,
            "rejected" | "failed" | "expired" => ResourceState::Unavailable,
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
            format!("{} {} {} {}", self.id, self.vpc_id, self.service_name, self.kind),
            &self.tags,
        )
    }
    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("ID".to_string(), self.id.clone()),
            ("Service".to_string(), self.service_name.clone()),
            ("Type".to_string(), self.kind.clone()),
            ("State".to_string(), self.state.clone()),
            ("VPC".to_string(), self.vpc_id.clone()),
            ("Private DNS".to_string(), self.private_dns_enabled.to_string()),
        ]
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{region}.console.aws.amazon.com/vpc/home?region={region}#Endpoints:vpcEndpointId={}",
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

// ── VpnConnection (Site-to-Site VPN) ──────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct VpnTunnel {
    pub outside_ip: String,
    pub status: String, // UP / DOWN
    pub last_status_change: Option<String>,
    pub status_message: String,
    pub accepted_route_count: i32,
}

#[derive(Clone, Debug)]
pub struct VpnConnection {
    pub id: String,
    pub state: String,
    pub customer_gateway_id: String,
    pub customer_gateway_ip: Option<String>, // resolved eagerly
    pub customer_gateway_asn: Option<String>,
    pub vpn_gateway_id: Option<String>,
    pub transit_gateway_id: Option<String>,
    pub category: String,
    pub routing: String, // static / dynamic (BGP)
    pub tunnels: Vec<VpnTunnel>,
    pub routes: Vec<String>,
    pub tags: HashMap<String, String>,
}

impl VpnConnection {
    pub fn from_sdk(
        v: &aws_sdk_ec2::types::VpnConnection,
        cgw_map: &HashMap<String, (Option<String>, Option<String>)>,
    ) -> Self {
        let tags = ec2_tags(v.tags());
        let cgw_id = v.customer_gateway_id().unwrap_or_default().to_string();
        let (cgw_ip, cgw_asn) = cgw_map.get(&cgw_id).cloned().unwrap_or((None, None));
        let routing = match v.options().and_then(|o| o.static_routes_only()) {
            Some(true) => "static".to_string(),
            _ => "dynamic (BGP)".to_string(),
        };
        let tunnels = v
            .vgw_telemetry()
            .iter()
            .map(|t| VpnTunnel {
                outside_ip: t.outside_ip_address().unwrap_or_default().to_string(),
                status: t
                    .status()
                    .map(|s| s.as_str().to_uppercase())
                    .unwrap_or_default(),
                last_status_change: t.last_status_change().map(fmt_ts),
                status_message: t.status_message().unwrap_or_default().to_string(),
                accepted_route_count: t.accepted_route_count().unwrap_or(0),
            })
            .collect();
        let routes = v
            .routes()
            .iter()
            .filter_map(|r| r.destination_cidr_block().map(|s| s.to_string()))
            .collect();
        Self {
            id: v.vpn_connection_id().unwrap_or_default().to_string(),
            state: v.state().map(|s| s.as_str().to_string()).unwrap_or_default(),
            customer_gateway_id: cgw_id,
            customer_gateway_ip: cgw_ip,
            customer_gateway_asn: cgw_asn,
            vpn_gateway_id: v.vpn_gateway_id().map(|s| s.to_string()),
            transit_gateway_id: v.transit_gateway_id().map(|s| s.to_string()),
            category: v.category().unwrap_or_default().to_string(),
            routing,
            tunnels,
            routes,
            tags,
        }
    }

    pub fn tunnels_up(&self) -> usize {
        self.tunnels.iter().filter(|t| t.status == "UP").count()
    }
}

crate::sections! {
    pub enum VpnConnectionDetailSection,
    pub static VPN_CONNECTION_SECTIONS = [
        Tunnels "Tunnels",
        Routes "Routes",
        Details "Details",
        Tags "Tags",
    ]
}

impl Resource for VpnConnection {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&VPN_CONNECTION_SECTIONS)
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws ec2 describe-vpn-connections --vpn-connection-ids {}",
            crate::aws::resource::shell_quote(self.id())
        ))
    }

    fn id(&self) -> &str {
        &self.id
    }
    fn name(&self) -> &str {
        name_or(&self.tags, &self.id)
    }
    fn resource_type(&self) -> &str {
        "VPN Connection"
    }
    fn state(&self) -> ResourceState {
        match self.state.as_str() {
            // The point of the view: "available" with a tunnel down is degraded.
            "available" if self.tunnels_up() == self.tunnels.len() && !self.tunnels.is_empty() => {
                ResourceState::Available
            }
            "available" => ResourceState::Unavailable,
            "pending" => ResourceState::Pending,
            "deleting" => ResourceState::Deleting,
            "deleted" => ResourceState::Terminated,
            other => ResourceState::Unknown(other.to_string()),
        }
    }

    fn state_label(&self) -> String {
        // "available" with a tunnel down is the case the colour flags — name it.
        let all_up = !self.tunnels.is_empty() && self.tunnels_up() == self.tunnels.len();
        if self.state == "available" && !all_up {
            "degraded".to_string()
        } else {
            native_state_label(&self.state, || self.state())
        }
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        append_tags(
            format!(
                "{} {} {} {}",
                self.id,
                self.customer_gateway_id,
                self.vpn_gateway_id.as_deref().unwrap_or(""),
                self.transit_gateway_id.as_deref().unwrap_or("")
            ),
            &self.tags,
        )
    }
    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("ID".to_string(), self.id.clone()),
            ("State".to_string(), self.state.clone()),
            (
                "Tunnels Up".to_string(),
                format!("{}/{}", self.tunnels_up(), self.tunnels.len()),
            ),
            ("Customer Gateway".to_string(), self.customer_gateway_id.clone()),
            ("Routing".to_string(), self.routing.clone()),
        ]
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{region}.console.aws.amazon.com/vpc/home?region={region}#VpnConnectionDetails:VpnConnectionId={}",
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

// ── DHCP Options Set ──────────────────────────────────────────────────────────

/// A DHCP options set (`dopt-…`). No lifecycle state — the interesting payload
/// is the parsed option key/values (domain name, DNS/NTP servers, NetBIOS).
#[derive(Clone, Debug)]
pub struct DhcpOptionsSet {
    pub id: String,
    pub owner_id: Option<String>,
    /// Parsed `dhcp_configurations`, sorted by key: (key, values).
    pub configs: Vec<(String, Vec<String>)>,
    pub tags: HashMap<String, String>,
}

impl DhcpOptionsSet {
    pub fn from_sdk(d: &aws_sdk_ec2::types::DhcpOptions) -> Self {
        let mut configs: Vec<(String, Vec<String>)> = d
            .dhcp_configurations()
            .iter()
            .filter_map(|c| {
                c.key().map(|k| {
                    (
                        k.to_string(),
                        c.values()
                            .iter()
                            .filter_map(|v| v.value().map(|s| s.to_string()))
                            .collect(),
                    )
                })
            })
            .collect();
        configs.sort_by(|a, b| a.0.cmp(&b.0));

        let mut tags = HashMap::new();
        for tag in d.tags() {
            if let (Some(key), Some(value)) = (tag.key(), tag.value()) {
                tags.insert(key.to_string(), value.to_string());
            }
        }

        Self {
            id: d.dhcp_options_id().unwrap_or("unknown").to_string(),
            owner_id: d.owner_id().map(|s| s.to_string()),
            configs,
            tags,
        }
    }

    /// Joined values for one option key (e.g. "domain-name-servers").
    pub fn values_for(&self, key: &str) -> Option<String> {
        self.configs
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.join(", "))
    }
}

/// Human label for a DHCP option key ("domain-name-servers" → "Domain Name Servers").
pub fn pretty_dhcp_key(key: &str) -> String {
    match key {
        "domain-name" => "Domain Name".to_string(),
        "domain-name-servers" => "Domain Name Servers".to_string(),
        "ntp-servers" => "NTP Servers".to_string(),
        "netbios-name-servers" => "NetBIOS Name Servers".to_string(),
        "netbios-node-type" => "NetBIOS Node Type".to_string(),
        "ipv6-address-preferred-lease-time" => "IPv6 Preferred Lease Time".to_string(),
        other => {
            // Title-case the dash-separated fallback.
            other
                .split('-')
                .map(|w| {
                    let mut c = w.chars();
                    match c.next() {
                        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                        None => String::new(),
                    }
                })
                .collect::<Vec<_>>()
                .join(" ")
        }
    }
}

crate::sections! {
    pub enum DhcpOptionsDetailSection,
    pub static DHCP_OPTIONS_SECTIONS = [
        Overview "Overview",
        Vpcs "VPCs",
        Tags "Tags",
    ]
}

impl Resource for DhcpOptionsSet {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&DHCP_OPTIONS_SECTIONS)
    }

    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws ec2 describe-dhcp-options --dhcp-options-ids {}",
            crate::aws::resource::shell_quote(self.id())
        ))
    }

    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        self.tags
            .get("Name")
            .map(|s| s.as_str())
            .unwrap_or(&self.id)
    }

    fn resource_type(&self) -> &str {
        "DHCP Options"
    }

    fn state(&self) -> ResourceState {
        // No lifecycle state on DHCP options sets — neutral gray, like
        // non-main route tables.
        ResourceState::Unknown("active".to_string())
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        let mut parts = vec![self.id.clone(), self.name().to_string()];
        for (k, values) in &self.configs {
            parts.push(k.clone());
            parts.extend(values.iter().cloned());
        }
        for (k, v) in &self.tags {
            parts.push(format!("{}:{}", k, v));
        }
        parts.join(" ")
    }

    fn details(&self) -> Vec<(String, String)> {
        let mut details = vec![("ID".to_string(), self.id.clone())];
        if let Some(owner) = &self.owner_id {
            details.push(("Owner".to_string(), owner.clone()));
        }
        for (k, values) in &self.configs {
            details.push((pretty_dhcp_key(k), values.join(", ")));
        }
        details
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{region}.console.aws.amazon.com/vpc/home?region={region}#DhcpOptionsDetails:DhcpOptionsId={}",
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

// ── VPC Peering Connection ────────────────────────────────────────────────────

/// One side (requester or accepter) of a peering connection.
#[derive(Clone, Debug, Default)]
pub struct PeeringSide {
    pub vpc_id: Option<String>,
    pub cidrs: Vec<String>,
    pub owner_id: Option<String>,
    pub region: Option<String>,
    pub allow_remote_dns: Option<bool>,
}

impl PeeringSide {
    fn from_sdk(info: Option<&aws_sdk_ec2::types::VpcPeeringConnectionVpcInfo>) -> Self {
        let Some(info) = info else {
            return Self::default();
        };
        let mut cidrs: Vec<String> = Vec::new();
        if let Some(c) = info.cidr_block() {
            cidrs.push(c.to_string());
        }
        for c in info.cidr_block_set() {
            if let Some(c) = c.cidr_block() {
                if !cidrs.iter().any(|have| have == c) {
                    cidrs.push(c.to_string());
                }
            }
        }
        for c in info.ipv6_cidr_block_set() {
            if let Some(c) = c.ipv6_cidr_block() {
                cidrs.push(c.to_string());
            }
        }
        Self {
            vpc_id: info.vpc_id().map(|s| s.to_string()),
            cidrs,
            owner_id: info.owner_id().map(|s| s.to_string()),
            region: info.region().map(|s| s.to_string()),
            allow_remote_dns: info
                .peering_options()
                .and_then(|o| o.allow_dns_resolution_from_remote_vpc()),
        }
    }
}

#[derive(Clone, Debug)]
pub struct VpcPeering {
    pub id: String,
    pub status: String,
    pub status_message: Option<String>,
    pub expiration_time: Option<String>,
    pub requester: PeeringSide,
    pub accepter: PeeringSide,
    pub tags: HashMap<String, String>,
}

impl VpcPeering {
    pub fn from_sdk(p: &aws_sdk_ec2::types::VpcPeeringConnection) -> Self {
        let mut tags = HashMap::new();
        for tag in p.tags() {
            if let (Some(key), Some(value)) = (tag.key(), tag.value()) {
                tags.insert(key.to_string(), value.to_string());
            }
        }
        Self {
            id: p.vpc_peering_connection_id().unwrap_or("unknown").to_string(),
            status: p
                .status()
                .and_then(|s| s.code())
                .map(|c| c.as_str().to_string())
                .unwrap_or_else(|| "unknown".to_string()),
            status_message: p.status().and_then(|s| s.message()).map(|s| s.to_string()),
            expiration_time: p.expiration_time().map(fmt_ts),
            requester: PeeringSide::from_sdk(p.requester_vpc_info()),
            accepter: PeeringSide::from_sdk(p.accepter_vpc_info()),
            tags,
        }
    }
}

crate::sections! {
    pub enum VpcPeeringDetailSection,
    pub static VPC_PEERING_SECTIONS = [
        Overview "Overview",
        Tags "Tags",
    ]
}

impl Resource for VpcPeering {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&VPC_PEERING_SECTIONS)
    }

    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws ec2 describe-vpc-peering-connections --vpc-peering-connection-ids {}",
            crate::aws::resource::shell_quote(self.id())
        ))
    }

    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        self.tags
            .get("Name")
            .map(|s| s.as_str())
            .unwrap_or(&self.id)
    }

    fn resource_type(&self) -> &str {
        "Peering Connection"
    }

    fn state(&self) -> ResourceState {
        match self.status.as_str() {
            "active" => ResourceState::Available,
            "pending-acceptance" | "provisioning" | "initiating-request" => {
                ResourceState::Pending
            }
            "failed" | "rejected" | "expired" | "deleted" | "deleting" => {
                ResourceState::Unavailable
            }
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
        let mut parts = vec![self.id.clone(), self.name().to_string(), self.status.clone()];
        for side in [&self.requester, &self.accepter] {
            if let Some(v) = &side.vpc_id {
                parts.push(v.clone());
            }
            parts.extend(side.cidrs.iter().cloned());
            if let Some(o) = &side.owner_id {
                parts.push(o.clone());
            }
        }
        for (k, v) in &self.tags {
            parts.push(format!("{}:{}", k, v));
        }
        parts.join(" ")
    }

    fn details(&self) -> Vec<(String, String)> {
        let side = |s: &PeeringSide| {
            format!(
                "{} ({})",
                s.vpc_id.as_deref().unwrap_or("—"),
                s.cidrs.join(", ")
            )
        };
        vec![
            ("ID".to_string(), self.id.clone()),
            ("Status".to_string(), self.status.clone()),
            ("Requester".to_string(), side(&self.requester)),
            ("Accepter".to_string(), side(&self.accepter)),
        ]
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{region}.console.aws.amazon.com/vpc/home?region={region}#PeeringConnectionDetails:VpcPeeringConnectionId={}",
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

// ── Managed Prefix List ───────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct PrefixList {
    pub id: String,
    pub arn: Option<String>,
    pub name: String,
    pub state: String,
    pub state_message: Option<String>,
    pub address_family: Option<String>,
    pub max_entries: Option<i32>,
    pub version: Option<i64>,
    pub owner_id: Option<String>,
    pub tags: HashMap<String, String>,
}

impl PrefixList {
    pub fn from_sdk(p: &aws_sdk_ec2::types::ManagedPrefixList) -> Self {
        let mut tags = HashMap::new();
        for tag in p.tags() {
            if let (Some(key), Some(value)) = (tag.key(), tag.value()) {
                tags.insert(key.to_string(), value.to_string());
            }
        }
        Self {
            id: p.prefix_list_id().unwrap_or("unknown").to_string(),
            arn: p.prefix_list_arn().map(|s| s.to_string()),
            name: p.prefix_list_name().unwrap_or("unnamed").to_string(),
            state: p
                .state()
                .map(|s| s.as_str().to_string())
                .unwrap_or_else(|| "unknown".to_string()),
            state_message: p.state_message().map(|s| s.to_string()),
            address_family: p.address_family().map(|s| s.to_string()),
            max_entries: p.max_entries(),
            version: p.version(),
            owner_id: p.owner_id().map(|s| s.to_string()),
            tags,
        }
    }

    /// AWS-managed lists (owner "AWS", e.g. com.amazonaws.<region>.s3).
    pub fn is_aws_managed(&self) -> bool {
        self.owner_id.as_deref() == Some("AWS")
    }
}

crate::sections! {
    pub enum PrefixListDetailSection,
    pub static PREFIX_LIST_SECTIONS = [
        Overview "Overview",
        Entries "Entries" => crate::app::App::trigger_pl_entries_load,
        Tags "Tags",
    ]
}

impl Resource for PrefixList {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&PREFIX_LIST_SECTIONS)
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws ec2 get-managed-prefix-list-entries --prefix-list-id {}",
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
        "Prefix List"
    }

    fn state(&self) -> ResourceState {
        match self.state.as_str() {
            "create-complete" | "modify-complete" | "restore-complete" => {
                ResourceState::Available
            }
            s if s.ends_with("-in-progress") => ResourceState::Pending,
            s if s.ends_with("-failed") || s == "delete-complete" => {
                ResourceState::Unavailable
            }
            other => ResourceState::Unknown(other.to_string()),
        }
    }

    fn state_label(&self) -> String {
        native_state_label(&self.state, || self.state())
    }

    fn is_noise(&self) -> bool {
        // AWS-managed lists are reference data, not the user's own config.
        self.is_aws_managed()
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        let mut parts = vec![self.id.clone(), self.name.clone(), self.state.clone()];
        if let Some(f) = &self.address_family {
            parts.push(f.clone());
        }
        if let Some(o) = &self.owner_id {
            parts.push(o.clone());
        }
        for (k, v) in &self.tags {
            parts.push(format!("{}:{}", k, v));
        }
        parts.join(" ")
    }

    fn details(&self) -> Vec<(String, String)> {
        let mut details = vec![
            ("ID".to_string(), self.id.clone()),
            ("Name".to_string(), self.name.clone()),
            ("State".to_string(), self.state.clone()),
        ];
        if let Some(f) = &self.address_family {
            details.push(("Address Family".to_string(), f.clone()));
        }
        if let Some(m) = self.max_entries {
            details.push(("Max Entries".to_string(), m.to_string()));
        }
        details
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{region}.console.aws.amazon.com/vpc/home?region={region}#ManagedPrefixListDetails:PrefixListId={}",
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

/// `GetManagedPrefixListEntries` — the CIDRs behind a `pl-` id. Feeds the
/// lazy `LazyStore::pl_entries` map as (cidr, description) rows.
pub async fn fetch_pl_entries(
    client: &Ec2Client,
    prefix_list_id: &str,
) -> std::result::Result<Vec<(String, String)>, String> {
    let mut paginator = client
        .get_managed_prefix_list_entries()
        .prefix_list_id(prefix_list_id)
        .into_paginator()
        .send();
    let mut entries = Vec::new();
    while let Some(page) = paginator.next().await {
        let page = page.map_err(|e| crate::error::sdk_error_message(&e))?;
        for e in page.entries() {
            entries.push((
                e.cidr().unwrap_or("—").to_string(),
                e.description().unwrap_or_default().to_string(),
            ));
        }
    }
    Ok(entries)
}

// ── Egress-Only Internet Gateway ──────────────────────────────────────────────

/// IPv6-only outbound gateway (`eigw-…`) — the NAT-gateway analogue for IPv6.
#[derive(Clone, Debug)]
pub struct EgressOnlyIgw {
    pub id: String,
    pub vpc_id: Option<String>,
    pub attachment_state: Option<String>,
    pub tags: HashMap<String, String>,
}

impl EgressOnlyIgw {
    pub fn from_sdk(g: &aws_sdk_ec2::types::EgressOnlyInternetGateway) -> Self {
        let mut tags = HashMap::new();
        for tag in g.tags() {
            if let (Some(key), Some(value)) = (tag.key(), tag.value()) {
                tags.insert(key.to_string(), value.to_string());
            }
        }
        let att = g.attachments().first();
        Self {
            id: g
                .egress_only_internet_gateway_id()
                .unwrap_or("unknown")
                .to_string(),
            vpc_id: att.and_then(|a| a.vpc_id()).map(|s| s.to_string()),
            attachment_state: att
                .and_then(|a| a.state())
                .map(|s| s.as_str().to_string()),
            tags,
        }
    }
}

impl Resource for EgressOnlyIgw {
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws ec2 describe-egress-only-internet-gateways --egress-only-internet-gateway-ids {}",
            crate::aws::resource::shell_quote(self.id())
        ))
    }

    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        self.tags
            .get("Name")
            .map(|s| s.as_str())
            .unwrap_or(&self.id)
    }

    fn resource_type(&self) -> &str {
        "Egress-Only IGW"
    }

    fn state(&self) -> ResourceState {
        match self.attachment_state.as_deref() {
            Some("attached") | Some("available") => ResourceState::Available,
            Some("attaching") => ResourceState::Pending,
            Some(other) => ResourceState::Unknown(other.to_string()),
            None => ResourceState::Unknown("detached".to_string()),
        }
    }

    fn state_label(&self) -> String {
        native_state_label(self.attachment_state.as_deref().unwrap_or(""), || {
            self.state()
        })
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        let mut parts = vec![self.id.clone(), self.name().to_string(), "ipv6".to_string()];
        if let Some(v) = &self.vpc_id {
            parts.push(v.clone());
        }
        for (k, v) in &self.tags {
            parts.push(format!("{}:{}", k, v));
        }
        parts.join(" ")
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("ID".to_string(), self.id.clone()),
            (
                "VPC".to_string(),
                self.vpc_id.clone().unwrap_or_else(|| "—".to_string()),
            ),
            (
                "Attachment".to_string(),
                self.attachment_state
                    .clone()
                    .unwrap_or_else(|| "detached".to_string()),
            ),
            ("Kind".to_string(), "Egress-only (IPv6 outbound)".to_string()),
        ]
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{region}.console.aws.amazon.com/vpc/home?region={region}#EgressOnlyInternetGatewayDetails:EgressOnlyInternetGatewayId={}",
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

// ── Virtual Private Gateway ───────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct VpnGateway {
    pub id: String,
    pub state: String,
    pub gateway_type: Option<String>,
    pub amazon_side_asn: Option<i64>,
    pub availability_zone: Option<String>,
    pub attached_vpcs: Vec<(String, String)>,
    pub tags: HashMap<String, String>,
}

impl VpnGateway {
    pub fn from_sdk(g: &aws_sdk_ec2::types::VpnGateway) -> Self {
        let mut tags = HashMap::new();
        for tag in g.tags() {
            if let (Some(key), Some(value)) = (tag.key(), tag.value()) {
                tags.insert(key.to_string(), value.to_string());
            }
        }
        Self {
            id: g.vpn_gateway_id().unwrap_or("unknown").to_string(),
            state: g
                .state()
                .map(|s| s.as_str().to_string())
                .unwrap_or_else(|| "unknown".to_string()),
            gateway_type: g.r#type().map(|t| t.as_str().to_string()),
            amazon_side_asn: g.amazon_side_asn(),
            availability_zone: g.availability_zone().map(|s| s.to_string()),
            attached_vpcs: g
                .vpc_attachments()
                .iter()
                .filter_map(|a| {
                    a.vpc_id().map(|v| {
                        (
                            v.to_string(),
                            a.state()
                                .map(|s| s.as_str().to_string())
                                .unwrap_or_else(|| "unknown".to_string()),
                        )
                    })
                })
                .collect(),
            tags,
        }
    }
}

impl Resource for VpnGateway {
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws ec2 describe-vpn-gateways --vpn-gateway-ids {}",
            crate::aws::resource::shell_quote(self.id())
        ))
    }

    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        self.tags
            .get("Name")
            .map(|s| s.as_str())
            .unwrap_or(&self.id)
    }

    fn resource_type(&self) -> &str {
        "VPN Gateway"
    }

    fn state(&self) -> ResourceState {
        match self.state.as_str() {
            "available" => ResourceState::Available,
            "pending" => ResourceState::Pending,
            "deleting" | "deleted" => ResourceState::Unavailable,
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
        let mut parts = vec![self.id.clone(), self.name().to_string(), self.state.clone()];
        for (v, _) in &self.attached_vpcs {
            parts.push(v.clone());
        }
        for (k, v) in &self.tags {
            parts.push(format!("{}:{}", k, v));
        }
        parts.join(" ")
    }

    fn details(&self) -> Vec<(String, String)> {
        let mut details = vec![
            ("ID".to_string(), self.id.clone()),
            ("State".to_string(), self.state.clone()),
        ];
        if let Some(t) = &self.gateway_type {
            details.push(("Type".to_string(), t.clone()));
        }
        if let Some(asn) = self.amazon_side_asn {
            details.push(("Amazon-side ASN".to_string(), asn.to_string()));
        }
        if let Some(az) = &self.availability_zone {
            details.push(("Availability Zone".to_string(), az.clone()));
        }
        if self.attached_vpcs.is_empty() {
            details.push(("Attached VPC".to_string(), "— (detached)".to_string()));
        }
        for (vpc, state) in &self.attached_vpcs {
            details.push(("Attached VPC".to_string(), format!("{} ({})", vpc, state)));
        }
        details
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{region}.console.aws.amazon.com/vpc/home?region={region}#VpnGatewayDetails:VpnGatewayId={}",
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

// ── Customer Gateway ──────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct CustomerGateway {
    pub id: String,
    pub state: String,
    pub ip_address: Option<String>,
    pub bgp_asn: Option<String>,
    pub gateway_type: Option<String>,
    pub device_name: Option<String>,
    pub certificate_arn: Option<String>,
    pub tags: HashMap<String, String>,
}

impl CustomerGateway {
    pub fn from_sdk(g: &aws_sdk_ec2::types::CustomerGateway) -> Self {
        let mut tags = HashMap::new();
        for tag in g.tags() {
            if let (Some(key), Some(value)) = (tag.key(), tag.value()) {
                tags.insert(key.to_string(), value.to_string());
            }
        }
        Self {
            id: g.customer_gateway_id().unwrap_or("unknown").to_string(),
            state: g.state().unwrap_or("unknown").to_string(),
            ip_address: g.ip_address().map(|s| s.to_string()),
            bgp_asn: g
                .bgp_asn()
                .or(g.bgp_asn_extended())
                .map(|s| s.to_string()),
            gateway_type: g.r#type().map(|s| s.to_string()),
            device_name: g.device_name().map(|s| s.to_string()),
            certificate_arn: g.certificate_arn().map(|s| s.to_string()),
            tags,
        }
    }
}

impl Resource for CustomerGateway {
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws ec2 describe-customer-gateways --customer-gateway-ids {}",
            crate::aws::resource::shell_quote(self.id())
        ))
    }

    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        self.tags
            .get("Name")
            .map(|s| s.as_str())
            .unwrap_or(&self.id)
    }

    fn resource_type(&self) -> &str {
        "Customer Gateway"
    }

    fn state(&self) -> ResourceState {
        match self.state.as_str() {
            "available" => ResourceState::Available,
            "pending" => ResourceState::Pending,
            "deleting" | "deleted" => ResourceState::Unavailable,
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
        let mut parts = vec![self.id.clone(), self.name().to_string(), self.state.clone()];
        if let Some(ip) = &self.ip_address {
            parts.push(ip.clone());
        }
        if let Some(d) = &self.device_name {
            parts.push(d.clone());
        }
        for (k, v) in &self.tags {
            parts.push(format!("{}:{}", k, v));
        }
        parts.join(" ")
    }

    fn details(&self) -> Vec<(String, String)> {
        let mut details = vec![
            ("ID".to_string(), self.id.clone()),
            ("State".to_string(), self.state.clone()),
        ];
        if let Some(ip) = &self.ip_address {
            details.push(("IP Address".to_string(), ip.clone()));
        }
        if let Some(asn) = &self.bgp_asn {
            details.push(("BGP ASN".to_string(), asn.clone()));
        }
        if let Some(t) = &self.gateway_type {
            details.push(("Type".to_string(), t.clone()));
        }
        if let Some(d) = &self.device_name {
            details.push(("Device".to_string(), d.clone()));
        }
        if let Some(c) = &self.certificate_arn {
            details.push(("Certificate".to_string(), c.clone()));
        }
        details
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{region}.console.aws.amazon.com/vpc/home?region={region}#CustomerGatewayDetails:CustomerGatewayId={}",
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

// ── VPC Flow Logs (lazy VPC-pane section) ─────────────────────────────────────

/// One VPC-level flow log, flattened for the VPC pane's Flow Logs section.
#[derive(Debug, Clone)]
pub struct FlowLogInfo {
    pub id: String,
    pub status: String,
    pub traffic_type: String,
    pub dest_type: String,
    pub destination: String,
    pub log_format_custom: bool,
    pub aggregation_secs: Option<i32>,
    pub deliver_error: Option<String>,
}

/// `DescribeVpcAttribute` × 2 — the API answers one attribute per call.
/// Feeds `LazyStore::vpc_dns_attrs` as (enableDnsSupport, enableDnsHostnames).
pub async fn fetch_vpc_dns_attrs(
    client: &Ec2Client,
    vpc_id: &str,
) -> std::result::Result<(bool, bool), String> {
    use aws_sdk_ec2::types::VpcAttributeName;
    let support = client
        .describe_vpc_attribute()
        .vpc_id(vpc_id)
        .attribute(VpcAttributeName::EnableDnsSupport)
        .send()
        .await
        .map_err(|e| crate::error::sdk_error_message(&e))?;
    let hostnames = client
        .describe_vpc_attribute()
        .vpc_id(vpc_id)
        .attribute(VpcAttributeName::EnableDnsHostnames)
        .send()
        .await
        .map_err(|e| crate::error::sdk_error_message(&e))?;
    Ok((
        support
            .enable_dns_support()
            .and_then(|a| a.value())
            .unwrap_or(false),
        hostnames
            .enable_dns_hostnames()
            .and_then(|a| a.value())
            .unwrap_or(false),
    ))
}

/// AWS's default flow-log record format — anything else is a custom format.
const DEFAULT_FLOW_LOG_FORMAT: &str = "${version} ${account-id} ${interface-id} ${srcaddr} ${dstaddr} ${srcport} ${dstport} ${protocol} ${packets} ${bytes} ${start} ${end} ${action} ${log-status}";

/// `DescribeFlowLogs` filtered to one VPC (VPC-level logs only — subnet/ENI
/// flow logs don't match the resource-id filter).
pub async fn fetch_vpc_flow_logs(
    client: &Ec2Client,
    vpc_id: &str,
) -> std::result::Result<Vec<FlowLogInfo>, String> {
    let resp = client
        .describe_flow_logs()
        .filter(
            aws_sdk_ec2::types::Filter::builder()
                .name("resource-id")
                .values(vpc_id)
                .build(),
        )
        .send()
        .await
        .map_err(|e| crate::error::sdk_error_message(&e))?;
    Ok(resp.flow_logs().iter().map(flow_log_from_sdk).collect())
}

fn flow_log_from_sdk(f: &aws_sdk_ec2::types::FlowLog) -> FlowLogInfo {
    // CloudWatch Logs destinations carry a group name; S3 / Firehose
    // destinations carry an ARN.
    let destination = f
        .log_group_name()
        .or(f.log_destination())
        .unwrap_or("—")
        .to_string();
    FlowLogInfo {
        id: f.flow_log_id().unwrap_or("unknown").to_string(),
        status: f.flow_log_status().unwrap_or("unknown").to_string(),
        traffic_type: f
            .traffic_type()
            .map(|t| t.as_str().to_string())
            .unwrap_or_else(|| "—".to_string()),
        dest_type: f
            .log_destination_type()
            .map(|t| t.as_str().to_string())
            .unwrap_or_else(|| "—".to_string()),
        destination,
        log_format_custom: f
            .log_format()
            .is_some_and(|fmt| fmt != DEFAULT_FLOW_LOG_FORMAT),
        aggregation_secs: f.max_aggregation_interval(),
        deliver_error: f.deliver_logs_error_message().map(|s| s.to_string()),
    }
}

// ── CloudWatch metrics (`m` overlay) ──────────────────────────────────────────

pub use crate::aws::services::ec2::{parse_metric_datapoints, MetricsTimeRange};

#[derive(Debug, Clone)]
pub struct NatMetricsData {
    pub time_range: MetricsTimeRange,
    pub bytes_in_from_dest: Vec<(f64, f64)>,
    pub bytes_out_to_dest: Vec<(f64, f64)>,
    pub active_connections: Vec<(f64, f64)>,
    pub connections_established: Vec<(f64, f64)>,
    pub port_alloc_errors: Vec<(f64, f64)>,
    pub packets_dropped: Vec<(f64, f64)>,
    pub x_max: f64,
}

#[derive(Debug, Clone)]
pub enum NatMetricsState {
    Loading,
    Loaded(NatMetricsData),
}

/// Pull `AWS/NATGateway` metrics for one NAT gateway. Bytes to/from the
/// destination are the traffic headline; port-allocation errors and dropped
/// packets are the saturation signals (non-zero = scale out or split subnets).
pub async fn fetch_nat_metrics(
    cw: aws_sdk_cloudwatch::Client,
    nat_id: String,
    time_range: MetricsTimeRange,
) -> crate::error::Result<NatMetricsData> {
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
            .namespace("AWS/NATGateway")
            .metric_name(name)
            .dimensions(Dimension::builder().name("NatGatewayId").value(&nat_id).build())
            .start_time(start_dt)
            .end_time(end_dt)
            .period(period)
            .set_statistics(Some(vec![stat]))
            .send()
    };

    let (bytes_in, bytes_out, active, established, port_errors, drops) = tokio::join!(
        metric("BytesInFromDestination", Statistic::Sum),
        metric("BytesOutToDestination", Statistic::Sum),
        metric("ActiveConnectionCount", Statistic::Maximum),
        metric("ConnectionEstablishedCount", Statistic::Sum),
        metric("ErrorPortAllocation", Statistic::Sum),
        metric("PacketsDropCount", Statistic::Sum),
    );

    Ok(NatMetricsData {
        time_range,
        bytes_in_from_dest: parse_metric_datapoints(bytes_in, start),
        bytes_out_to_dest: parse_metric_datapoints(bytes_out, start),
        active_connections: parse_metric_datapoints(active, start),
        connections_established: parse_metric_datapoints(established, start),
        port_alloc_errors: parse_metric_datapoints(port_errors, start),
        packets_dropped: parse_metric_datapoints(drops, start),
        x_max: time_range.duration_secs() as f64,
    })
}

#[derive(Debug, Clone)]
pub struct VpnMetricsData {
    pub time_range: MetricsTimeRange,
    pub tunnel_state: Vec<(f64, f64)>,
    pub data_in: Vec<(f64, f64)>,
    pub data_out: Vec<(f64, f64)>,
    pub x_max: f64,
}

#[derive(Debug, Clone)]
pub enum VpnMetricsState {
    Loading,
    Loaded(VpnMetricsData),
}

/// Pull `AWS/VPN` metrics for one VPN connection (dimension `VpnId` aggregates
/// both tunnels). `TunnelState` averages to the fraction of tunnels up
/// (1.0 = both, 0.5 = one down); data in/out are the throughput signals.
pub async fn fetch_vpn_metrics(
    cw: aws_sdk_cloudwatch::Client,
    vpn_id: String,
    time_range: MetricsTimeRange,
) -> crate::error::Result<VpnMetricsData> {
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
            .namespace("AWS/VPN")
            .metric_name(name)
            .dimensions(Dimension::builder().name("VpnId").value(&vpn_id).build())
            .start_time(start_dt)
            .end_time(end_dt)
            .period(period)
            .set_statistics(Some(vec![stat]))
            .send()
    };

    let (state, data_in, data_out) = tokio::join!(
        metric("TunnelState", Statistic::Average),
        metric("TunnelDataIn", Statistic::Sum),
        metric("TunnelDataOut", Statistic::Sum),
    );

    Ok(VpnMetricsData {
        time_range,
        tunnel_state: parse_metric_datapoints(state, start),
        data_in: parse_metric_datapoints(data_in, start),
        data_out: parse_metric_datapoints(data_out, start),
        x_max: time_range.duration_secs() as f64,
    })
}

#[derive(Debug, Clone)]
pub struct VpceMetricsData {
    pub time_range: MetricsTimeRange,
    pub active_connections: Vec<(f64, f64)>,
    pub new_connections: Vec<(f64, f64)>,
    pub bytes_processed: Vec<(f64, f64)>,
    pub packets_dropped: Vec<(f64, f64)>,
    pub rst_packets: Vec<(f64, f64)>,
    pub x_max: f64,
}

#[derive(Debug, Clone)]
pub enum VpceMetricsState {
    Loading,
    Loaded(VpceMetricsData),
}

/// Pull `AWS/PrivateLinkEndpoints` metrics for one interface / GWLB endpoint.
/// The namespace publishes under the full dimension set (Endpoint Type,
/// Service Name, VPC Endpoint Id, VPC Id), so a plain `GetMetricStatistics`
/// on `VPC Endpoint Id` alone matches nothing — aggregate with the
/// `GetMetricData` SEARCH pattern instead (like MSK / Network Firewall).
/// Gateway endpoints (S3 / DynamoDB route-table style) publish nothing;
/// callers hint instead of fetching.
pub async fn fetch_vpce_metrics(
    cw: aws_sdk_cloudwatch::Client,
    endpoint_id: String,
    time_range: MetricsTimeRange,
) -> crate::error::Result<VpceMetricsData> {
    use aws_sdk_cloudwatch::types::MetricDataQuery;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let start = now - time_range.duration_secs();
    let period = time_range.period_secs();

    // (id, aggregate, inner stat, metric)
    let specs = [
        ("m0", "MAX", "Maximum", "ActiveConnections"),
        ("m1", "SUM", "Sum", "NewConnections"),
        ("m2", "SUM", "Sum", "BytesProcessed"),
        ("m3", "SUM", "Sum", "PacketsDropped"),
        ("m4", "SUM", "Sum", "RstPacketsReceived"),
    ];

    let queries: Vec<MetricDataQuery> = specs
        .iter()
        .map(|(id, agg, stat, metric)| {
            let expr = format!(
                "{}(SEARCH('{{AWS/PrivateLinkEndpoints,\"Endpoint Type\",\"Service Name\",\"VPC Endpoint Id\",\"VPC Id\"}} MetricName=\"{}\" \"VPC Endpoint Id\"=\"{}\"', '{}', {}))",
                agg, metric, endpoint_id, stat, period
            );
            MetricDataQuery::builder().id(*id).expression(expr).build()
        })
        .collect();

    let resp = cw
        .get_metric_data()
        .set_metric_data_queries(Some(queries))
        .start_time(aws_sdk_cloudwatch::primitives::DateTime::from_secs(start))
        .end_time(aws_sdk_cloudwatch::primitives::DateTime::from_secs(now))
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;

    let mut series: std::collections::HashMap<String, Vec<(f64, f64)>> =
        std::collections::HashMap::new();
    for r in resp.metric_data_results() {
        let id = r.id().unwrap_or_default().to_string();
        let mut pts: Vec<(f64, f64)> = r
            .timestamps()
            .iter()
            .zip(r.values().iter())
            .map(|(t, v)| (t.secs() as f64 - start as f64, *v))
            .collect();
        pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        series.insert(id, pts);
    }

    let take = |id: &str| series.get(id).cloned().unwrap_or_default();
    Ok(VpceMetricsData {
        time_range,
        active_connections: take("m0"),
        new_connections: take("m1"),
        bytes_processed: take("m2"),
        packets_dropped: take("m3"),
        rst_packets: take("m4"),
        x_max: time_range.duration_secs() as f64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_sdk_ec2::types::{AttributeValue, DhcpConfiguration, DhcpOptions, Tag};

    fn sample_dhcp_options() -> DhcpOptionsSet {
        let av = |v: &str| AttributeValue::builder().value(v).build();
        let cfg = |k: &str, vals: &[&str]| {
            let mut b = DhcpConfiguration::builder().key(k);
            for v in vals {
                b = b.values(av(v));
            }
            b.build()
        };
        let sdk = DhcpOptions::builder()
            .dhcp_options_id("dopt-0abc")
            .owner_id("123456789012")
            .dhcp_configurations(cfg("ntp-servers", &["10.0.0.5"]))
            .dhcp_configurations(cfg(
                "domain-name-servers",
                &["AmazonProvidedDNS", "10.0.0.2"],
            ))
            .dhcp_configurations(cfg("domain-name", &["corp.example.com"]))
            .tags(Tag::builder().key("Name").value("corp-dns").build())
            .build();
        DhcpOptionsSet::from_sdk(&sdk)
    }

    #[test]
    fn dhcp_configs_parse_sorted_by_key() {
        let d = sample_dhcp_options();
        let keys: Vec<&str> = d.configs.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, vec!["domain-name", "domain-name-servers", "ntp-servers"]);
        assert_eq!(
            d.values_for("domain-name-servers").as_deref(),
            Some("AmazonProvidedDNS, 10.0.0.2")
        );
        assert_eq!(d.values_for("netbios-node-type"), None);
    }

    #[test]
    fn dhcp_options_name_prefers_name_tag() {
        let d = sample_dhcp_options();
        assert_eq!(d.name(), "corp-dns");
        assert_eq!(d.id(), "dopt-0abc");
        let untagged = DhcpOptionsSet {
            tags: HashMap::new(),
            ..d
        };
        assert_eq!(untagged.name(), "dopt-0abc");
    }

    #[test]
    fn dhcp_keys_prettify_with_titlecase_fallback() {
        assert_eq!(pretty_dhcp_key("domain-name-servers"), "Domain Name Servers");
        assert_eq!(pretty_dhcp_key("ntp-servers"), "NTP Servers");
        assert_eq!(pretty_dhcp_key("netbios-node-type"), "NetBIOS Node Type");
        assert_eq!(pretty_dhcp_key("some-new-option"), "Some New Option");
    }

    fn peering_with_status(code: aws_sdk_ec2::types::VpcPeeringConnectionStateReasonCode) -> VpcPeering {
        use aws_sdk_ec2::types::{
            CidrBlock, VpcPeeringConnection, VpcPeeringConnectionStateReason,
            VpcPeeringConnectionVpcInfo,
        };
        let sdk = VpcPeeringConnection::builder()
            .vpc_peering_connection_id("pcx-0abc")
            .status(VpcPeeringConnectionStateReason::builder().code(code).build())
            .requester_vpc_info(
                VpcPeeringConnectionVpcInfo::builder()
                    .vpc_id("vpc-req")
                    .cidr_block("10.0.0.0/16")
                    .cidr_block_set(CidrBlock::builder().cidr_block("10.0.0.0/16").build())
                    .cidr_block_set(CidrBlock::builder().cidr_block("10.1.0.0/16").build())
                    .owner_id("111111111111")
                    .region("us-east-1")
                    .build(),
            )
            .accepter_vpc_info(
                VpcPeeringConnectionVpcInfo::builder()
                    .vpc_id("vpc-acc")
                    .cidr_block("172.16.0.0/16")
                    .build(),
            )
            .build();
        VpcPeering::from_sdk(&sdk)
    }

    #[test]
    fn peering_states_map_to_colors() {
        use aws_sdk_ec2::types::VpcPeeringConnectionStateReasonCode as Code;
        assert!(matches!(
            peering_with_status(Code::Active).state(),
            ResourceState::Available
        ));
        assert!(matches!(
            peering_with_status(Code::PendingAcceptance).state(),
            ResourceState::Pending
        ));
        assert!(matches!(
            peering_with_status(Code::Failed).state(),
            ResourceState::Unavailable
        ));
    }

    #[test]
    fn peering_sides_dedup_cidrs_and_parse_both_ends() {
        use aws_sdk_ec2::types::VpcPeeringConnectionStateReasonCode as Code;
        let p = peering_with_status(Code::Active);
        // cidr_block also appears in cidr_block_set — deduped.
        assert_eq!(p.requester.cidrs, vec!["10.0.0.0/16", "10.1.0.0/16"]);
        assert_eq!(p.requester.vpc_id.as_deref(), Some("vpc-req"));
        assert_eq!(p.accepter.vpc_id.as_deref(), Some("vpc-acc"));
        assert_eq!(p.accepter.cidrs, vec!["172.16.0.0/16"]);
    }

    #[test]
    fn aws_managed_prefix_lists_are_noise() {
        use aws_sdk_ec2::types::{ManagedPrefixList, PrefixListState};
        let aws = PrefixList::from_sdk(
            &ManagedPrefixList::builder()
                .prefix_list_id("pl-aws")
                .prefix_list_name("com.amazonaws.us-east-1.s3")
                .owner_id("AWS")
                .state(PrefixListState::CreateComplete)
                .build(),
        );
        assert!(aws.is_noise());
        assert!(matches!(aws.state(), ResourceState::Available));
        let mine = PrefixList {
            owner_id: Some("111111111111".to_string()),
            ..aws
        };
        assert!(!mine.is_noise());
    }

    #[test]
    fn gateway_rows_parse_and_map_states() {
        use aws_sdk_ec2::types::{
            AttachmentStatus, CustomerGateway as SdkCgw, EgressOnlyInternetGateway,
            InternetGatewayAttachment, VpcAttachment, VpnGateway as SdkVgw, VpnState,
        };
        let eigw = EgressOnlyIgw::from_sdk(
            &EgressOnlyInternetGateway::builder()
                .egress_only_internet_gateway_id("eigw-1")
                .attachments(
                    InternetGatewayAttachment::builder()
                        .vpc_id("vpc-1")
                        .state(AttachmentStatus::Attached)
                        .build(),
                )
                .build(),
        );
        assert_eq!(eigw.vpc_id.as_deref(), Some("vpc-1"));
        assert!(matches!(eigw.state(), ResourceState::Available));

        let vgw = VpnGateway::from_sdk(
            &SdkVgw::builder()
                .vpn_gateway_id("vgw-1")
                .state(VpnState::Available)
                .amazon_side_asn(64512)
                .vpc_attachments(
                    VpcAttachment::builder()
                        .vpc_id("vpc-1")
                        .state(AttachmentStatus::Attached)
                        .build(),
                )
                .build(),
        );
        assert!(matches!(vgw.state(), ResourceState::Available));
        assert_eq!(vgw.attached_vpcs, vec![("vpc-1".to_string(), "attached".to_string())]);

        let cgw = CustomerGateway::from_sdk(
            &SdkCgw::builder()
                .customer_gateway_id("cgw-1")
                .state("available")
                .ip_address("203.0.113.10")
                .bgp_asn("65000")
                .build(),
        );
        assert!(matches!(cgw.state(), ResourceState::Available));
        assert_eq!(cgw.bgp_asn.as_deref(), Some("65000"));
    }

    #[test]
    fn flow_logs_flag_custom_formats_only() {
        use aws_sdk_ec2::types::{FlowLog, LogDestinationType, TrafficType};
        let default_fmt = FlowLog::builder()
            .flow_log_id("fl-1")
            .flow_log_status("ACTIVE")
            .traffic_type(TrafficType::All)
            .log_destination_type(LogDestinationType::CloudWatchLogs)
            .log_group_name("/vpc/flow")
            .log_format(super::DEFAULT_FLOW_LOG_FORMAT)
            .build();
        let info = super::flow_log_from_sdk(&default_fmt);
        assert!(!info.log_format_custom);
        assert_eq!(info.destination, "/vpc/flow");

        let custom = FlowLog::builder()
            .flow_log_id("fl-2")
            .log_destination_type(LogDestinationType::S3)
            .log_destination("arn:aws:s3:::my-flow-bucket")
            .log_format("${srcaddr} ${dstaddr}")
            .build();
        let info = super::flow_log_from_sdk(&custom);
        assert!(info.log_format_custom);
        assert_eq!(info.destination, "arn:aws:s3:::my-flow-bucket");
    }

    #[test]
    fn prefix_list_states_map_by_suffix() {
        let pl = |s: &str| PrefixList {
            id: "pl-1".into(),
            arn: None,
            name: "x".into(),
            state: s.into(),
            state_message: None,
            address_family: None,
            max_entries: None,
            version: None,
            owner_id: None,
            tags: HashMap::new(),
        };
        assert!(matches!(pl("modify-complete").state(), ResourceState::Available));
        assert!(matches!(pl("create-in-progress").state(), ResourceState::Pending));
        assert!(matches!(pl("create-failed").state(), ResourceState::Unavailable));
        assert!(matches!(pl("delete-complete").state(), ResourceState::Unavailable));
    }
}
