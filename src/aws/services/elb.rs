use crate::aws::client::AwsClients;
use crate::aws::resource::sg_refs;
use crate::aws::resource::{native_state_label, Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use aws_sdk_elasticloadbalancingv2::Client as ElbClient;
use std::any::Any;
use std::collections::HashMap;
use tokio::sync::mpsc;

pub struct ElbService {
    client: ElbClient,
}

impl ElbService {
    pub fn new(aws_clients: &AwsClients) -> Self {
        Self {
            client: aws_clients.elb_client(),
        }
    }
}

/// Fetch tags for up to 20 ARNs in a single `describe_tags` call and merge them
/// into a `arn -> tags` map. ELB does not return tags inline on
/// describe_load_balancers / describe_target_groups.
async fn fetch_tags(client: &ElbClient, arns: &[String]) -> HashMap<String, HashMap<String, String>> {
    let mut out: HashMap<String, HashMap<String, String>> = HashMap::new();
    for chunk in arns.chunks(20) {
        let resp = client
            .describe_tags()
            .set_resource_arns(Some(chunk.to_vec()))
            .send()
            .await;
        if let Ok(resp) = resp {
            for td in resp.tag_descriptions() {
                if let Some(arn) = td.resource_arn() {
                    let tags: HashMap<String, String> = td
                        .tags()
                        .iter()
                        .filter_map(|t| Some((t.key()?.to_string(), t.value()?.to_string())))
                        .collect();
                    out.insert(arn.to_string(), tags);
                }
            }
        }
    }
    out
}

#[async_trait]
impl AwsService for ElbService {
    fn service_type(&self) -> ServiceType {
        ServiceType::Elb
    }

    fn name(&self) -> &str {
        "ELB"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        self.list_resources_streaming(tx, ServiceType::Elb).await?;
        Ok(vec![])
    }

    async fn list_resources_streaming(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        let mut total = 0usize;

        // ── Load balancers ──────────────────────────────────────────────
        let mut lb_paginator = self.client.describe_load_balancers().into_paginator().send();
        while let Some(result) = lb_paginator.next().await {
            match result {
                Ok(page) => {
                    let mut lbs: Vec<LoadBalancer> =
                        page.load_balancers().iter().map(LoadBalancer::from_sdk).collect();
                    if lbs.is_empty() {
                        continue;
                    }
                    let arns: Vec<String> = lbs.iter().map(|l| l.arn.clone()).collect();
                    let tag_map = fetch_tags(&self.client, &arns).await;
                    for lb in &mut lbs {
                        if let Some(tags) = tag_map.get(&lb.arn) {
                            lb.tags = tags.clone();
                        }
                    }
                    let batch: Vec<Box<dyn Resource>> =
                        lbs.into_iter().map(|l| Box::new(l) as Box<dyn Resource>).collect();
                    let count = batch.len();
                    total += count;
                    let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                        service: service_type,
                        resources: batch,
                        progress: LoadProgress {
                            loaded_count: total,
                            total_count: None,
                            status_message: Some("Loading load balancers…".to_string()),
                        },
                    });
                }
                Err(e) => {
                    let _ = event_tx.send(Event::ResourceLoadError {
                        service: service_type,
                        error: format!("Failed to list load balancers: {}", e),
                    });
                    return Ok(());
                }
            }
        }

        // ── Target groups ───────────────────────────────────────────────
        let mut tg_paginator = self.client.describe_target_groups().into_paginator().send();
        while let Some(result) = tg_paginator.next().await {
            match result {
                Ok(page) => {
                    let mut tgs: Vec<TargetGroup> =
                        page.target_groups().iter().map(TargetGroup::from_sdk).collect();
                    if tgs.is_empty() {
                        continue;
                    }
                    let arns: Vec<String> = tgs.iter().map(|t| t.arn.clone()).collect();
                    let tag_map = fetch_tags(&self.client, &arns).await;
                    for tg in &mut tgs {
                        if let Some(tags) = tag_map.get(&tg.arn) {
                            tg.tags = tags.clone();
                        }
                    }
                    let batch: Vec<Box<dyn Resource>> =
                        tgs.into_iter().map(|t| Box::new(t) as Box<dyn Resource>).collect();
                    let count = batch.len();
                    total += count;
                    let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                        service: service_type,
                        resources: batch,
                        progress: LoadProgress {
                            loaded_count: total,
                            total_count: None,
                            status_message: Some("Loading target groups…".to_string()),
                        },
                    });
                }
                Err(e) => {
                    let _ = event_tx.send(Event::ResourceLoadWarning {
                        service: service_type,
                        warning: format!("target groups: {}", crate::error::sdk_error_message(&e)),
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

// ── LoadBalancer ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct LoadBalancer {
    pub arn: String,
    pub name: String,
    pub dns_name: String,
    pub lb_type: String,
    pub scheme: String,
    pub state_code: String,
    pub state_reason: Option<String>,
    pub vpc_id: Option<String>,
    pub ip_address_type: Option<String>,
    pub availability_zones: Vec<String>,
    pub security_groups: Vec<String>,
    pub created_time: String,
    pub tags: HashMap<String, String>,
}

impl LoadBalancer {
    pub fn from_sdk(lb: &aws_sdk_elasticloadbalancingv2::types::LoadBalancer) -> Self {
        let availability_zones: Vec<String> = lb
            .availability_zones()
            .iter()
            .filter_map(|az| az.zone_name().map(|s| s.to_string()))
            .collect();

        let (state_code, state_reason) = match lb.state() {
            Some(s) => (
                s.code().map(|c| c.as_str().to_string()).unwrap_or_default(),
                s.reason().map(|r| r.to_string()),
            ),
            None => (String::new(), None),
        };

        Self {
            arn: lb.load_balancer_arn().unwrap_or("").to_string(),
            name: lb.load_balancer_name().unwrap_or("").to_string(),
            dns_name: lb.dns_name().unwrap_or("").to_string(),
            lb_type: lb.r#type().map(|t| t.as_str().to_string()).unwrap_or_default(),
            scheme: lb.scheme().map(|s| s.as_str().to_string()).unwrap_or_default(),
            state_code,
            state_reason,
            vpc_id: lb.vpc_id().map(|s| s.to_string()),
            ip_address_type: lb.ip_address_type().map(|t| t.as_str().to_string()),
            availability_zones,
            security_groups: lb.security_groups().to_vec(),
            created_time: lb
                .created_time()
                .map(|t| {
                    let s = t.to_string();
                    s.split('.').next().unwrap_or(&s).replace('T', " ")
                })
                .unwrap_or_default(),
            tags: HashMap::new(),
        }
    }
}

crate::sections! {
    pub enum LoadBalancerDetailSection,
    pub static LOAD_BALANCER_SECTIONS = [
        Details "Details",
        Listeners "Listeners" => crate::app::App::trigger_load_balancer_details_load,
        Attributes "Attributes" => crate::app::App::trigger_load_balancer_details_load,
        Tags "Tags",
    ]
}

impl Resource for LoadBalancer {
    fn references(&self) -> Vec<(String, String)> {
        let mut v = sg_refs(self.security_group_ids());
        if let Some(x) = &self.vpc_id { v.push(("VPC".to_string(), x.clone())); }
        v
    }

    fn security_group_ids(&self) -> Vec<String> {
        self.security_groups.clone()
    }

    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&LOAD_BALANCER_SECTIONS)
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws elbv2 describe-load-balancers --load-balancer-arns {}",
            crate::aws::resource::shell_quote(self.id())
        ))
    }

    fn id(&self) -> &str {
        &self.arn
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn resource_type(&self) -> &str {
        "Load Balancer"
    }

    fn state(&self) -> ResourceState {
        match self.state_code.as_str() {
            "active" => ResourceState::Running,
            "provisioning" => ResourceState::Pending,
            "failed" => ResourceState::Unavailable,
            _ => ResourceState::Unknown(self.state_code.clone()),
        }
    }

    fn state_label(&self) -> String {
        native_state_label(&self.state_code, || self.state())
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!(
            "{} {} {} {}",
            self.name, self.lb_type, self.scheme, self.dns_name
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        let mut rows = vec![
            ("Name".to_string(), self.name.clone()),
            ("Type".to_string(), self.lb_type.clone()),
            ("Scheme".to_string(), self.scheme.clone()),
            ("State".to_string(), self.state_code.clone()),
            ("DNS Name".to_string(), self.dns_name.clone()),
        ];
        if let Some(vpc) = &self.vpc_id {
            rows.push(("VPC".to_string(), vpc.clone()));
        }
        if let Some(ip) = &self.ip_address_type {
            rows.push(("IP Address Type".to_string(), ip.clone()));
        }
        if !self.availability_zones.is_empty() {
            rows.push((
                "Availability Zones".to_string(),
                self.availability_zones.join(", "),
            ));
        }
        if !self.security_groups.is_empty() {
            rows.push((
                "Security Groups".to_string(),
                self.security_groups.join(", "),
            ));
        }
        if !self.created_time.is_empty() {
            rows.push(("Created".to_string(), self.created_time.clone()));
        }
        if let Some(reason) = &self.state_reason {
            rows.push(("State Reason".to_string(), reason.clone()));
        }
        rows
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/ec2/home?region={}#LoadBalancers:",
            region, region
        ))
    }
}

