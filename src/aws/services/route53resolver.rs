use crate::aws::client::AwsClients;
use crate::aws::resource::{native_state_label, Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use aws_sdk_route53resolver::Client as ResolverClient;
use std::any::Any;
use std::collections::HashMap;
use tokio::sync::mpsc;

/// Route 53 Resolver — a standalone *regional* service with two resource types
/// over one heterogeneous list: resolver endpoints (INBOUND / OUTBOUND) and
/// resolver rules (FORWARD / SYSTEM / RECURSIVE). Endpoint IP addresses, rule
/// associations, and tags are fetched lazily (see the `fetch_*` helpers).
pub struct Route53ResolverService {
    client: ResolverClient,
}

impl Route53ResolverService {
    pub fn new(aws_clients: &AwsClients) -> Self {
        Self {
            client: aws_clients.route53resolver_client(),
        }
    }
}

#[async_trait]
impl AwsService for Route53ResolverService {
    fn service_type(&self) -> ServiceType {
        ServiceType::Route53Resolver
    }

    fn name(&self) -> &str {
        "Route53 Resolver"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        self.list_resources_streaming(tx, ServiceType::Route53Resolver)
            .await?;
        Ok(vec![])
    }

    async fn list_resources_streaming(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        let mut total = 0usize;

        // Phase 1: Resolver endpoints (fluent paginator).
        let mut stream = self
            .client
            .list_resolver_endpoints()
            .into_paginator()
            .send();
        loop {
            match stream.next().await {
                Some(Ok(page)) => {
                    let batch: Vec<Box<dyn Resource>> = page
                        .resolver_endpoints()
                        .iter()
                        .map(|e| Box::new(ResolverEndpoint::from_sdk(e)) as Box<dyn Resource>)
                        .collect();
                    total += batch.len();
                    let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                        service: service_type,
                        resources: batch,
                        progress: LoadProgress {
                            loaded_count: total,
                            total_count: None,
                            status_message: Some("Loading resolver endpoints…".to_string()),
                        },
                    });
                }
                Some(Err(e)) => {
                    let _ = event_tx.send(Event::ResourceLoadWarning {
                        service: service_type,
                        warning: format!(
                            "resolver endpoints: {}",
                            crate::error::sdk_error_message(&e)
                        ),
                    });
                    break;
                }
                None => break,
            }
        }

        // Phase 2: Resolver rules (fluent paginator).
        let mut stream = self.client.list_resolver_rules().into_paginator().send();
        loop {
            match stream.next().await {
                Some(Ok(page)) => {
                    let batch: Vec<Box<dyn Resource>> = page
                        .resolver_rules()
                        .iter()
                        .map(|r| Box::new(ResolverRule::from_sdk(r)) as Box<dyn Resource>)
                        .collect();
                    total += batch.len();
                    let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                        service: service_type,
                        resources: batch,
                        progress: LoadProgress {
                            loaded_count: total,
                            total_count: None,
                            status_message: Some("Loading resolver rules…".to_string()),
                        },
                    });
                }
                Some(Err(e)) => {
                    let _ = event_tx.send(Event::ResourceLoadWarning {
                        service: service_type,
                        warning: format!(
                            "resolver rules: {}",
                            crate::error::sdk_error_message(&e)
                        ),
                    });
                    break;
                }
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

// ── helpers ─────────────────────────────────────────────────────────────────

fn state_endpoint(raw: &str) -> ResourceState {
    match raw {
        "OPERATIONAL" => ResourceState::Available,
        "CREATING" | "UPDATING" | "AUTO_RECOVERING" => ResourceState::Pending,
        "ACTION_NEEDED" => ResourceState::Unavailable,
        "DELETING" => ResourceState::Deleting,
        other => ResourceState::Unknown(other.to_string()),
    }
}

fn state_rule(raw: &str) -> ResourceState {
    match raw {
        "COMPLETE" => ResourceState::Available,
        "UPDATING" => ResourceState::Pending,
        "DELETING" => ResourceState::Deleting,
        "FAILED" => ResourceState::Unavailable,
        other => ResourceState::Unknown(other.to_string()),
    }
}

// ── ResolverEndpoint ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ResolverEndpoint {
    pub id: String,
    pub arn: String,
    pub name: String,
    pub direction: String,
    pub status: String,
    pub status_message: String,
    pub ip_address_count: i32,
    pub host_vpc_id: String,
    pub security_group_ids: Vec<String>,
    pub creation_time: Option<String>,
    /// Empty — Resolver tags are shown lazily in the Tags section; this backs
    /// the `Resource::tags()` fallback only.
    pub tags: HashMap<String, String>,
}

