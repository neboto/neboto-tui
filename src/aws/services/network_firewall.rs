use crate::aws::client::AwsClients;
use crate::aws::resource::{native_state_label, Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::aws::services::ec2::MetricsTimeRange;
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use aws_sdk_cloudwatch::primitives::DateTime as CwDateTime;
use aws_sdk_networkfirewall::Client as NfwClient;
use std::any::Any;
use std::collections::HashMap;
use tokio::sync::mpsc;

/// AWS Network Firewall — sub-tabs Firewalls / Firewall Policies / Rule Groups.
/// Each tab is a list→describe (N+1) load with bounded concurrency. Firewalls and
/// policies get split detail panes; rule-group contents (Suricata text or the
/// structured 5-tuple/stateless rules) are fetched lazily on first view.
pub struct NetworkFirewallService {
    client: NfwClient,
}

impl NetworkFirewallService {
    pub fn new(aws_clients: &AwsClients) -> Self {
        Self {
            client: aws_clients.network_firewall_client(),
        }
    }
}

/// How many `describe_*` calls to keep in flight per page during the N+1 load.
const DESCRIBE_CONCURRENCY: usize = 8;

#[async_trait]
impl AwsService for NetworkFirewallService {
    fn service_type(&self) -> ServiceType {
        ServiceType::NetworkFirewall
    }

    fn name(&self) -> &str {
        "Network Firewall"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        self.list_resources_streaming(tx, ServiceType::NetworkFirewall)
            .await?;
        Ok(vec![])
    }

    async fn list_resources_streaming(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        let mut total = 0usize;

        // ── Firewalls (list → describe_firewall per arn) ────────────────────
        let mut fw_arns: Vec<String> = Vec::new();
        let mut paginator = self.client.list_firewalls().into_paginator().send();
        while let Some(result) = paginator.next().await {
            match result {
                Ok(page) => {
                    for f in page.firewalls() {
                        if let Some(arn) = f.firewall_arn() {
                            fw_arns.push(arn.to_string());
                        }
                    }
                }
                Err(e) => {
                    let _ = event_tx.send(Event::ResourceLoadError {
                        service: service_type,
                        error: format!(
                            "Failed to list firewalls: {}",
                            crate::error::sdk_error_message(&e)
                        ),
                    });
                    return Ok(());
                }
            }
        }

        for chunk in fw_arns.chunks(DESCRIBE_CONCURRENCY) {
            let futs = chunk.iter().map(|arn| {
                let client = self.client.clone();
                let arn = arn.clone();
                async move {
                    client
                        .describe_firewall()
                        .firewall_arn(&arn)
                        .send()
                        .await
                        .ok()
                        .map(|resp| NfwFirewall::from_describe(&resp))
                }
            });
            let batch: Vec<Box<dyn Resource>> = futures::future::join_all(futs)
                .await
                .into_iter()
                .flatten()
                .map(|f| Box::new(f) as Box<dyn Resource>)
                .collect();
            if !batch.is_empty() {
                total += batch.len();
                let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                    service: service_type,
                    resources: batch,
                    progress: LoadProgress {
                        loaded_count: total,
                        total_count: None,
                        status_message: Some("Loading firewalls…".to_string()),
                    },
                });
            }
        }

        // ── Firewall policies (list → describe_firewall_policy per arn) ──────
        let mut pol_arns: Vec<String> = Vec::new();
        let mut paginator = self
            .client
            .list_firewall_policies()
            .into_paginator()
            .send();
        while let Some(result) = paginator.next().await {
            match result {
                Ok(page) => {
                    for p in page.firewall_policies() {
                        if let Some(arn) = p.arn() {
                            pol_arns.push(arn.to_string());
                        }
                    }
                }
                Err(e) => {
                    let _ = event_tx.send(Event::ResourceLoadError {
                        service: service_type,
                        error: format!(
                            "Failed to list firewall policies: {}",
                            crate::error::sdk_error_message(&e)
                        ),
                    });
                    return Ok(());
                }
            }
        }

        for chunk in pol_arns.chunks(DESCRIBE_CONCURRENCY) {
            let futs = chunk.iter().map(|arn| {
                let client = self.client.clone();
                let arn = arn.clone();
                async move {
                    client
                        .describe_firewall_policy()
                        .firewall_policy_arn(&arn)
                        .send()
                        .await
                        .ok()
                        .map(|resp| NfwPolicy::from_describe(&resp))
                }
            });
            let batch: Vec<Box<dyn Resource>> = futures::future::join_all(futs)
                .await
                .into_iter()
                .flatten()
                .map(|p| Box::new(p) as Box<dyn Resource>)
                .collect();
            if !batch.is_empty() {
                total += batch.len();
                let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                    service: service_type,
                    resources: batch,
                    progress: LoadProgress {
                        loaded_count: total,
                        total_count: None,
                        status_message: Some("Loading firewall policies…".to_string()),
                    },
                });
            }
        }

        // ── Rule groups (summaries only; rules are lazy) ─────────────────────
        let mut rule_groups: Vec<NfwRuleGroup> = Vec::new();
        let mut paginator = self.client.list_rule_groups().into_paginator().send();
        while let Some(result) = paginator.next().await {
            match result {
                Ok(page) => {
                    for rg in page.rule_groups() {
                        if let (Some(name), Some(arn)) = (rg.name(), rg.arn()) {
                            rule_groups.push(NfwRuleGroup::from_summary(name, arn));
                        }
                    }
                }
                Err(e) => {
                    let _ = event_tx.send(Event::ResourceLoadError {
                        service: service_type,
                        error: format!(
                            "Failed to list rule groups: {}",
                            crate::error::sdk_error_message(&e)
                        ),
                    });
                    return Ok(());
                }
            }
        }
        if !rule_groups.is_empty() {
            let batch: Vec<Box<dyn Resource>> = rule_groups
                .into_iter()
                .map(|rg| Box::new(rg) as Box<dyn Resource>)
                .collect();
            total += batch.len();
            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                service: service_type,
                resources: batch,
                progress: LoadProgress {
                    loaded_count: total,
                    total_count: None,
                    status_message: Some("Loading rule groups…".to_string()),
                },
            });
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