// ── TargetGroup ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct TargetGroup {
    pub arn: String,
    pub name: String,
    pub protocol: String,
    pub port: Option<i32>,
    pub target_type: String,
    pub vpc_id: Option<String>,
    pub protocol_version: Option<String>,
    pub load_balancer_arns: Vec<String>,
    pub health_check_enabled: bool,
    pub health_check_protocol: String,
    pub health_check_port: Option<String>,
    pub health_check_path: Option<String>,
    pub health_check_interval_secs: Option<i32>,
    pub health_check_timeout_secs: Option<i32>,
    pub healthy_threshold: Option<i32>,
    pub unhealthy_threshold: Option<i32>,
    pub matcher: Option<String>,
    pub tags: HashMap<String, String>,
}

impl TargetGroup {
    pub fn from_sdk(tg: &aws_sdk_elasticloadbalancingv2::types::TargetGroup) -> Self {
        let matcher = tg.matcher().and_then(|m| {
            m.http_code()
                .map(|c| c.to_string())
                .or_else(|| m.grpc_code().map(|c| c.to_string()))
        });

        Self {
            arn: tg.target_group_arn().unwrap_or("").to_string(),
            name: tg.target_group_name().unwrap_or("").to_string(),
            protocol: tg.protocol().map(|p| p.as_str().to_string()).unwrap_or_default(),
            port: tg.port(),
            target_type: tg.target_type().map(|t| t.as_str().to_string()).unwrap_or_default(),
            vpc_id: tg.vpc_id().map(|s| s.to_string()),
            protocol_version: tg.protocol_version().map(|s| s.to_string()),
            load_balancer_arns: tg.load_balancer_arns().to_vec(),
            health_check_enabled: tg.health_check_enabled().unwrap_or(false),
            health_check_protocol: tg
                .health_check_protocol()
                .map(|p| p.as_str().to_string())
                .unwrap_or_default(),
            health_check_port: tg.health_check_port().map(|s| s.to_string()),
            health_check_path: tg.health_check_path().map(|s| s.to_string()),
            health_check_interval_secs: tg.health_check_interval_seconds(),
            health_check_timeout_secs: tg.health_check_timeout_seconds(),
            healthy_threshold: tg.healthy_threshold_count(),
            unhealthy_threshold: tg.unhealthy_threshold_count(),
            matcher,
            tags: HashMap::new(),
        }
    }