impl ResolverEndpoint {
    pub fn from_sdk(e: &aws_sdk_route53resolver::types::ResolverEndpoint) -> Self {
        Self {
            id: e.id().unwrap_or_default().to_string(),
            arn: e.arn().unwrap_or_default().to_string(),
            name: e.name().unwrap_or_default().to_string(),
            direction: e
                .direction()
                .map(|d| d.as_str().to_string())
                .unwrap_or_default(),
            status: e
                .status()
                .map(|s| s.as_str().to_string())
                .unwrap_or_default(),
            status_message: e.status_message().unwrap_or_default().to_string(),
            ip_address_count: e.ip_address_count().unwrap_or_default(),
            host_vpc_id: e.host_vpc_id().unwrap_or_default().to_string(),
            security_group_ids: e.security_group_ids().to_vec(),
            creation_time: e.creation_time().map(|s| s.to_string()),
            tags: HashMap::new(),
        }
    }
}

crate::sections! {
    pub enum ResolverEndpointDetailSection,
    pub static RESOLVER_ENDPOINT_SECTIONS = [
        Details "Details" => crate::app::App::trigger_resolver_endpoint_load,
        IpAddresses "IP Addresses" => crate::app::App::trigger_resolver_endpoint_load,
        Tags "Tags" => crate::app::App::trigger_resolver_endpoint_load,
    ]
}

impl Resource for ResolverEndpoint {
    fn security_group_ids(&self) -> Vec<String> {
        self.security_group_ids.clone()
    }

    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&RESOLVER_ENDPOINT_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.id
    }
    fn name(&self) -> &str {
        if self.name.is_empty() {
            &self.id
        } else {
            &self.name
        }
    }
    fn resource_type(&self) -> &str {
        "Resolver Endpoint"
    }
    fn state(&self) -> ResourceState {
        state_endpoint(&self.status)
    }

    fn state_label(&self) -> String {
        native_state_label(&self.status, || self.state())
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        format!(
            "{} {} {} {}",
            self.id, self.name, self.direction, self.host_vpc_id
        )
    }
    fn details(&self) -> Vec<(String, String)> {
        let mut d = vec![
            ("ID".to_string(), self.id.clone()),
            ("Name".to_string(), self.name.clone()),
            ("Direction".to_string(), self.direction.clone()),
            ("Status".to_string(), self.status.clone()),
            (
                "IP Address Count".to_string(),
                self.ip_address_count.to_string(),
            ),
            ("Host VPC".to_string(), self.host_vpc_id.clone()),
        ];
        if !self.status_message.is_empty() {
            d.push(("Status Message".to_string(), self.status_message.clone()));
        }
        if !self.security_group_ids.is_empty() {
            d.push((
                "Security Groups".to_string(),
                self.security_group_ids.join(", "),
            ));
        }
        if let Some(ct) = &self.creation_time {
            d.push(("Created".to_string(), ct.clone()));
        }
        d
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{region}.console.aws.amazon.com/route53resolver/home?region={region}#/inbound-endpoints/{}",
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

// ── ResolverEndpointIp (lazy) ─────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ResolverEndpointIp {
    pub ip: String,
    pub ipv6: String,
    pub subnet_id: String,
    pub status: String,
}

// ── ResolverRule ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ResolverRule {
    pub id: String,
    pub arn: String,
    pub name: String,
    pub domain_name: String,
    pub rule_type: String,
    pub status: String,
    pub status_message: String,
    pub resolver_endpoint_id: Option<String>,
    pub target_ips: Vec<String>,
    pub share_status: String,
    pub owner_id: String,
    /// Empty — tags shown lazily; backs the `Resource::tags()` fallback only.
    pub tags: HashMap<String, String>,
}