// ── NfwFirewall ───────────────────────────────────────────────────────────────

/// Per-AZ config-sync status, the operational signal that tells you whether a
/// firewall's policy has propagated to every endpoint.
#[derive(Debug, Clone)]
pub struct NfwSyncState {
    pub availability_zone: String,
    pub subnet_id: String,
    pub endpoint_id: String,
    pub status: String,
}

#[derive(Debug, Clone)]
pub struct NfwFirewall {
    pub name: String,
    pub arn: String,
    pub vpc_id: String,
    pub policy_arn: String,
    pub status: String, // READY / PROVISIONING / DELETING
    pub configuration_sync: String,
    pub description: String,
    pub subnet_mappings: Vec<String>, // subnet ids (one per AZ)
    pub delete_protection: bool,
    pub policy_change_protection: bool,
    pub subnet_change_protection: bool,
    pub sync_states: Vec<NfwSyncState>,
    pub tags: HashMap<String, String>,
}

impl NfwFirewall {
    pub fn from_describe(
        resp: &aws_sdk_networkfirewall::operation::describe_firewall::DescribeFirewallOutput,
    ) -> Self {
        let fw = resp.firewall();
        let status_obj = resp.firewall_status();

        let (name, arn, vpc_id, policy_arn, description, subnet_mappings, delete_protection,
            policy_change_protection, subnet_change_protection, tags) = match fw {
            Some(f) => (
                f.firewall_name().unwrap_or_default().to_string(),
                f.firewall_arn().unwrap_or_default().to_string(),
                f.vpc_id().to_string(),
                f.firewall_policy_arn().to_string(),
                f.description().unwrap_or_default().to_string(),
                f.subnet_mappings()
                    .iter()
                    .map(|m| m.subnet_id().to_string())
                    .collect(),
                f.delete_protection(),
                f.firewall_policy_change_protection(),
                f.subnet_change_protection(),
                f.tags()
                    .iter()
                    .map(|t| (t.key().to_string(), t.value().to_string()))
                    .collect(),
            ),
            None => (
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                Vec::new(),
                false,
                false,
                false,
                HashMap::new(),
            ),
        };

        let (status, configuration_sync, sync_states) = match status_obj {
            Some(s) => {
                let status = s.status().as_str().to_string();
                let configuration_sync =
                    s.configuration_sync_state_summary().as_str().to_string();
                let sync_states = s
                    .sync_states()
                    .map(|m| {
                        let mut v: Vec<NfwSyncState> = m
                            .iter()
                            .map(|(az, ss)| {
                                let att = ss.attachment();
                                NfwSyncState {
                                    availability_zone: az.clone(),
                                    subnet_id: att
                                        .and_then(|a| a.subnet_id())
                                        .unwrap_or_default()
                                        .to_string(),
                                    endpoint_id: att
                                        .and_then(|a| a.endpoint_id())
                                        .unwrap_or_default()
                                        .to_string(),
                                    status: att
                                        .and_then(|a| a.status())
                                        .map(|st| st.as_str().to_string())
                                        .unwrap_or_default(),
                                }
                            })
                            .collect();
                        v.sort_by(|a, b| a.availability_zone.cmp(&b.availability_zone));
                        v
                    })
                    .unwrap_or_default();
                (status, configuration_sync, sync_states)
            }
            None => (String::new(), String::new(), Vec::new()),
        };

        Self {
            name,
            arn,
            vpc_id,
            policy_arn,
            status,
            configuration_sync,
            description,
            subnet_mappings,
            delete_protection,
            policy_change_protection,
            subnet_change_protection,
            sync_states,
            tags,
        }
    }
}