    pub fn target_display(&self) -> String {
        match self.port {
            Some(p) => format!("{}:{}", self.protocol, p),
            None => self.protocol.clone(),
        }
    }
}

crate::sections! {
    pub enum TargetGroupDetailSection,
    pub static TARGET_GROUP_SECTIONS = [
        Health "Health" => crate::app::App::trigger_target_health_load,
        Config "Config",
        Attributes "Attributes" => crate::app::App::trigger_target_group_attributes_load,
        Tags "Tags",
    ]
}

impl Resource for TargetGroup {
    fn references(&self) -> Vec<(String, String)> {
        let mut v = Vec::new();
        if let Some(x) = &self.vpc_id { v.push(("VPC".to_string(), x.clone())); }
        for x in &self.load_balancer_arns { v.push(("Load Balancer".to_string(), x.clone())); }
        v
    }

    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&TARGET_GROUP_SECTIONS)
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws elbv2 describe-target-health --target-group-arn {}",
            crate::aws::resource::shell_quote(self.id())
        ))
    }

    fn id(&self) -> &str {
        &self.arn
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn resource_type(&self) -> &str {
        "Target Group"
    }

    fn state(&self) -> ResourceState {
        ResourceState::stateless()
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!("{} {} {}", self.name, self.protocol, self.target_type)
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Name".to_string(), self.name.clone()),
            ("Protocol".to_string(), self.target_display()),
            ("Target Type".to_string(), self.target_type.clone()),
        ]
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/ec2/home?region={}#TargetGroups:",
            region, region
        ))
    }
}