impl ResolverRule {
    pub fn from_sdk(r: &aws_sdk_route53resolver::types::ResolverRule) -> Self {
        let target_ips = r
            .target_ips()
            .iter()
            .map(|t| {
                let ip = t.ip().unwrap_or_default();
                match t.port() {
                    Some(p) => format!("{}:{}", ip, p),
                    None => ip.to_string(),
                }
            })
            .collect();
        Self {
            id: r.id().unwrap_or_default().to_string(),
            arn: r.arn().unwrap_or_default().to_string(),
            name: r.name().unwrap_or_default().to_string(),
            domain_name: r.domain_name().unwrap_or_default().to_string(),
            rule_type: r
                .rule_type()
                .map(|t| t.as_str().to_string())
                .unwrap_or_default(),
            status: r
                .status()
                .map(|s| s.as_str().to_string())
                .unwrap_or_default(),
            status_message: r.status_message().unwrap_or_default().to_string(),
            resolver_endpoint_id: r
                .resolver_endpoint_id()
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string()),
            target_ips,
            share_status: r
                .share_status()
                .map(|s| s.as_str().to_string())
                .unwrap_or_default(),
            owner_id: r.owner_id().unwrap_or_default().to_string(),
            tags: HashMap::new(),
        }
    }
}

crate::sections! {
    pub enum ResolverRuleDetailSection,
    pub static RESOLVER_RULE_SECTIONS = [
        Details "Details" => crate::app::App::trigger_resolver_rule_load,
        Targets "Targets" => crate::app::App::trigger_resolver_rule_load,
        Associations "Associations" => crate::app::App::trigger_resolver_rule_load,
        Tags "Tags" => crate::app::App::trigger_resolver_rule_load,
    ]
}

impl Resource for ResolverRule {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&RESOLVER_RULE_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.id
    }
    fn name(&self) -> &str {
        if self.name.is_empty() {
            &self.domain_name
        } else {
            &self.name
        }
    }
    fn resource_type(&self) -> &str {
        "Resolver Rule"
    }
    fn state(&self) -> ResourceState {
        state_rule(&self.status)
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
            self.id,
            self.name,
            self.domain_name,
            self.rule_type,
            self.target_ips.join(" ")
        )
    }
    fn details(&self) -> Vec<(String, String)> {
        let mut d = vec![
            ("ID".to_string(), self.id.clone()),
            ("Name".to_string(), self.name.clone()),
            ("Domain Name".to_string(), self.domain_name.clone()),
            ("Rule Type".to_string(), self.rule_type.clone()),
            ("Status".to_string(), self.status.clone()),
        ];
        if !self.status_message.is_empty() {
            d.push(("Status Message".to_string(), self.status_message.clone()));
        }
        if let Some(ep) = &self.resolver_endpoint_id {
            d.push(("Resolver Endpoint".to_string(), ep.clone()));
        }
        if !self.target_ips.is_empty() {
            d.push(("Targets".to_string(), self.target_ips.join(", ")));
        }
        if !self.share_status.is_empty() {
            d.push(("Share Status".to_string(), self.share_status.clone()));
        }
        if !self.owner_id.is_empty() {
            d.push(("Owner".to_string(), self.owner_id.clone()));
        }
        d
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{region}.console.aws.amazon.com/route53resolver/home?region={region}#/rules/{}",
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

// ── ResolverRuleAssociation (lazy) ────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ResolverRuleAssociation {
    pub vpc_id: String,
    pub status: String,
}

// ── Lazy detail: endpoint IP addresses + tags ─────────────────────────────────

#[derive(Debug, Clone)]
pub struct ResolverEndpointDetail {
    pub ip_addresses: Vec<ResolverEndpointIp>,
    pub tags: Vec<(String, String)>,
}

/// Fetch an endpoint's per-AZ IP addresses (`ListResolverEndpointIpAddresses`)
/// plus its tags (`ListTagsForResource`), backing the IpAddresses + Tags
/// sections. Keyed by endpoint id.
pub async fn fetch_resolver_endpoint_detail(
    client: ResolverClient,
    endpoint_id: String,
    arn: String,
) -> Result<ResolverEndpointDetail> {
    let mut ip_addresses = Vec::new();
    let mut next: Option<String> = None;
    loop {
        let mut req = client
            .list_resolver_endpoint_ip_addresses()
            .resolver_endpoint_id(&endpoint_id);
        if let Some(t) = &next {
            req = req.next_token(t);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
        for ip in resp.ip_addresses() {
            ip_addresses.push(ResolverEndpointIp {
                ip: ip.ip().unwrap_or_default().to_string(),
                ipv6: ip.ipv6().unwrap_or_default().to_string(),
                subnet_id: ip.subnet_id().unwrap_or_default().to_string(),
                status: ip
                    .status()
                    .map(|s| s.as_str().to_string())
                    .unwrap_or_default(),
            });
        }
        next = crate::aws::pagination::next_page_token(resp.next_token(), &next);
        if next.is_none() {
            break;
        }
    }
    let tags = fetch_resolver_tags(&client, &arn).await;
    Ok(ResolverEndpointDetail { ip_addresses, tags })
}

// ── Lazy detail: rule associations + tags ─────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ResolverRuleDetail {
    pub associations: Vec<ResolverRuleAssociation>,
    pub tags: Vec<(String, String)>,
}