crate::sections! {
    pub enum NfwFirewallDetailSection,
    pub static NFW_FIREWALL_SECTIONS = [
        Details "Details" => crate::app::App::trigger_nfw_logging_load,
        Subnets "Subnets" => crate::app::App::trigger_nfw_logging_load,
        Policy "Policy" => crate::app::App::trigger_nfw_logging_load,
        Logging "Logging" => crate::app::App::trigger_nfw_logging_load,
        Tags "Tags" => crate::app::App::trigger_nfw_logging_load,
    ]
}

impl Resource for NfwFirewall {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&NFW_FIREWALL_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.arn
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn resource_type(&self) -> &str {
        "Firewall"
    }

    fn state(&self) -> ResourceState {
        nfw_status_to_state(&self.status)
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
            self.name, self.arn, self.vpc_id, self.status, self.policy_arn
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        // Fallback only — the split pane is the real view; this feeds export.
        vec![
            ("Name".to_string(), self.name.clone()),
            ("ARN".to_string(), self.arn.clone()),
            ("Status".to_string(), self.status.clone()),
            ("VPC".to_string(), self.vpc_id.clone()),
            ("Policy".to_string(), self.policy_arn.clone()),
            (
                "Delete Protection".to_string(),
                self.delete_protection.to_string(),
            ),
        ]
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/vpc/home?region={}#NetworkFirewalls:",
            region, region
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── NfwPolicy ─────────────────────────────────────────────────────────────────

/// A rule-group reference on a policy: the referenced rule-group ARN plus its
/// evaluation priority (stateless always has one; stateful may).
#[derive(Debug, Clone)]
pub struct NfwRuleGroupRef {
    pub arn: String,
    pub priority: Option<i32>,
}

#[derive(Debug, Clone)]
pub struct NfwPolicy {
    pub name: String,
    pub arn: String,
    pub description: String,
    pub status: String,
    pub stateless_default_actions: Vec<String>,
    pub stateless_fragment_default_actions: Vec<String>,
    pub stateful_default_actions: Vec<String>,
    pub stateful_rule_groups: Vec<NfwRuleGroupRef>,
    pub stateless_rule_groups: Vec<NfwRuleGroupRef>,
    pub tags: HashMap<String, String>,
}

impl NfwPolicy {
    pub fn from_describe(
        resp: &aws_sdk_networkfirewall::operation::describe_firewall_policy::DescribeFirewallPolicyOutput,
    ) -> Self {
        let meta = resp.firewall_policy_response();
        let policy = resp.firewall_policy();

        let (name, arn, description, status, tags) = match meta {
            Some(m) => (
                m.firewall_policy_name().to_string(),
                m.firewall_policy_arn().to_string(),
                m.description().unwrap_or_default().to_string(),
                m.firewall_policy_status()
                    .map(|s| s.as_str().to_string())
                    .unwrap_or_default(),
                m.tags()
                    .iter()
                    .map(|t| (t.key().to_string(), t.value().to_string()))
                    .collect(),
            ),
            None => (
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                HashMap::new(),
            ),
        };

        let (
            stateless_default_actions,
            stateless_fragment_default_actions,
            stateful_default_actions,
            stateful_rule_groups,
            stateless_rule_groups,
        ) = match policy {
            Some(p) => (
                p.stateless_default_actions().to_vec(),
                p.stateless_fragment_default_actions().to_vec(),
                p.stateful_default_actions().to_vec(),
                p.stateful_rule_group_references()
                    .iter()
                    .map(|r| NfwRuleGroupRef {
                        arn: r.resource_arn().to_string(),
                        priority: r.priority(),
                    })
                    .collect(),
                p.stateless_rule_group_references()
                    .iter()
                    .map(|r| NfwRuleGroupRef {
                        arn: r.resource_arn().to_string(),
                        priority: Some(r.priority()),
                    })
                    .collect(),
            ),
            None => (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new()),
        };

        Self {
            name,
            arn,
            description,
            status,
            stateless_default_actions,
            stateless_fragment_default_actions,
            stateful_default_actions,
            stateful_rule_groups,
            stateless_rule_groups,
            tags,
        }
    }
}

crate::sections! {
    pub enum NfwPolicyDetailSection,
    pub static NFW_POLICY_SECTIONS = [
        Stateful "Stateful",
        Stateless "Stateless",
        Tags "Tags",
    ]
}

impl Resource for NfwPolicy {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&NFW_POLICY_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.arn
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn resource_type(&self) -> &str {
        "Firewall Policy"
    }

    fn state(&self) -> ResourceState {
        nfw_resource_status_to_state(&self.status)
    }

    fn state_label(&self) -> String {
        native_state_label(&self.status, || self.state())
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!("{} {} {}", self.name, self.arn, self.description)
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Name".to_string(), self.name.clone()),
            ("ARN".to_string(), self.arn.clone()),
            ("Status".to_string(), self.status.clone()),
            (
                "Stateful Rule Groups".to_string(),
                self.stateful_rule_groups.len().to_string(),
            ),
            (
                "Stateless Rule Groups".to_string(),
                self.stateless_rule_groups.len().to_string(),
            ),
        ]
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/vpc/home?region={}#FirewallPolicies:",
            region, region
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── NfwRuleGroup ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct NfwRuleGroup {
    pub name: String,
    pub arn: String,
    pub kind: String, // STATEFUL / STATELESS (inferred from the arn; type + capacity arrive with the lazy describe)
    pub tags: HashMap<String, String>,
}

impl NfwRuleGroup {
    /// From the list summary — only name/arn are known. `kind` is inferred from
    /// the ARN path segment (`stateful-rulegroup` / `stateless-rulegroup`); the
    /// real type + capacity arrive with the lazy describe.
    pub fn from_summary(name: &str, arn: &str) -> Self {
        let kind = if arn.contains("stateless-rulegroup") {
            "STATELESS".to_string()
        } else if arn.contains("stateful-rulegroup") {
            "STATEFUL".to_string()
        } else {
            String::new()
        };
        Self {
            name: name.to_string(),
            arn: arn.to_string(),
            kind,
            tags: HashMap::new(),
        }
    }
}

crate::sections! {
    pub enum NfwRuleGroupDetailSection,
    pub static NFW_RULE_GROUP_SECTIONS = [
        Rules "Rules" => crate::app::App::trigger_nfw_rule_group_rules_load,
        Tags "Tags" => crate::app::App::trigger_nfw_rule_group_rules_load,
    ]
}

impl Resource for NfwRuleGroup {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&NFW_RULE_GROUP_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.arn
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn resource_type(&self) -> &str {
        "NFW Rule Group"
    }

    fn state(&self) -> ResourceState {
        ResourceState::stateless()
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!("{} {} {}", self.name, self.arn, self.kind)
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Name".to_string(), self.name.clone()),
            ("ARN".to_string(), self.arn.clone()),
            (
                "Type".to_string(),
                if self.kind.is_empty() {
                    "—".to_string()
                } else {
                    self.kind.clone()
                },
            ),
        ]
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/vpc/home?region={}#NetworkFirewallRuleGroups:",
            region, region
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── Lazy rule-group contents (DescribeRuleGroup) ──────────────────────────────

/// A parsed stateful 5-tuple rule (when a rule group uses structured `StatefulRules`
/// rather than a Suricata `rules_string`).
#[derive(Debug, Clone)]
pub struct NfwStatefulRule {
    pub action: String,
    pub protocol: String,
    pub source: String,
    pub source_port: String,
    pub direction: String,
    pub destination: String,
    pub destination_port: String,
    pub options: String,
}

/// The contents of a rule group, fetched lazily. A stateful group is either a
/// Suricata `rules_string` (shown in the `v` viewer), a structured 5-tuple table,
/// or a domain allow/deny list; a stateless group is a count of stateless rules.
#[derive(Debug, Clone)]
pub struct NfwRuleGroupRules {
    pub kind: String,
    pub capacity: i32,
    pub description: String,
    /// Suricata-style raw rules text, if the group uses `rules_string`.
    pub rules_string: Option<String>,
    /// Structured stateful 5-tuple rules, if any.
    pub stateful_rules: Vec<NfwStatefulRule>,
    /// Domain-list targets + generated-rule type, if a domain list.
    pub domain_targets: Vec<String>,
    pub domain_rules_type: Option<String>,
    /// Number of stateless rules (rendered as a summary; full 5-tuple expansion
    /// is large and out of scope v1).
    pub stateless_rule_count: usize,
    pub tags: HashMap<String, String>,
}

pub async fn fetch_rule_group_rules(
    client: NfwClient,
    arn: String,
) -> Result<NfwRuleGroupRules> {
    let resp = client
        .describe_rule_group()
        .rule_group_arn(&arn)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;

    let meta = resp.rule_group_response();
    let kind = meta
        .and_then(|m| m.r#type())
        .map(|t| t.as_str().to_string())
        .unwrap_or_default();
    let capacity = meta.and_then(|m| m.capacity()).unwrap_or(0);
    let description = meta
        .and_then(|m| m.description())
        .unwrap_or_default()
        .to_string();
    let tags = meta
        .map(|m| {
            m.tags()
                .iter()
                .map(|t| (t.key().to_string(), t.value().to_string()))
                .collect::<HashMap<_, _>>()
        })
        .unwrap_or_default();

    let mut rules_string = None;
    let mut stateful_rules = Vec::new();
    let mut domain_targets = Vec::new();
    let mut domain_rules_type = None;
    let mut stateless_rule_count = 0;

    if let Some(src) = resp.rule_group().and_then(|rg| rg.rules_source()) {
        if let Some(rs) = src.rules_string() {
            if !rs.is_empty() {
                rules_string = Some(rs.to_string());
            }
        }
        for r in src.stateful_rules() {
            let header = r.header();
            stateful_rules.push(NfwStatefulRule {
                action: r.action().as_str().to_string(),
                protocol: header
                    .map(|h| h.protocol().as_str().to_string())
                    .unwrap_or_default(),
                source: header.map(|h| h.source().to_string()).unwrap_or_default(),
                source_port: header
                    .map(|h| h.source_port().to_string())
                    .unwrap_or_default(),
                direction: header
                    .map(|h| h.direction().as_str().to_string())
                    .unwrap_or_default(),
                destination: header
                    .map(|h| h.destination().to_string())
                    .unwrap_or_default(),
                destination_port: header
                    .map(|h| h.destination_port().to_string())
                    .unwrap_or_default(),
                options: r
                    .rule_options()
                    .iter()
                    .map(|o| {
                        if o.settings().is_empty() {
                            o.keyword().to_string()
                        } else {
                            format!("{}:{}", o.keyword(), o.settings().join(","))
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("; "),
            });
        }
        if let Some(list) = src.rules_source_list() {
            domain_targets = list.targets().to_vec();
            domain_rules_type = Some(list.generated_rules_type().as_str().to_string());
        }
        if let Some(sl) = src.stateless_rules_and_custom_actions() {
            stateless_rule_count = sl.stateless_rules().len();
        }
    }

    Ok(NfwRuleGroupRules {
        kind,
        capacity,
        description,
        rules_string,
        stateful_rules,
        domain_targets,
        domain_rules_type,
        stateless_rule_count,
        tags,
    })
}

// ── Status mapping ─────────────────────────────────────────────────────────────

/// Map a firewall `FirewallStatusValue` to a list status dot.
pub fn nfw_status_to_state(status: &str) -> ResourceState {
    match status {
        "READY" => ResourceState::Available,
        "PROVISIONING" => ResourceState::Pending,
        "DELETING" => ResourceState::Deleting,
        "" => ResourceState::Unknown(String::new()),
        other => ResourceState::Unknown(other.to_string()),
    }
}

/// Map a policy/rule-group `ResourceStatus` (ACTIVE / DELETING / ERROR) to a dot.
pub fn nfw_resource_status_to_state(status: &str) -> ResourceState {
    match status {
        "ACTIVE" => ResourceState::Available,
        "DELETING" => ResourceState::Deleting,
        "ERROR" => ResourceState::Unavailable,
        "" => ResourceState::Available, // status not populated on this path
        other => ResourceState::Unknown(other.to_string()),
    }
}

// ── Metrics (AWS/NetworkFirewall) ───────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct NfwMetricsData {
    pub received: Vec<(f64, f64)>,
    pub passed: Vec<(f64, f64)>,
    pub dropped: Vec<(f64, f64)>,
    pub rejected: Vec<(f64, f64)>,
    pub x_max: f64,
}

#[derive(Debug, Clone)]
pub enum NfwMetricsState {
    Loading,
    Loaded(NfwMetricsData),
}

/// Fetch firewall traffic metrics from the `AWS/NetworkFirewall` namespace.
/// Network Firewall publishes per-`AvailabilityZone` (and per-`Engine`)
/// dimensions, so there is no firewall-wide series to read directly — we use
/// `GetMetricData` with `SUM(SEARCH(...))` to aggregate every AZ/engine series
/// for the firewall into one total per metric.
pub async fn fetch_nfw_metrics(
    cw: aws_sdk_cloudwatch::Client,
    firewall_name: String,
    time_range: MetricsTimeRange,
) -> Result<NfwMetricsData> {
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let start_secs = now_secs - time_range.duration_secs();
    let period = time_range.period_secs();

    let metrics = [
        ("m0", "ReceivedPacketCount"),
        ("m1", "PassedPackets"),
        ("m2", "DroppedPackets"),
        ("m3", "RejectedPackets"),
    ];

    let mut queries = Vec::new();
    for (id, metric) in metrics {
        // SEARCH matches every dimension schema (AZ/Engine/CustomAction) for this
        // firewall+metric; SUM collapses them into one total time series.
        let expr = format!(
            "SUM(SEARCH('Namespace=\"AWS/NetworkFirewall\" MetricName=\"{}\" FirewallName=\"{}\"', 'Sum', {}))",
            metric, firewall_name, period
        );
        queries.push(
            aws_sdk_cloudwatch::types::MetricDataQuery::builder()
                .id(id)
                .expression(expr)
                .build(),
        );
    }

    let resp = cw
        .get_metric_data()
        .set_metric_data_queries(Some(queries))
        .start_time(CwDateTime::from_secs(start_secs))
        .end_time(CwDateTime::from_secs(now_secs))
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;

    let mut series: HashMap<String, Vec<(f64, f64)>> = HashMap::new();
    for r in resp.metric_data_results() {
        let id = r.id().unwrap_or_default().to_string();
        let mut pts: Vec<(f64, f64)> = r
            .timestamps()
            .iter()
            .zip(r.values().iter())
            .map(|(t, v)| (t.secs() as f64 - start_secs as f64, *v))
            .collect();
        pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        series.insert(id, pts);
    }

    let take = |id: &str| series.get(id).cloned().unwrap_or_default();
    Ok(NfwMetricsData {
        received: take("m0"),
        passed: take("m1"),
        dropped: take("m2"),
        rejected: take("m3"),
        x_max: time_range.duration_secs() as f64,
    })
}

// ── Logging configuration (FLOW / ALERT / TLS destinations) ──────────────────

#[derive(Debug, Clone)]
pub struct NfwLogDestination {
    pub log_type: String,         // FLOW / ALERT / TLS
    pub destination_type: String, // CloudWatchLogs / S3 / KinesisDataFirehose
    pub target: String,           // log group / bucket(+prefix) / delivery stream
}

/// Read a firewall's logging configuration → its FLOW/ALERT/TLS destinations,
/// for the detail-pane Logging section.
pub async fn fetch_nfw_logging(
    client: NfwClient,
    firewall_arn: String,
) -> Result<Vec<NfwLogDestination>> {
    let resp = client
        .describe_logging_configuration()
        .firewall_arn(&firewall_arn)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;

    let mut out = Vec::new();
    if let Some(cfg) = resp.logging_configuration() {
        for c in cfg.log_destination_configs() {
            let dest_type = c.log_destination_type().as_str().to_string();
            let d = c.log_destination();
            let target = match dest_type.as_str() {
                "CloudWatchLogs" => d.get("logGroup").cloned().unwrap_or_default(),
                "S3" => {
                    let bucket = d.get("bucketName").cloned().unwrap_or_default();
                    match d.get("prefix") {
                        Some(p) if !p.is_empty() => format!("{}/{}", bucket, p),
                        _ => bucket,
                    }
                }
                "KinesisDataFirehose" => d.get("deliveryStream").cloned().unwrap_or_default(),
                _ => d.values().cloned().collect::<Vec<_>>().join(", "),
            };
            out.push(NfwLogDestination {
                log_type: c.log_type().as_str().to_string(),
                destination_type: dest_type,
                target,
            });
        }
    }
    Ok(out)
}

/// Resolve a firewall's CloudWatch Logs group for the live tail (`t`). Prefers
/// ALERT logs (the actionable ones), then FLOW, then anything. Errors with a
/// helpful message when logging is off or only goes to S3 / Firehose (which we
/// can't tail).
pub async fn resolve_nfw_log_group(
    client: NfwClient,
    firewall_name: String,
    firewall_arn: String,
) -> Result<(String, Vec<String>)> {
    let dests = fetch_nfw_logging(client, firewall_arn).await?;
    let cwl: Vec<&NfwLogDestination> = dests
        .iter()
        .filter(|d| d.destination_type == "CloudWatchLogs" && !d.target.is_empty())
        .collect();
    if cwl.is_empty() {
        return Err(crate::error::Error::AwsSdk(format!(
            "{}: no CloudWatch Logs destination — logging is off or sent to S3/Firehose (can't tail).",
            firewall_name
        )));
    }
    let pick = cwl
        .iter()
        .find(|d| d.log_type == "ALERT")
        .or_else(|| cwl.iter().find(|d| d.log_type == "FLOW"))
        .or_else(|| cwl.first())
        .map(|d| d.target.clone())
        .unwrap();
    Ok((pick, Vec::new()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn firewall_status_maps_to_state() {
        assert!(matches!(
            nfw_status_to_state("READY"),
            ResourceState::Available
        ));
        assert!(matches!(
            nfw_status_to_state("PROVISIONING"),
            ResourceState::Pending
        ));
        assert!(matches!(
            nfw_status_to_state("DELETING"),
            ResourceState::Deleting
        ));
        assert!(matches!(
            nfw_status_to_state("WAT"),
            ResourceState::Unknown(_)
        ));
    }

    #[test]
    fn resource_status_maps_to_state() {
        assert!(matches!(
            nfw_resource_status_to_state("ACTIVE"),
            ResourceState::Available
        ));
        assert!(matches!(
            nfw_resource_status_to_state("ERROR"),
            ResourceState::Unavailable
        ));
        // Empty (status not on this path) is treated as healthy.
        assert!(matches!(
            nfw_resource_status_to_state(""),
            ResourceState::Available
        ));
    }

    #[test]
    fn rule_group_kind_inferred_from_arn() {
        let stateful = NfwRuleGroup::from_summary(
            "rg1",
            "arn:aws:network-firewall:us-east-1:1:stateful-rulegroup/rg1",
        );
        assert_eq!(stateful.kind, "STATEFUL");
        let stateless = NfwRuleGroup::from_summary(
            "rg2",
            "arn:aws:network-firewall:us-east-1:1:stateless-rulegroup/rg2",
        );
        assert_eq!(stateless.kind, "STATELESS");
    }
}