// ── Target health (lazy-loaded via describe_target_health) ────────────────────

#[derive(Debug, Clone)]
pub struct TargetHealthEntry {
    pub target_id: String,
    pub port: Option<i32>,
    pub availability_zone: Option<String>,
    pub state: String,
    pub reason: Option<String>,
    pub description: Option<String>,
}

pub async fn fetch_target_health(
    client: ElbClient,
    target_group_arn: String,
) -> Result<Vec<TargetHealthEntry>> {
    let resp = client
        .describe_target_health()
        .target_group_arn(&target_group_arn)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;

    let entries: Vec<TargetHealthEntry> = resp
        .target_health_descriptions()
        .iter()
        .map(|d| {
            let (state, reason, description) = match d.target_health() {
                Some(h) => (
                    h.state().map(|s| s.as_str().to_string()).unwrap_or_default(),
                    h.reason().map(|r| r.as_str().to_string()),
                    h.description().map(|s| s.to_string()),
                ),
                None => (String::new(), None, None),
            };
            let (target_id, port, availability_zone) = match d.target() {
                Some(t) => (
                    t.id().unwrap_or("").to_string(),
                    t.port(),
                    t.availability_zone().map(|s| s.to_string()),
                ),
                None => (String::new(), None, None),
            };
            TargetHealthEntry {
                target_id,
                port,
                availability_zone,
                state,
                reason,
                description,
            }
        })
        .collect();

    Ok(entries)
}

// ── Listeners + attributes (lazy-loaded for the LoadBalancer split pane) ──────

#[derive(Debug, Clone)]
pub struct LbRule {
    pub priority: String,
    pub conditions: Vec<String>,
    pub action: String,
}

#[derive(Debug, Clone)]
pub struct LbListener {
    pub protocol: String,
    pub port: Option<i32>,
    pub ssl_policy: Option<String>,
    pub certificate_count: usize,
    pub default_action: String,
    /// Non-default routing rules, ordered by priority.
    pub rules: Vec<LbRule>,
}

#[derive(Debug, Clone)]
pub struct LbDetails {
    pub listeners: Vec<LbListener>,
    pub attributes: Vec<(String, String)>,
}

/// Extract the human-readable target group name from a target group ARN.
/// `arn:aws:elasticloadbalancing:…:targetgroup/NAME/abc123` → `NAME`.
fn tg_name_from_arn(arn: &str) -> String {
    arn.split('/').nth(1).unwrap_or(arn).to_string()
}