/// Fetch a rule's VPC associations (`ListResolverRuleAssociations`, filtered to
/// the rule) plus its tags, backing the Associations + Tags sections. Keyed by
/// rule id.
pub async fn fetch_resolver_rule_detail(
    client: ResolverClient,
    rule_id: String,
    arn: String,
) -> Result<ResolverRuleDetail> {
    let filter = aws_sdk_route53resolver::types::Filter::builder()
        .name("ResolverRuleId")
        .values(rule_id.clone())
        .build();

    let mut associations = Vec::new();
    let mut next: Option<String> = None;
    loop {
        let mut req = client
            .list_resolver_rule_associations()
            .filters(filter.clone());
        if let Some(t) = &next {
            req = req.next_token(t);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
        for a in resp.resolver_rule_associations() {
            associations.push(ResolverRuleAssociation {
                vpc_id: a.vpc_id().unwrap_or_default().to_string(),
                status: a
                    .status()
                    .map(|s| s.as_str().to_string())
                    .unwrap_or_default(),
            });
        }
        next = crate::aws::pagination::next_page_token(resp.next_token(), &next);
        if next.is_none() {
            break;
        }
    }
    let tags = fetch_resolver_tags(&client, &arn).await;
    Ok(ResolverRuleDetail { associations, tags })
}

async fn fetch_resolver_tags(client: &ResolverClient, arn: &str) -> Vec<(String, String)> {
    let mut tags = Vec::new();
    let mut next: Option<String> = None;
    loop {
        let mut req = client.list_tags_for_resource().resource_arn(arn);
        if let Some(t) = &next {
            req = req.next_token(t);
        }
        match req.send().await {
            Ok(resp) => {
                for t in resp.tags() {
                    tags.push((t.key().to_string(), t.value().to_string()));
                }
                next = crate::aws::pagination::next_page_token(resp.next_token(), &next);
                if next.is_none() {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    tags
}

// ── Endpoint CloudWatch metrics (`m`) — AWS/Route53Resolver, dim EndpointId ───

use crate::aws::services::ec2::{parse_metric_datapoints, MetricsTimeRange};

#[derive(Debug, Clone)]
pub struct ResolverEpMetricsData {
    pub time_range: MetricsTimeRange,
    pub inbound_queries: Vec<(f64, f64)>,
    pub outbound_queries: Vec<(f64, f64)>,
    pub x_max: f64,
}

#[derive(Debug, Clone)]
pub enum ResolverEpMetricsState {
    Loading,
    Loaded(ResolverEpMetricsData),
}

/// Query volume through one resolver endpoint. Resolver metrics are regional
/// (unlike the Route 53 data plane) — regular regional CloudWatch client.
/// Both directions are fetched; only the endpoint's own direction has data,
/// the other series is simply empty.
pub async fn fetch_resolver_ep_metrics(
    cw: aws_sdk_cloudwatch::Client,
    endpoint_id: String,
    time_range: MetricsTimeRange,
) -> crate::error::Result<ResolverEpMetricsData> {
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
            .namespace("AWS/Route53Resolver")
            .metric_name(name)
            .dimensions(Dimension::builder().name("EndpointId").value(&endpoint_id).build())
            .start_time(start_dt)
            .end_time(end_dt)
            .period(period)
            .set_statistics(Some(vec![Statistic::Sum]))
            .send()
    };

    let (inbound, outbound) =
        tokio::join!(metric("InboundQueryVolume"), metric("OutboundQueryVolume"));

    Ok(ResolverEpMetricsData {
        time_range,
        inbound_queries: parse_metric_datapoints(inbound, start),
        outbound_queries: parse_metric_datapoints(outbound, start),
        x_max: time_range.duration_secs() as f64,
    })
}
