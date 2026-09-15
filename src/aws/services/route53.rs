use crate::aws::client::AwsClients;
use crate::aws::resource::{Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use aws_sdk_route53::Client as R53Client;
use std::any::Any;
use std::collections::HashMap;
use tokio::sync::mpsc;

pub struct Route53Service {
    client: R53Client,
}

impl Route53Service {
    pub fn new(aws_clients: &AwsClients) -> Self {
        Self {
            client: aws_clients.route53_client(),
        }
    }
}

#[async_trait]
impl AwsService for Route53Service {
    fn service_type(&self) -> ServiceType {
        ServiceType::Route53
    }

    fn name(&self) -> &str {
        "Route53"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        self.list_resources_streaming(tx, ServiceType::Route53).await?;
        Ok(vec![])
    }

    async fn list_resources_streaming(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        let mut total = 0usize;
        let mut paginator = self.client.list_hosted_zones().into_paginator().send();

        while let Some(result) = paginator.next().await {
            match result {
                Ok(page) => {
                    let batch: Vec<Box<dyn Resource>> = page
                        .hosted_zones()
                        .iter()
                        .map(|z| Box::new(R53HostedZone::from_sdk(z)) as Box<dyn Resource>)
                        .collect();

                    let count = batch.len();
                    if count == 0 {
                        continue;
                    }
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
                Err(e) => {
                    let _ = event_tx.send(Event::ResourceLoadError {
                        service: service_type,
                        error: format!("Failed to list Route53 hosted zones: {}", e),
                    });
                    return Ok(());
                }
            }
        }

        // Health checks (global, like hosted zones). Streamed as a second batch
        // so the Health Checks sub-tab has data without a separate load. Tags are
        // resolved in batches of 10 via ListTagsForResources. Error-tolerant: a
        // health-check failure never aborts the (already-sent) zones.
        let mut hc_paginator = self.client.list_health_checks().into_paginator().send();
        while let Some(result) = hc_paginator.next().await {
            match result {
                Ok(page) => {
                    let mut checks: Vec<R53HealthCheck> = page
                        .health_checks()
                        .iter()
                        .map(R53HealthCheck::from_sdk)
                        .collect();
                    if checks.is_empty() {
                        continue;
                    }
                    // Batch-resolve tags (up to 10 ids per call).
                    for chunk in checks.chunks_mut(10) {
                        let ids: Vec<String> = chunk.iter().map(|c| c.id.clone()).collect();
                        if let Ok(resp) = self
                            .client
                            .list_tags_for_resources()
                            .resource_type(aws_sdk_route53::types::TagResourceType::Healthcheck)
                            .set_resource_ids(Some(ids))
                            .send()
                            .await
                        {
                            for set in resp.resource_tag_sets() {
                                if let Some(rid) = set.resource_id() {
                                    if let Some(c) = chunk.iter_mut().find(|c| c.id == rid) {
                                        for t in set.tags() {
                                            if let (Some(k), Some(v)) = (t.key(), t.value()) {
                                                c.tags.insert(k.to_string(), v.to_string());
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    total += checks.len();
                    let batch: Vec<Box<dyn Resource>> =
                        checks.into_iter().map(|c| Box::new(c) as Box<dyn Resource>).collect();
                    let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                        service: service_type,
                        resources: batch,
                        progress: LoadProgress {
                            loaded_count: total,
                            total_count: None,
                            status_message: Some("Loading health checks…".to_string()),
                        },
                    });
                }
                Err(e) => {
                    let _ = event_tx.send(Event::ResourceLoadWarning {
                        service: service_type,
                        warning: format!("health checks: {}", crate::error::sdk_error_message(&e)),
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

// ── R53HostedZone ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct R53HostedZone {
    /// Full path ID, e.g. "/hostedzone/Z1234567890ABC"
    pub id: String,
    pub name: String,
    pub private_zone: bool,
    pub record_count: i64,
    pub comment: String,
    /// Always empty — zone tags are fetched lazily as part of the Sharing
    /// bundle (`R53ZoneDetail`) and shown in the Tags section; this
    /// field backs the `Resource::tags()` fallback only.
    pub tags: HashMap<String, String>,
}

impl R53HostedZone {
    pub fn from_sdk(z: &aws_sdk_route53::types::HostedZone) -> Self {
        let id = z.id().to_string();
        let name = z.name().to_string();
        let private_zone = z.config()
            .map(|c| c.private_zone())
            .unwrap_or(false);
        let comment = z.config()
            .and_then(|c| c.comment())
            .unwrap_or("")
            .to_string();
        let record_count = z.resource_record_set_count().unwrap_or(0);

        Self { id, name, private_zone, record_count, comment, tags: HashMap::new() }
    }

    /// Strip /hostedzone/ prefix to get the bare zone ID for display/URLs.
    pub fn bare_id(&self) -> &str {
        self.id.trim_start_matches("/hostedzone/")
    }

    pub fn zone_type(&self) -> &str {
        if self.private_zone { "Private" } else { "Public" }
    }
}

crate::sections! {
    pub enum R53ZoneDetailSection,
    pub static R53_ZONE_SECTIONS = [
        Records "Records" => crate::app::App::trigger_r53_records_load,
        Sharing "Sharing" => crate::app::App::trigger_r53_zone_detail_load,
        Info "Info" => crate::app::App::trigger_r53_zone_detail_load,
        Tags "Tags" => crate::app::App::trigger_r53_zone_detail_load,
    ]
}

impl Resource for R53HostedZone {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&R53_ZONE_SECTIONS)
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws route53 list-resource-record-sets --hosted-zone-id {}",
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
        "R53 Hosted Zone"
    }

    fn state(&self) -> ResourceState {
        ResourceState::stateless()
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        // Both id forms: a jump seeds the list search with its target id and
        // resolves exact-id against the *filtered* rows, and the record →
        // zone and hosted-zone-ARN jumps target the `/hostedzone/…` form
        // `id()` stores — without it here the filter is empty and the jump
        // silently strands.
        format!(
            "{} {} {} {} {}",
            self.name,
            self.bare_id(),
            self.id,
            self.zone_type(),
            self.comment,
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Zone Name".to_string(), self.name.clone()),
            ("Zone ID".to_string(), self.bare_id().to_string()),
            ("Type".to_string(), self.zone_type().to_string()),
            ("Record Count".to_string(), self.record_count.to_string()),
            ("Comment".to_string(), if self.comment.is_empty() { "—".to_string() } else { self.comment.clone() }),
        ]
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn console_url(&self, _region: &str) -> Option<String> {
        // Route53 console is always in us-east-1
        Some(format!(
            "https://us-east-1.console.aws.amazon.com/route53/v2/hostedzones#{}",
            self.bare_id()
        ))
    }
}

// ── R53HealthCheck ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct R53HealthCheck {
    pub id: String,
    /// HTTP / HTTPS / HTTP_STR_MATCH / TCP / CALCULATED / CLOUDWATCH_METRIC / …
    pub check_type: String,
    pub fqdn: Option<String>,
    pub ip_address: Option<String>,
    pub port: Option<i32>,
    pub resource_path: Option<String>,
    pub search_string: Option<String>,
    pub request_interval: Option<i32>,
    pub failure_threshold: Option<i32>,
    pub measure_latency: bool,
    pub inverted: bool,
    pub disabled: bool,
    pub enable_sni: bool,
    /// Health-checker region names (empty ⇒ default global set).
    pub regions: Vec<String>,
    /// Child health check ids (CALCULATED type).
    pub child_health_checks: Vec<String>,
    /// CloudWatch alarm name (CLOUDWATCH_METRIC type).
    pub alarm_name: Option<String>,
    pub tags: HashMap<String, String>,
}

impl R53HealthCheck {
    pub fn from_sdk(hc: &aws_sdk_route53::types::HealthCheck) -> Self {
        let cfg = hc.health_check_config();
        let check_type = cfg
            .map(|c| c.r#type().as_str().to_string())
            .unwrap_or_default();
        Self {
            id: hc.id().to_string(),
            check_type,
            fqdn: cfg.and_then(|c| c.fully_qualified_domain_name()).map(str::to_string),
            ip_address: cfg.and_then(|c| c.ip_address()).map(str::to_string),
            port: cfg.and_then(|c| c.port()),
            resource_path: cfg.and_then(|c| c.resource_path()).map(str::to_string),
            search_string: cfg.and_then(|c| c.search_string()).map(str::to_string),
            request_interval: cfg.and_then(|c| c.request_interval()),
            failure_threshold: cfg.and_then(|c| c.failure_threshold()),
            measure_latency: cfg.and_then(|c| c.measure_latency()).unwrap_or(false),
            inverted: cfg.and_then(|c| c.inverted()).unwrap_or(false),
            disabled: cfg.and_then(|c| c.disabled()).unwrap_or(false),
            enable_sni: cfg.and_then(|c| c.enable_sni()).unwrap_or(false),
            regions: cfg
                .map(|c| c.regions().iter().map(|r| r.as_str().to_string()).collect())
                .unwrap_or_default(),
            child_health_checks: cfg.map(|c| c.child_health_checks().to_vec()).unwrap_or_default(),
            alarm_name: cfg
                .and_then(|c| c.alarm_identifier())
                .map(|a| a.name().to_string()),
            tags: HashMap::new(),
        }
    }

    /// A one-line human summary of what this check targets.
    pub fn target(&self) -> String {
        if !self.child_health_checks.is_empty() {
            return format!("CALCULATED · {} children", self.child_health_checks.len());
        }
        if let Some(a) = &self.alarm_name {
            return format!("CloudWatch alarm · {}", a);
        }
        let host = self
            .fqdn
            .clone()
            .or_else(|| self.ip_address.clone())
            .unwrap_or_else(|| "—".to_string());
        let mut s = host;
        if let Some(p) = self.port {
            s.push_str(&format!(":{}", p));
        }
        if let Some(path) = &self.resource_path {
            s.push_str(path);
        }
        s
    }
}

crate::sections! {
    pub enum R53HealthCheckDetailSection,
    pub static R53_HEALTH_CHECK_SECTIONS = [
        Overview "Overview",
        Status "Status" => crate::app::App::trigger_r53_health_status_load,
        Tags "Tags",
    ]
}

impl Resource for R53HealthCheck {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&R53_HEALTH_CHECK_SECTIONS)
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws route53 get-health-check --health-check-id {}",
            crate::aws::resource::shell_quote(self.id())
        ))
    }

    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        // Prefer the Name tag; else fall back to the id (target is computed).
        self.tags.get("Name").map(|s| s.as_str()).unwrap_or(&self.id)
    }

    fn resource_type(&self) -> &str {
        "R53 Health Check"
    }

    fn state(&self) -> ResourceState {
        // Live status is a lazy fetch; reflect the config's disabled flag here.
        if self.disabled {
            ResourceState::Stopped
        } else {
            ResourceState::Running
        }
    }

    fn state_label(&self) -> String {
        if self.disabled { "disabled" } else { "enabled" }.to_string()
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        let mut parts = vec![self.id.clone(), self.check_type.clone(), self.target()];
        for (k, v) in &self.tags {
            parts.push(format!("{}:{}", k, v));
        }
        parts.join(" ")
    }

    fn details(&self) -> Vec<(String, String)> {
        // Split pane is the primary view; this is the export/flat fallback.
        vec![
            ("Health Check ID".to_string(), self.id.clone()),
            ("Type".to_string(), self.check_type.clone()),
            ("Target".to_string(), self.target()),
        ]
    }

    fn console_url(&self, _region: &str) -> Option<String> {
        Some(format!(
            "https://us-east-1.console.aws.amazon.com/route53/healthchecks/home#/details/{}",
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

// ── R53Record ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct R53Record {
    /// Stable, unique across zones: `<TYPE> <name> [<set-id>] @<zone-id>` —
    /// name + type alone collide in split-horizon setups (a public and a
    /// private zone for the same domain) and inside weighted/latency sets.
    pub id: String,
    /// FQDN without the trailing dot (`api.example.com`), so it compares
    /// equal to the way every other service spells the same host.
    pub name: String,
    pub record_type: String,
    /// Owning zone, `/hostedzone/Z…` (what `R53HostedZone::id()` stores).
    pub zone_id: String,
    /// Owning zone name without the trailing dot.
    pub zone_name: String,
    pub ttl: Option<i64>,
    pub values: Vec<String>,
    pub alias_target: Option<String>,
    /// Alias records only: whether Route 53 follows the target's own health.
    pub evaluate_target_health: bool,
    /// The routing-policy discriminators. Every non-simple policy is identified
    /// by exactly one of these being populated, and all of them require
    /// `set_identifier` — which is the only thing distinguishing two record sets
    /// that share a name and type.
    pub set_identifier: Option<String>,
    pub weight: Option<i64>,
    /// Latency policy: the AWS region the record resolves for.
    pub region: Option<String>,
    /// PRIMARY / SECONDARY.
    pub failover: Option<String>,
    /// Pre-formatted geolocation ("continent EU", "US-CA", "default").
    pub geo_location: Option<String>,
    /// Pre-formatted geoproximity ("region eu-west-1, bias +20").
    pub geo_proximity: Option<String>,
    /// IP-based routing: "collection <id> · <location>".
    pub cidr_routing: Option<String>,
    pub multi_value_answer: Option<bool>,
    pub health_check_id: Option<String>,
    pub traffic_policy_instance_id: Option<String>,
}

impl R53Record {
    pub fn from_sdk(
        r: &aws_sdk_route53::types::ResourceRecordSet,
        zone_id: &str,
        zone_name: &str,
    ) -> Self {
        let values: Vec<String> = r.resource_records()
            .iter()
            .map(|rr| rr.value().to_string())
            .collect();

        let alias_target = r.alias_target()
            .map(|a| a.dns_name().to_string());

        // Geolocation arrives as up to three codes; "*" is Route 53's marker for
        // the catch-all record that serves everywhere else.
        let geo_location = r.geo_location().and_then(|g| {
            if g.country_code() == Some("*") {
                return Some("default (everywhere else)".to_string());
            }
            match (g.continent_code(), g.country_code(), g.subdivision_code()) {
                (Some(c), _, _) => Some(format!("continent {c}")),
                (_, Some(country), Some(sub)) => Some(format!("{country}-{sub}")),
                (_, Some(country), None) => Some(country.to_string()),
                _ => None,
            }
        });

        let geo_proximity = r.geo_proximity_location().and_then(|g| {
            let where_ = match (g.aws_region(), g.local_zone_group(), g.coordinates()) {
                (Some(region), _, _) => format!("region {region}"),
                (_, Some(lz), _) => format!("local zone {lz}"),
                (_, _, Some(c)) => format!("{}, {}", c.latitude(), c.longitude()),
                _ => return None,
            };
            Some(match g.bias() {
                Some(b) if b != 0 => format!("{where_}, bias {b:+}"),
                _ => where_,
            })
        });

        let cidr_routing = r.cidr_routing_config().map(|c| {
            format!("collection {} · {}", c.collection_id(), c.location_name())
        });

        let name = r.name().trim_end_matches('.').to_string();
        let record_type = r.r#type().as_str().to_string();
        let zone_bare = zone_id.trim_start_matches("/hostedzone/");
        let id = match r.set_identifier() {
            Some(sid) => format!("{record_type} {name} [{sid}] @{zone_bare}"),
            None => format!("{record_type} {name} @{zone_bare}"),
        };
        Self {
            id,
            name,
            record_type,
            zone_id: zone_id.to_string(),
            zone_name: zone_name.trim_end_matches('.').to_string(),
            ttl: r.ttl(),
            values,
            alias_target,
            evaluate_target_health: r
                .alias_target()
                .map(|a| a.evaluate_target_health())
                .unwrap_or(false),
            set_identifier: r.set_identifier().map(str::to_string),
            weight: r.weight(),
            region: r.region().map(|x| x.as_str().to_string()),
            failover: r.failover().map(|f| f.as_str().to_string()),
            geo_location,
            geo_proximity,
            cidr_routing,
            multi_value_answer: r.multi_value_answer(),
            health_check_id: r.health_check_id().map(str::to_string),
            traffic_policy_instance_id: r.traffic_policy_instance_id().map(str::to_string),
        }
    }

    /// The record's routing policy, named the way the console names it.
    /// A simple record populates none of the discriminators.
    pub fn routing_policy(&self) -> &'static str {
        if self.weight.is_some() {
            "weighted"
        } else if self.region.is_some() {
            "latency"
        } else if self.failover.is_some() {
            "failover"
        } else if self.geo_location.is_some() {
            "geolocation"
        } else if self.geo_proximity.is_some() {
            "geoproximity"
        } else if self.cidr_routing.is_some() {
            "IP-based"
        } else if self.multi_value_answer == Some(true) {
            "multivalue"
        } else {
            "simple"
        }
    }

    /// One line describing a non-simple record's policy, set identifier and the
    /// policy's own parameter — `None` for a simple record, which has nothing to
    /// say beyond what the table row already shows.
    pub fn routing_summary(&self) -> Option<String> {
        let policy = self.routing_policy();
        if policy == "simple" {
            return None;
        }
        let mut parts = vec![policy.to_string()];
        if let Some(sid) = &self.set_identifier {
            parts.push(format!("set \"{sid}\""));
        }
        let detail = match policy {
            // Weight 0 is meaningful: the record is in the set but never served.
            "weighted" => self.weight.map(|w| {
                if w == 0 {
                    "weight 0 (never served)".to_string()
                } else {
                    format!("weight {w}")
                }
            }),
            "latency" => self.region.as_ref().map(|r| format!("region {r}")),
            "failover" => self.failover.clone(),
            "geolocation" => self.geo_location.clone(),
            "geoproximity" => self.geo_proximity.clone(),
            "IP-based" => self.cidr_routing.clone(),
            _ => None,
        };
        parts.extend(detail);
        Some(parts.join(" · "))
    }

    pub fn zone_bare_id(&self) -> &str {
        self.zone_id.trim_start_matches("/hostedzone/")
    }

    /// What the record answers with, in one cell: the alias target, or the
    /// first value plus a `(+N)` count when there are more.
    pub fn value_summary(&self) -> String {
        if let Some(alias) = &self.alias_target {
            return format!("alias {}", alias.trim_end_matches('.'));
        }
        match self.values.as_slice() {
            [] => "—".to_string(),
            [one] => one.clone(),
            [first, rest @ ..] => format!("{} (+{})", first, rest.len()),
        }
    }

    /// The service a DNS name resolves to, when the hostname says so —
    /// what `⏎` on an alias/CNAME row jumps to and what `U` on the target
    /// matches. Alias targets are AWS-generated hostnames with a stable shape
    /// per service; anything else is `None`.
    pub fn dns_target(host: &str) -> Option<DnsTarget> {
        let host = host.trim_end_matches('.').to_ascii_lowercase();
        if host.ends_with(".cloudfront.net") {
            return Some(DnsTarget::CloudFront(host));
        }
        if host.ends_with(".awsglobalaccelerator.com") {
            return Some(DnsTarget::GlobalAccelerator(host));
        }
        // S3 website endpoints: `s3-website-us-east-1.amazonaws.com` (dash)
        // or `s3-website.eu-west-1.amazonaws.com` (dot); an alias target is
        // the bare endpoint (the bucket is the record name), a CNAME value
        // is `<bucket>.<endpoint>`.
        if host.contains("s3-website") && host.ends_with(".amazonaws.com") {
            let bucket = host
                .split_once(".s3-website")
                .map(|(b, _)| b.to_string())
                .filter(|b| !b.is_empty());
            return Some(DnsTarget::S3Website(bucket));
        }
        // Load balancers: `[dualstack.][internal-]<name>-<suffix>.<region>.elb.amazonaws.com`
        // (classic/ALB, decimal suffix) or `<name>-<hex>.elb.<region>.amazonaws.com`
        // (NLB). The name is the first label minus its generated suffix.
        if host.ends_with(".amazonaws.com") && host.contains(".elb.") {
            let rest = host.strip_prefix("dualstack.").unwrap_or(&host);
            let first = rest.split('.').next().unwrap_or("");
            let first = first.strip_prefix("internal-").unwrap_or(first);
            let name = match first.rsplit_once('-') {
                Some((n, suffix))
                    if !n.is_empty()
                        && suffix.len() >= 8
                        && suffix.chars().all(|c| c.is_ascii_hexdigit()) =>
                {
                    n.to_string()
                }
                _ => first.to_string(),
            };
            if !name.is_empty() {
                return Some(DnsTarget::LoadBalancer(name));
            }
        }
        None
    }

    /// The hostname this record points at, if it points at one: the alias
    /// target, or a CNAME's single value.
    pub fn target_host(&self) -> Option<&str> {
        if let Some(a) = &self.alias_target {
            return Some(a.as_str());
        }
        if self.record_type == "CNAME" {
            return self.values.first().map(String::as_str);
        }
        None
    }
}

/// Where an AWS-generated hostname leads (see `R53Record::dns_target`).
#[derive(Debug, Clone, PartialEq)]
pub enum DnsTarget {
    /// Load balancer *name* (ALB/NLB/classic), recovered from the DNS name.
    LoadBalancer(String),
    /// The `d….cloudfront.net` domain (a distribution's `domain_name`).
    CloudFront(String),
    /// S3 website endpoint; `Some(bucket)` when the hostname carried it
    /// (CNAME form), `None` for the bare alias endpoint (bucket = record name).
    S3Website(Option<String>),
    /// The accelerator's `….awsglobalaccelerator.com` DNS name.
    GlobalAccelerator(String),
}

impl Resource for R53Record {
    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn resource_type(&self) -> &str {
        "R53 Record"
    }

    /// The record type doubles as the state so `F` cycles A / CNAME / TXT …
    /// and `z`'s state sort groups by type.
    fn state(&self) -> ResourceState {
        ResourceState::Unknown(self.record_type.clone())
    }

    fn tags(&self) -> &HashMap<String, String> {
        static EMPTY: std::sync::OnceLock<HashMap<String, String>> = std::sync::OnceLock::new();
        EMPTY.get_or_init(HashMap::new)
    }

    fn search_text(&self) -> String {
        let mut parts = vec![
            self.name.clone(),
            self.record_type.clone(),
            self.zone_name.clone(),
            self.zone_bare_id().to_string(),
            self.routing_policy().to_string(),
        ];
        parts.extend(self.values.iter().cloned());
        parts.extend(self.alias_target.clone());
        parts.extend(self.set_identifier.clone());
        parts.extend(self.health_check_id.clone());
        parts.join(" ")
    }

    fn details(&self) -> Vec<(String, String)> {
        let mut rows = vec![
            ("Name".to_string(), self.name.clone()),
            ("Type".to_string(), self.record_type.clone()),
            ("Hosted Zone".to_string(), self.zone_name.clone()),
            ("Zone ID".to_string(), self.zone_bare_id().to_string()),
        ];
        match self.ttl {
            Some(t) => rows.push(("TTL".to_string(), format!("{t}s"))),
            None if self.alias_target.is_some() => {
                rows.push(("TTL".to_string(), "alias (follows target)".to_string()))
            }
            None => {}
        }
        rows.push(("Routing Policy".to_string(), self.routing_policy().to_string()));
        if let Some(sid) = &self.set_identifier {
            rows.push(("Set Identifier".to_string(), sid.clone()));
        }
        if let Some(w) = self.weight {
            let v = if w == 0 { "0 (never served)".to_string() } else { w.to_string() };
            rows.push(("Weight".to_string(), v));
        }
        if let Some(r) = &self.region {
            rows.push(("Latency Region".to_string(), r.clone()));
        }
        if let Some(f) = &self.failover {
            rows.push(("Failover".to_string(), f.clone()));
        }
        if let Some(g) = &self.geo_location {
            rows.push(("Geolocation".to_string(), g.clone()));
        }
        if let Some(g) = &self.geo_proximity {
            rows.push(("Geoproximity".to_string(), g.clone()));
        }
        if let Some(c) = &self.cidr_routing {
            rows.push(("IP-based Routing".to_string(), c.clone()));
        }
        if self.multi_value_answer == Some(true) {
            rows.push(("Multivalue Answer".to_string(), "✓".to_string()));
        }
        if let Some(hc) = &self.health_check_id {
            rows.push(("Health Check".to_string(), hc.clone()));
        }
        if let Some(tp) = &self.traffic_policy_instance_id {
            rows.push(("Traffic Policy Instance".to_string(), tp.clone()));
        }
        if let Some(alias) = &self.alias_target {
            rows.push(("Alias Target".to_string(), alias.trim_end_matches('.').to_string()));
            rows.push((
                "Evaluate Target Health".to_string(),
                if self.evaluate_target_health { "✓".to_string() } else { "✗".to_string() },
            ));
        }
        if !self.values.is_empty() {
            rows.push(("".to_string(), "".to_string()));
            rows.push((format!("Values ({})", self.values.len()), "".to_string()));
            for v in &self.values {
                rows.push((format!("  {}", v), "".to_string()));
            }
        }
        rows
    }

    /// Everything the record points at, for the `U` lens: the raw hostnames
    /// and values (a distribution's `domain_name`, a bucket's endpoint), the
    /// *derived* load balancer name (its DNS name embeds the name with a
    /// generated suffix, which whole-token matching can't see through), the
    /// health check, the zone — and the record's own name, so `U` on a
    /// CloudFront distribution whose alias *is* this name lists it.
    fn references(&self) -> Vec<(String, String)> {
        let mut v: Vec<(String, String)> = vec![("Name".to_string(), self.name.clone())];
        if let Some(a) = &self.alias_target {
            v.push(("Alias Target".to_string(), a.trim_end_matches('.').to_string()));
        }
        for val in &self.values {
            v.push(("Value".to_string(), val.trim_end_matches('.').to_string()));
        }
        if let Some(host) = self.target_host() {
            match Self::dns_target(host) {
                Some(DnsTarget::LoadBalancer(name)) => {
                    v.push(("Load Balancer".to_string(), name));
                }
                Some(DnsTarget::S3Website(bucket)) => {
                    v.push(("S3 Bucket".to_string(), bucket.unwrap_or_else(|| self.name.clone())));
                }
                _ => {}
            }
        }
        if let Some(hc) = &self.health_check_id {
            v.push(("Health Check".to_string(), hc.clone()));
        }
        v.push(("Hosted Zone".to_string(), self.zone_id.clone()));
        v
    }

    /// Changes to a record are logged against its *zone* (`ChangeResourceRecordSets`
    /// names the hosted zone), so the timeline looks the zone up.
    fn trail_lookup_keys(&self) -> Vec<String> {
        vec![self.zone_bare_id().to_string(), self.zone_id.clone()]
    }

    fn cli_command(&self) -> Option<String> {
        use crate::aws::resource::shell_quote;
        let mut cmd = format!(
            "aws route53 list-resource-record-sets --hosted-zone-id {} --start-record-name {} --start-record-type {} --max-items 1",
            shell_quote(self.zone_bare_id()),
            shell_quote(&self.name),
            shell_quote(&self.record_type)
        );
        if let Some(sid) = &self.set_identifier {
            cmd.push_str(&format!(" --start-record-identifier {}", shell_quote(sid)));
        }
        Some(cmd)
    }

    fn console_url(&self, _region: &str) -> Option<String> {
        // No per-record deep link exists; the zone's record list is the
        // closest the console offers.
        Some(format!(
            "https://us-east-1.console.aws.amazon.com/route53/v2/hostedzones#ListRecordSets/{}",
            self.zone_bare_id()
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Every record set in one zone (`ListResourceRecordSets`, no SDK paginator).
/// Shared by the zone pane's Records section and the cross-zone Records
/// sub-tab, through the same `lazy.r53_zone_records` map, so neither fetches
/// what the other already has.
///
/// The identifier marker is not optional: when a page boundary falls inside
/// a set of weighted/latency records (which all share one name and type),
/// name+type alone restart at the top of that set and the loop never
/// advances. A first-page failure is a real error; a mid-pagination failure
/// keeps the partial list.
pub async fn fetch_zone_records(
    client: &R53Client,
    zone_id: &str,
    zone_name: &str,
) -> std::result::Result<Vec<R53Record>, String> {
    let mut records = Vec::new();
    let mut next_name: Option<String> = None;
    let mut next_type: Option<String> = None;
    let mut next_ident: Option<String> = None;
    loop {
        let mut req = client.list_resource_record_sets().hosted_zone_id(zone_id);
        if let Some(name) = &next_name {
            req = req.start_record_name(name);
        }
        if let Some(rtype) = &next_type {
            req = req.start_record_type(aws_sdk_route53::types::RrType::from(rtype.as_str()));
        }
        if let Some(ident) = &next_ident {
            req = req.start_record_identifier(ident);
        }
        match req.send().await {
            Ok(resp) => {
                for r in resp.resource_record_sets() {
                    records.push(R53Record::from_sdk(r, zone_id, zone_name));
                }
                if resp.is_truncated() {
                    let name = resp.next_record_name().map(|s| s.to_string());
                    let rtype = resp.next_record_type().map(|t| t.as_str().to_string());
                    let ident = resp.next_record_identifier().map(|s| s.to_string());
                    // Belt-and-braces against a non-advancing marker set: the
                    // same trio twice means another page would repeat forever.
                    if name == next_name && rtype == next_type && ident == next_ident {
                        break;
                    }
                    next_name = name;
                    next_type = rtype;
                    next_ident = ident;
                } else {
                    break;
                }
            }
            Err(e) if records.is_empty() => {
                return Err(crate::error::sdk_error_message(&e));
            }
            Err(_) => break,
        }
    }
    Ok(records)
}

// ── Zone detail (lazy): sharing + name servers + DNSSEC + query logging + tags ─

/// One VPC entry in a private hosted zone's sharing picture — either a live
/// association (`GetHostedZone`) or a pending cross-account authorization
/// (`ListVPCAssociationAuthorizations`). Route 53 never reports which account
/// owns the VPC, only its id + region, so cross-account rows can't be labeled;
/// same-region ids are still `Enter`-jumpable via the generic `vpc-` classifier.
#[derive(Debug, Clone)]
pub struct R53ZoneVpc {
    pub vpc_id: String,
    pub vpc_region: String,
}

#[derive(Debug, Clone)]
pub struct R53ZoneDetail {
    /// VPCs the zone actually resolves in right now (private zones).
    pub associated: Vec<R53ZoneVpc>,
    /// VPCs authorized to associate but not (yet, or no longer) actually
    /// associated — AWS never auto-deletes the authorization once used, so a
    /// VPC can show up here *and* in `associated` at the same time (a dangling
    /// authorization worth flagging, not an error).
    pub authorized: Vec<R53ZoneVpc>,
    pub tags: Vec<(String, String)>,
    /// The delegation set's name servers (public zones; private zones have
    /// none) — what the registrar / parent zone must delegate to.
    pub name_servers: Vec<String>,
    /// DNSSEC signing status (`SIGNING`, `NOT_SIGNING`, `ACTION_NEEDED`, …)
    /// with its status message; `None` when the call failed or the zone is
    /// private (DNSSEC applies to public zones only).
    pub dnssec: Option<R53Dnssec>,
    /// Query logging destination: `(log group name, region)` parsed from the
    /// config's log-group ARN. `None` = no query logging config (or the call
    /// failed — best effort).
    pub query_log_group: Option<(String, String)>,
    /// `MAX_RRSETS_BY_ZONE`: the record ceiling this zone is measured against.
    pub record_limit: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct R53Dnssec {
    pub status: String,
    pub status_message: Option<String>,
    /// Key-signing keys: (name, status, signing algorithm).
    pub keys: Vec<(String, String, String)>,
}

/// Parse a CloudWatch Logs log-group ARN
/// (`arn:aws:logs:us-east-1:123:log-group:/aws/route53/example.com:*`) into
/// `(name, region)`. Route 53 query logs always land in us-east-1, but the
/// ARN says so itself, so read it rather than assume.
pub fn parse_log_group_arn(arn: &str) -> Option<(String, String)> {
    let mut parts = arn.splitn(6, ':');
    let (_arn, _aws, svc, region, _acct, rest) = (
        parts.next()?,
        parts.next()?,
        parts.next()?,
        parts.next()?,
        parts.next()?,
        parts.next()?,
    );
    if svc != "logs" {
        return None;
    }
    let name = rest.strip_prefix("log-group:")?;
    let name = name.strip_suffix(":*").unwrap_or(name);
    if name.is_empty() || region.is_empty() {
        return None;
    }
    Some((name.to_string(), region.to_string()))
}

/// Fetch a hosted zone's detail bundle: `GetHostedZone` (name servers, and
/// VPC associations for private zones), `ListVPCAssociationAuthorizations`
/// (private zones — pending cross-account authorizations), `GetDNSSEC`
/// (public zones), `ListQueryLoggingConfigs`, `GetHostedZoneLimit`, and
/// tags (`ListTagsForResource`). Bundled into one fetch, like the Resolver
/// endpoint/rule details, so visiting Sharing, Info or Tags warms all three.
/// Only the sharing calls are fatal (they were before); the Info extras are
/// best-effort and degrade to "unknown" rows.
pub async fn fetch_zone_detail(
    client: R53Client,
    zone_id: String,
    private_zone: bool,
) -> Result<R53ZoneDetail> {
    let tags = fetch_zone_tags(&client, &zone_id).await;

    let get_resp = client
        .get_hosted_zone()
        .id(&zone_id)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
    let name_servers: Vec<String> = get_resp
        .delegation_set()
        .map(|d| d.name_servers().iter().map(|s| s.to_string()).collect())
        .unwrap_or_default();

    let query_log_group = client
        .list_query_logging_configs()
        .hosted_zone_id(&zone_id)
        .send()
        .await
        .ok()
        .and_then(|resp| {
            resp.query_logging_configs()
                .first()
                .and_then(|c| parse_log_group_arn(c.cloud_watch_logs_log_group_arn()))
        });

    let record_limit = client
        .get_hosted_zone_limit()
        .hosted_zone_id(&zone_id)
        .r#type(aws_sdk_route53::types::HostedZoneLimitType::MaxRrsetsByZone)
        .send()
        .await
        .ok()
        .and_then(|resp| resp.limit().map(|l| l.value()));

    if !private_zone {
        let dnssec = client.get_dnssec().hosted_zone_id(&zone_id).send().await.ok().map(|resp| {
            let (status, status_message) = resp
                .status()
                .map(|st| {
                    (
                        st.serve_signature().unwrap_or("UNKNOWN").to_string(),
                        st.status_message().map(str::to_string),
                    )
                })
                .unwrap_or_else(|| ("UNKNOWN".to_string(), None));
            let keys = resp
                .key_signing_keys()
                .iter()
                .map(|k| {
                    (
                        k.name().unwrap_or("—").to_string(),
                        k.status().unwrap_or("—").to_string(),
                        k.signing_algorithm_mnemonic().unwrap_or("—").to_string(),
                    )
                })
                .collect();
            R53Dnssec { status, status_message, keys }
        });
        return Ok(R53ZoneDetail {
            associated: Vec::new(),
            authorized: Vec::new(),
            tags,
            name_servers,
            dnssec,
            query_log_group,
            record_limit,
        });
    }

    let associated = get_resp
        .vpcs()
        .iter()
        .map(|v| R53ZoneVpc {
            vpc_id: v.vpc_id().unwrap_or_default().to_string(),
            vpc_region: v
                .vpc_region()
                .map(|r| r.as_str().to_string())
                .unwrap_or_default(),
        })
        .collect();

    // No fluent paginator for this op — hand-rolled token loop.
    let mut authorized = Vec::new();
    let mut next: Option<String> = None;
    loop {
        let mut req = client
            .list_vpc_association_authorizations()
            .hosted_zone_id(&zone_id);
        if let Some(t) = &next {
            req = req.next_token(t);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
        for v in resp.vpcs() {
            authorized.push(R53ZoneVpc {
                vpc_id: v.vpc_id().unwrap_or_default().to_string(),
                vpc_region: v
                    .vpc_region()
                    .map(|r| r.as_str().to_string())
                    .unwrap_or_default(),
            });
        }
        next = crate::aws::pagination::next_page_token(resp.next_token(), &next);
        if next.is_none() {
            break;
        }
    }

    Ok(R53ZoneDetail {
        associated,
        authorized,
        tags,
        name_servers,
        dnssec: None,
        query_log_group,
        record_limit,
    })
}

async fn fetch_zone_tags(client: &R53Client, zone_id: &str) -> Vec<(String, String)> {
    match client
        .list_tags_for_resource()
        .resource_type(aws_sdk_route53::types::TagResourceType::Hostedzone)
        .resource_id(zone_id)
        .send()
        .await
    {
        Ok(resp) => resp
            .resource_tag_set()
            .map(|set| {
                set.tags()
                    .iter()
                    .filter_map(|t| Some((t.key()?.to_string(), t.value()?.to_string())))
                    .collect()
            })
            .unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

// ── Health-check live status (lazy, `GetHealthCheckStatus`) ───────────────────

/// One health checker's observation: (region, status text, checked time).
#[derive(Debug, Clone)]
pub struct R53HealthObservation {
    pub region: String,
    pub status: String,
    pub checked_time: Option<String>,
}

/// Fetch the per-region observations for one health check (`GetHealthCheckStatus`).
pub async fn fetch_health_check_status(
    client: R53Client,
    id: String,
) -> Result<Vec<R53HealthObservation>> {
    let resp = client.get_health_check_status().health_check_id(&id).send().await?;
    let obs = resp
        .health_check_observations()
        .iter()
        .map(|o| {
            let report = o.status_report();
            R53HealthObservation {
                region: o
                    .region()
                    .map(|r| r.as_str().to_string())
                    .unwrap_or_default(),
                status: report
                    .and_then(|r| r.status())
                    .unwrap_or("—")
                    .to_string(),
                checked_time: report.and_then(|r| r.checked_time()).map(|t| t.to_string()),
            }
        })
        .collect();
    Ok(obs)
}

// ── Health-check CloudWatch metrics (`m`) — AWS/Route53, dim HealthCheckId ─────
//
// Route53 is a global namespace, so the caller must pass a us-east-1 CloudWatch
// client (like CloudFront). HealthCheckStatus is 1/0 (healthy/unhealthy);
// HealthCheckPercentageHealthy is the % of checkers reporting healthy.

pub use crate::aws::services::ec2::MetricsTimeRange;

#[derive(Debug, Clone)]
pub struct R53HealthMetricsData {
    pub time_range: MetricsTimeRange,
    pub status: Vec<(f64, f64)>,
    pub percent_healthy: Vec<(f64, f64)>,
    pub connection_time: Vec<(f64, f64)>,
    pub time_to_first_byte: Vec<(f64, f64)>,
    pub x_max: f64,
}

#[derive(Debug, Clone)]
pub enum R53HealthMetricsState {
    Loading,
    Loaded(R53HealthMetricsData),
}

pub async fn fetch_health_check_metrics(
    cw: aws_sdk_cloudwatch::Client,
    health_check_id: String,
    time_range: MetricsTimeRange,
) -> Result<R53HealthMetricsData> {
    use aws_sdk_cloudwatch::types::{Dimension, Statistic};

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let start = now - time_range.duration_secs();
    let period = time_range.period_secs();
    let start_dt = aws_sdk_cloudwatch::primitives::DateTime::from_secs(start);
    let end_dt = aws_sdk_cloudwatch::primitives::DateTime::from_secs(now);

    let dim = || {
        Dimension::builder()
            .name("HealthCheckId")
            .value(&health_check_id)
            .build()
    };
    let metric = |name: &'static str, stat: Statistic| {
        cw.get_metric_statistics()
            .namespace("AWS/Route53")
            .metric_name(name)
            .dimensions(dim())
            .start_time(start_dt)
            .end_time(end_dt)
            .period(period)
            .set_statistics(Some(vec![stat]))
            .send()
    };

    let (status, percent, conn, ttfb) = tokio::join!(
        metric("HealthCheckStatus", Statistic::Minimum),
        metric("HealthCheckPercentageHealthy", Statistic::Average),
        metric("ConnectionTime", Statistic::Average),
        metric("TimeToFirstByte", Statistic::Average),
    );

    let parse = |resp: std::result::Result<
        aws_sdk_cloudwatch::operation::get_metric_statistics::GetMetricStatisticsOutput,
        _,
    >,
                 pick: fn(&aws_sdk_cloudwatch::types::Datapoint) -> Option<f64>|
     -> Vec<(f64, f64)> {
        let mut pts: Vec<(f64, f64)> = match resp {
            Ok(r) => r
                .datapoints()
                .iter()
                .filter_map(|dp| Some((dp.timestamp()?.secs() as f64 - start as f64, pick(dp)?)))
                .collect(),
            Err(_) => vec![],
        };
        pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        pts
    };

    Ok(R53HealthMetricsData {
        time_range,
        status: parse(status, |dp| dp.minimum()),
        percent_healthy: parse(percent, |dp| dp.average()),
        connection_time: parse(conn, |dp| dp.average()),
        time_to_first_byte: parse(ttfb, |dp| dp.average()),
        x_max: time_range.duration_secs() as f64,
    })
}

// ── Hosted-zone CloudWatch metrics (`m`) — AWS/Route53, dim HostedZoneId ──────
//
// Same global namespace as health checks (us-east-1 client). DNSQueries is
// only published for **public** hosted zones — callers gate on `private_zone`
// and pass the bare zone id (no "/hostedzone/" prefix).

#[derive(Debug, Clone)]
pub struct R53ZoneMetricsData {
    pub time_range: MetricsTimeRange,
    pub queries: Vec<(f64, f64)>,
    pub x_max: f64,
}

#[derive(Debug, Clone)]
pub enum R53ZoneMetricsState {
    Loading,
    Loaded(R53ZoneMetricsData),
}

pub async fn fetch_zone_metrics(
    cw: aws_sdk_cloudwatch::Client,
    zone_id: String,
    time_range: MetricsTimeRange,
) -> Result<R53ZoneMetricsData> {
    use aws_sdk_cloudwatch::types::{Dimension, Statistic};

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let start = now - time_range.duration_secs();
    let period = time_range.period_secs();

    let queries = cw
        .get_metric_statistics()
        .namespace("AWS/Route53")
        .metric_name("DNSQueries")
        .dimensions(Dimension::builder().name("HostedZoneId").value(&zone_id).build())
        .start_time(aws_sdk_cloudwatch::primitives::DateTime::from_secs(start))
        .end_time(aws_sdk_cloudwatch::primitives::DateTime::from_secs(now))
        .period(period)
        .set_statistics(Some(vec![Statistic::Sum]))
        .send()
        .await;

    Ok(R53ZoneMetricsData {
        time_range,
        queries: crate::aws::services::ec2::parse_metric_datapoints(queries, start),
        x_max: time_range.duration_secs() as f64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dns_target_recognises_aws_generated_hostnames() {
        assert_eq!(
            R53Record::dns_target("dualstack.my-alb-1234567890.eu-west-1.elb.amazonaws.com."),
            Some(DnsTarget::LoadBalancer("my-alb".into()))
        );
        assert_eq!(
            R53Record::dns_target("internal-api-prod-987654321.us-east-1.elb.amazonaws.com"),
            Some(DnsTarget::LoadBalancer("api-prod".into()))
        );
        assert_eq!(
            R53Record::dns_target("edge-nlb-0a1b2c3d4e5f6a7b.elb.eu-west-1.amazonaws.com"),
            Some(DnsTarget::LoadBalancer("edge-nlb".into()))
        );
        assert_eq!(
            R53Record::dns_target("d111111abcdef8.cloudfront.net"),
            Some(DnsTarget::CloudFront("d111111abcdef8.cloudfront.net".into()))
        );
        assert_eq!(
            R53Record::dns_target("s3-website-us-east-1.amazonaws.com."),
            Some(DnsTarget::S3Website(None))
        );
        assert_eq!(
            R53Record::dns_target("www.example.com.s3-website.eu-west-1.amazonaws.com"),
            Some(DnsTarget::S3Website(Some("www.example.com".into())))
        );
        assert_eq!(R53Record::dns_target("mail.example.com"), None);
    }

    #[test]
    fn log_group_arn_parses_name_and_region() {
        assert_eq!(
            parse_log_group_arn("arn:aws:logs:us-east-1:123456789012:log-group:/aws/route53/example.com:*"),
            Some(("/aws/route53/example.com".into(), "us-east-1".into()))
        );
        assert_eq!(parse_log_group_arn("arn:aws:s3:::bucket"), None);
    }

    #[test]
    fn record_id_is_unique_per_zone_and_set() {
        use aws_sdk_route53::types::{ResourceRecord, ResourceRecordSet, RrType};
        let rr = ResourceRecordSet::builder()
            .name("api.example.com.")
            .r#type(RrType::A)
            .set_identifier("blue")
            .weight(10)
            .resource_records(ResourceRecord::builder().value("10.0.0.1").build().unwrap())
            .build()
            .unwrap();
        let rec = R53Record::from_sdk(&rr, "/hostedzone/Z123", "example.com.");
        assert_eq!(rec.id, "A api.example.com [blue] @Z123");
        assert_eq!(rec.name, "api.example.com");
        assert_eq!(rec.zone_name, "example.com");
        assert_eq!(rec.value_summary(), "10.0.0.1");
        assert!(rec.references().iter().any(|(l, v)| l == "Hosted Zone" && v == "/hostedzone/Z123"));
    }
}