/// Summarise a listener's default action(s) into a short, support-readable
/// string (e.g. `forward → api-tg`, `redirect → HTTPS:443`, `fixed 503`).
fn summarize_default_actions(
    actions: &[aws_sdk_elasticloadbalancingv2::types::Action],
) -> String {
    use aws_sdk_elasticloadbalancingv2::types::ActionTypeEnum;

    let parts: Vec<String> = actions
        .iter()
        .map(|a| match a.r#type() {
            Some(ActionTypeEnum::Forward) => {
                let mut tgs: Vec<String> = vec![];
                if let Some(fwd) = a.forward_config() {
                    for t in fwd.target_groups() {
                        if let Some(arn) = t.target_group_arn() {
                            tgs.push(tg_name_from_arn(arn));
                        }
                    }
                }
                if tgs.is_empty() {
                    "forward".to_string()
                } else {
                    format!("forward → {}", tgs.join(", "))
                }
            }
            Some(ActionTypeEnum::Redirect) => {
                if let Some(r) = a.redirect_config() {
                    let proto = r.protocol().unwrap_or("#{protocol}");
                    let port = r.port().unwrap_or("#{port}");
                    let code = r.status_code().map(|c| c.as_str()).unwrap_or("");
                    format!("redirect → {}:{} {}", proto, port, code).trim().to_string()
                } else {
                    "redirect".to_string()
                }
            }
            Some(ActionTypeEnum::FixedResponse) => {
                let code = a
                    .fixed_response_config()
                    .and_then(|f| f.status_code())
                    .unwrap_or("");
                format!("fixed-response {}", code).trim().to_string()
            }
            Some(ActionTypeEnum::AuthenticateOidc) => "authenticate-oidc".to_string(),
            Some(ActionTypeEnum::AuthenticateCognito) => "authenticate-cognito".to_string(),
            Some(other) => other.as_str().to_string(),
            None => "?".to_string(),
        })
        .collect();

    parts.join("; ")
}

/// Summarise a single rule condition into a short, support-readable string
/// (e.g. `path: /api/*`, `host: api.example.com`, `method: GET, POST`).
fn summarize_condition(c: &aws_sdk_elasticloadbalancingv2::types::RuleCondition) -> String {
    let field = c.field().unwrap_or("");
    let label = match field {
        "host-header" => "host",
        "path-pattern" => "path",
        "http-header" => "header",
        "http-request-method" => "method",
        "query-string" => "query",
        "source-ip" => "source-ip",
        other => other,
    };

    // Prefer the typed config; fall back to the generic `values()` list.
    let values: Vec<String> = match field {
        "host-header" => c
            .host_header_config()
            .map(|h| h.values().to_vec())
            .unwrap_or_default(),
        "path-pattern" => c
            .path_pattern_config()
            .map(|p| p.values().to_vec())
            .unwrap_or_default(),
        "http-request-method" => c
            .http_request_method_config()
            .map(|m| m.values().to_vec())
            .unwrap_or_default(),
        "source-ip" => c
            .source_ip_config()
            .map(|s| s.values().to_vec())
            .unwrap_or_default(),
        "http-header" => {
            if let Some(h) = c.http_header_config() {
                let name = h.http_header_name().unwrap_or("");
                let vals = h.values().join(", ");
                return format!("header {}: {}", name, vals);
            }
            vec![]
        }
        "query-string" => {
            if let Some(q) = c.query_string_config() {
                let pairs: Vec<String> = q
                    .values()
                    .iter()
                    .map(|kv| match (kv.key(), kv.value()) {
                        (Some(k), Some(v)) => format!("{}={}", k, v),
                        (None, Some(v)) => v.to_string(),
                        _ => String::new(),
                    })
                    .collect();
                return format!("query: {}", pairs.join(", "));
            }
            vec![]
        }
        _ => c.values().to_vec(),
    };

    let values = if values.is_empty() {
        c.values().to_vec()
    } else {
        values
    };

    format!("{}: {}", label, values.join(", "))
}

pub async fn fetch_load_balancer_details(
    client: ElbClient,
    load_balancer_arn: String,
) -> Result<LbDetails> {
    // Listeners and attributes are independent calls — run concurrently.
    let listeners_fut = client
        .describe_listeners()
        .load_balancer_arn(&load_balancer_arn)
        .send();
    let attrs_fut = client
        .describe_load_balancer_attributes()
        .load_balancer_arn(&load_balancer_arn)
        .send();

    let (listeners_res, attrs_res) = tokio::join!(listeners_fut, attrs_fut);

    let listeners_resp =
        listeners_res.map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
    let attrs_resp = attrs_res.map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;

    // Collect (listener_arn, listener) so we can fetch each listener's rules.
    let mut listener_arns: Vec<String> = vec![];
    let mut listeners: Vec<LbListener> = listeners_resp
        .listeners()
        .iter()
        .map(|l| {
            listener_arns.push(l.listener_arn().unwrap_or("").to_string());
            LbListener {
                protocol: l.protocol().map(|p| p.as_str().to_string()).unwrap_or_default(),
                port: l.port(),
                ssl_policy: l.ssl_policy().map(|s| s.to_string()),
                certificate_count: l.certificates().len(),
                default_action: summarize_default_actions(l.default_actions()),
                rules: vec![],
            }
        })
        .collect();

    // Fetch rules for every listener concurrently, then attach non-default
    // rules (priority + conditions + action) to each listener.
    let rule_futs = listener_arns.iter().filter(|a| !a.is_empty()).map(|arn| {
        let arn = arn.clone();
        let client = client.clone();
        async move {
            let resp = client.describe_rules().listener_arn(&arn).send().await;
            (arn, resp)
        }
    });
    let rule_results = futures::future::join_all(rule_futs).await;

    let mut rules_by_listener: std::collections::HashMap<String, Vec<LbRule>> =
        std::collections::HashMap::new();
    for (arn, resp) in rule_results {
        if let Ok(resp) = resp {
            let mut rules: Vec<LbRule> = resp
                .rules()
                .iter()
                .filter(|r| !r.is_default().unwrap_or(false))
                .map(|r| LbRule {
                    priority: r.priority().unwrap_or("").to_string(),
                    conditions: r.conditions().iter().map(summarize_condition).collect(),
                    action: summarize_default_actions(r.actions()),
                })
                .collect();
            rules.sort_by_key(|r| r.priority.parse::<i32>().unwrap_or(i32::MAX));
            rules_by_listener.insert(arn, rules);
        }
    }

    for (listener, arn) in listeners.iter_mut().zip(listener_arns.iter()) {
        if let Some(rules) = rules_by_listener.remove(arn) {
            listener.rules = rules;
        }
    }

    listeners.sort_by_key(|l| l.port.unwrap_or(0));

    let mut attributes: Vec<(String, String)> = attrs_resp
        .attributes()
        .iter()
        .filter_map(|a| {
            let key = a.key()?.to_string();
            let value = a.value().unwrap_or("").to_string();
            if value.is_empty() {
                None
            } else {
                Some((key, value))
            }
        })
        .collect();
    attributes.sort_by(|a, b| a.0.cmp(&b.0));

    Ok(LbDetails {
        listeners,
        attributes,
    })
}

// ── Target group attributes (lazy-loaded for the TargetGroup split pane) ──────

pub async fn fetch_target_group_attributes(
    client: ElbClient,
    target_group_arn: String,
) -> Result<Vec<(String, String)>> {
    let resp = client
        .describe_target_group_attributes()
        .target_group_arn(&target_group_arn)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;

    let mut attributes: Vec<(String, String)> = resp
        .attributes()
        .iter()
        .filter_map(|a| {
            let key = a.key()?.to_string();
            let value = a.value().unwrap_or("").to_string();
            if value.is_empty() {
                None
            } else {
                Some((key, value))
            }
        })
        .collect();
    attributes.sort_by(|a, b| a.0.cmp(&b.0));

    Ok(attributes)
}

// ── CloudWatch metrics (`m` overlay) ──────────────────────────────────────────

use crate::aws::services::ec2::MetricsTimeRange;

/// One named metric series for the ELB overlay grid.
#[derive(Debug, Clone)]
pub struct ElbSeries {
    pub title: String,
    pub points: Vec<(f64, f64)>,
}

#[derive(Debug, Clone)]
pub struct ElbMetricsData {
    pub subject: String, // "Load Balancer" / "Target Group"
    pub time_range: MetricsTimeRange,
    pub x_max: f64,
    pub series: Vec<ElbSeries>,
}

#[derive(Debug, Clone)]
pub enum ElbMetricsState {
    Loading,
    Loaded(ElbMetricsData),
}

/// CloudWatch `LoadBalancer` dimension value: the `app/NAME/id` (or `net/…`)
/// suffix of a load balancer ARN. `arn:…:loadbalancer/app/NAME/id` → `app/NAME/id`.
pub fn lb_metric_dimension(arn: &str) -> Option<String> {
    let idx = arn.find(":loadbalancer/")?;
    Some(arn[idx + ":loadbalancer/".len()..].to_string())
}

/// CloudWatch `TargetGroup` dimension value: the `targetgroup/NAME/id` suffix of
/// a target group ARN.
pub fn tg_metric_dimension(arn: &str) -> Option<String> {
    let idx = arn.find(":targetgroup/")?;
    Some(arn[idx + 1..].to_string())
}

/// CloudWatch namespace for an ELB type / target-group protocol.
fn lb_namespace(lb_type: &str) -> &'static str {
    match lb_type {
        "network" => "AWS/NetworkELB",
        "gateway" => "AWS/GatewayELB",
        _ => "AWS/ApplicationELB",
    }
}

/// Run one `get_metric_statistics` per metric (concurrently) over a shared set of
/// dimensions, returning the named series (empty where a metric has no data).
async fn fetch_series(
    cw: &aws_sdk_cloudwatch::Client,
    namespace: &str,
    dims: &[aws_sdk_cloudwatch::types::Dimension],
    metrics: &[(&str, aws_sdk_cloudwatch::types::Statistic, &str)],
    start: i64,
    period: i32,
    now: i64,
) -> Vec<ElbSeries> {
    let start_dt = aws_sdk_cloudwatch::primitives::DateTime::from_secs(start);
    let end_dt = aws_sdk_cloudwatch::primitives::DateTime::from_secs(now);

    let futs: Vec<_> = metrics
        .iter()
        .map(|(name, stat, title)| {
            let req = cw
                .get_metric_statistics()
                .namespace(namespace)
                .metric_name(*name)
                .set_dimensions(Some(dims.to_vec()))
                .start_time(start_dt)
                .end_time(end_dt)
                .period(period)
                .set_statistics(Some(vec![stat.clone()]))
                .send();
            let stat = stat.clone();
            let title = title.to_string();
            async move {
                let points = match req.await {
                    Ok(r) => {
                        let mut pts: Vec<(f64, f64)> = r
                            .datapoints()
                            .iter()
                            .filter_map(|dp| {
                                let v = match stat {
                                    aws_sdk_cloudwatch::types::Statistic::Sum => dp.sum(),
                                    aws_sdk_cloudwatch::types::Statistic::Average => dp.average(),
                                    aws_sdk_cloudwatch::types::Statistic::Maximum => dp.maximum(),
                                    aws_sdk_cloudwatch::types::Statistic::Minimum => dp.minimum(),
                                    _ => dp.sum(),
                                };
                                Some((dp.timestamp()?.secs() as f64 - start as f64, v?))
                            })
                            .collect();
                        pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
                        pts
                    }
                    Err(_) => vec![],
                };
                ElbSeries { title, points }
            }
        })
        .collect();
    futures::future::join_all(futs).await
}

fn metrics_window(time_range: MetricsTimeRange) -> (i64, i64, i32) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    (now - time_range.duration_secs(), now, time_range.period_secs())
}

/// Load balancer metrics — request rate, latency, errors (ALB) or flows /
/// processed bytes / resets (NLB), depending on the LB type.
pub async fn fetch_lb_metrics(
    cw: aws_sdk_cloudwatch::Client,
    lb_arn: String,
    lb_type: String,
    time_range: MetricsTimeRange,
) -> Result<ElbMetricsData> {
    use aws_sdk_cloudwatch::types::{Dimension, Statistic};

    let dim_value = lb_metric_dimension(&lb_arn)
        .ok_or_else(|| crate::error::Error::AwsSdk("Not an ELBv2 load balancer ARN".to_string()))?;
    let dims = vec![Dimension::builder()
        .name("LoadBalancer")
        .value(dim_value)
        .build()];
    let (start, now, period) = metrics_window(time_range);
    let namespace = lb_namespace(&lb_type);

    let metrics: &[(&str, Statistic, &str)] = if lb_type == "network" {
        &[
            ("ActiveFlowCount", Statistic::Average, "Active Flows"),
            ("NewFlowCount", Statistic::Sum, "New Flows"),
            ("ProcessedBytes", Statistic::Sum, "Processed Bytes"),
            ("TCP_Target_Reset_Count", Statistic::Sum, "TCP Target Resets"),
        ]
    } else if lb_type == "gateway" {
        &[
            ("ActiveFlowCount", Statistic::Average, "Active Flows"),
            ("NewFlowCount", Statistic::Sum, "New Flows"),
            ("ProcessedBytes", Statistic::Sum, "Processed Bytes"),
        ]
    } else {
        &[
            ("RequestCount", Statistic::Sum, "Requests"),
            ("TargetResponseTime", Statistic::Average, "Target Latency (s)"),
            ("HTTPCode_Target_5XX_Count", Statistic::Sum, "Target 5XX"),
            ("HTTPCode_ELB_5XX_Count", Statistic::Sum, "ELB 5XX"),
            ("HTTPCode_Target_4XX_Count", Statistic::Sum, "Target 4XX"),
            ("ActiveConnectionCount", Statistic::Sum, "Active Connections"),
        ]
    };

    let series = fetch_series(&cw, namespace, &dims, metrics, start, period, now).await;
    Ok(ElbMetricsData {
        subject: "Load Balancer".to_string(),
        time_range,
        x_max: time_range.duration_secs() as f64,
        series,
    })
}

/// Target group metrics — healthy/unhealthy hosts, request rate per target, and
/// latency. Needs both the `TargetGroup` and `LoadBalancer` dimensions; the
/// namespace follows the target-group protocol (HTTP/HTTPS → ALB, else NLB).
pub async fn fetch_tg_metrics(
    cw: aws_sdk_cloudwatch::Client,
    tg_arn: String,
    lb_arns: Vec<String>,
    protocol: String,
    time_range: MetricsTimeRange,
) -> Result<ElbMetricsData> {
    use aws_sdk_cloudwatch::types::{Dimension, Statistic};

    let tg_dim = tg_metric_dimension(&tg_arn)
        .ok_or_else(|| crate::error::Error::AwsSdk("Not a target group ARN".to_string()))?;
    let lb_dim = lb_arns.first().and_then(|a| lb_metric_dimension(a));
    let mut dims = vec![Dimension::builder().name("TargetGroup").value(tg_dim).build()];
    if let Some(lb) = lb_dim {
        dims.push(Dimension::builder().name("LoadBalancer").value(lb).build());
    }
    let (start, now, period) = metrics_window(time_range);
    let is_alb = matches!(protocol.as_str(), "HTTP" | "HTTPS");
    let namespace = if is_alb { "AWS/ApplicationELB" } else { "AWS/NetworkELB" };

    let metrics: &[(&str, Statistic, &str)] = if is_alb {
        &[
            ("HealthyHostCount", Statistic::Average, "Healthy Hosts"),
            ("UnHealthyHostCount", Statistic::Maximum, "Unhealthy Hosts"),
            ("RequestCountPerTarget", Statistic::Sum, "Requests / Target"),
            ("TargetResponseTime", Statistic::Average, "Target Latency (s)"),
        ]
    } else {
        &[
            ("HealthyHostCount", Statistic::Average, "Healthy Hosts"),
            ("UnHealthyHostCount", Statistic::Maximum, "Unhealthy Hosts"),
        ]
    };

    let series = fetch_series(&cw, namespace, &dims, metrics, start, period, now).await;
    Ok(ElbMetricsData {
        subject: "Target Group".to_string(),
        time_range,
        x_max: time_range.duration_secs() as f64,
        series,
    })
}
