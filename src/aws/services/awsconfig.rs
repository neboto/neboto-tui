use crate::aws::client::AwsClients;
use crate::aws::resource::{native_state_label, Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use aws_sdk_config::Client as ConfigClient;
use std::any::Any;
use std::collections::HashMap;
use tokio::sync::mpsc;

pub struct AwsConfigService {
    client: ConfigClient,
}

impl AwsConfigService {
    pub fn new(aws_clients: &AwsClients) -> Self {
        Self {
            client: aws_clients.awsconfig_client(),
        }
    }
}

#[async_trait]
impl AwsService for AwsConfigService {
    fn service_type(&self) -> ServiceType {
        ServiceType::Config
    }

    fn name(&self) -> &str {
        "AWS Config"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        self.list_resources_streaming(tx, ServiceType::Config).await?;
        Ok(vec![])
    }

    async fn list_resources_streaming(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        // Check if Config recorder is active
        if !self.is_recorder_active().await {
            let _ = event_tx.send(Event::ResourceLoadError {
                service: service_type,
                error: "AWS Config isn't recording in this region. Enable a configuration recorder to use this service.".to_string(),
            });
            return Ok(());
        }

        let mut total = 0usize;

        // Phase 1: Load rules
        let rules = self.fetch_rules().await;
        if !rules.is_empty() {
            let count = rules.len();
            total += count;
            let batch: Vec<Box<dyn Resource>> = rules
                .into_iter()
                .map(|r| Box::new(r) as Box<dyn Resource>)
                .collect();
            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                service: service_type,
                resources: batch,
                progress: LoadProgress {
                    loaded_count: total,
                    total_count: None,
                    status_message: Some("Loading conformance packs…".to_string()),
                },
            });
        }

        // Phase 2: Load conformance packs
        let packs = self.fetch_conformance_packs().await;
        if !packs.is_empty() {
            let count = packs.len();
            total += count;
            let batch: Vec<Box<dyn Resource>> = packs
                .into_iter()
                .map(|p| Box::new(p) as Box<dyn Resource>)
                .collect();
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

impl AwsConfigService {
    async fn is_recorder_active(&self) -> bool {
        match self.client.describe_configuration_recorder_status().send().await {
            Ok(resp) => resp
                .configuration_recorders_status()
                .iter()
                .any(|s| s.recording()),
            Err(_) => false,
        }
    }

    async fn fetch_rules(&self) -> Vec<ConfigRule> {
        // Get all rules
        let mut rules_raw = Vec::new();
        let mut paginator = self.client.describe_config_rules().into_paginator().send();
        while let Some(Ok(page)) = paginator.next().await {
            for rule in page.config_rules() {
                rules_raw.push(rule.clone());
            }
        }

        // Get compliance for all rules
        let mut compliance_map: HashMap<String, (String, i32)> = HashMap::new();
        let mut comp_paginator = self
            .client
            .describe_compliance_by_config_rule()
            .into_paginator()
            .send();
        while let Some(Ok(page)) = comp_paginator.next().await {
            for c in page.compliance_by_config_rules() {
                if let Some(name) = c.config_rule_name() {
                    let status = c
                        .compliance()
                        .and_then(|comp| comp.compliance_type())
                        .map(|t| t.as_str().to_string())
                        .unwrap_or_else(|| "INSUFFICIENT_DATA".to_string());
                    let count = c
                        .compliance()
                        .and_then(|comp| comp.compliance_contributor_count())
                        .map(|cc| cc.capped_count())
                        .unwrap_or(0);
                    compliance_map.insert(name.to_string(), (status, count));
                }
            }
        }

        // Build ConfigRule structs
        rules_raw
            .into_iter()
            .map(|rule| {
                let name = rule.config_rule_name().unwrap_or_default().to_string();
                let arn = rule.config_rule_arn().unwrap_or_default().to_string();
                let rule_id = rule.config_rule_id().unwrap_or_default().to_string();
                let description = rule.description().unwrap_or_default().to_string();

                let (source, identifier) = if let Some(src) = rule.source() {
                    let owner = src.owner().as_str().to_string();
                    let id = src
                        .source_identifier()
                        .unwrap_or_default()
                        .to_string();
                    (owner, id)
                } else {
                    (String::new(), String::new())
                };

                let trigger = rule
                    .maximum_execution_frequency()
                    .map(|_| "Periodic".to_string())
                    .unwrap_or_else(|| "ConfigurationChanges".to_string());

                let (compliance, non_compliant_count) = compliance_map
                    .get(&name)
                    .cloned()
                    .unwrap_or_else(|| ("INSUFFICIENT_DATA".to_string(), 0));

                let parameters: Vec<(String, String)> = rule
                    .input_parameters()
                    .map(|p| {
                        // Input parameters is a JSON string
                        serde_json::from_str::<HashMap<String, serde_json::Value>>(p)
                            .unwrap_or_default()
                            .into_iter()
                            .map(|(k, v)| (k, v.to_string().trim_matches('"').to_string()))
                            .collect()
                    })
                    .unwrap_or_default();

                ConfigRule {
                    name,
                    arn,
                    rule_id,
                    source,
                    identifier,
                    trigger,
                    compliance,
                    non_compliant_count,
                    description,
                    parameters,
                }
            })
            .collect()
    }

    async fn fetch_conformance_packs(&self) -> Vec<ConfigConformancePack> {
        let mut packs = Vec::new();
        let mut paginator = self
            .client
            .describe_conformance_packs()
            .into_paginator()
            .send();
        while let Some(Ok(page)) = paginator.next().await {
            for pack in page.conformance_pack_details() {
                packs.push(ConfigConformancePack {
                    name: pack.conformance_pack_name().to_string(),
                    arn: pack.conformance_pack_arn().to_string(),
                    compliance: String::new(), // filled below
                });
            }
        }

        // Get compliance summary
        if !packs.is_empty() {
            let pack_names: Vec<String> = packs.iter().map(|p| p.name.clone()).collect();
            if let Ok(resp) = self
                .client
                .get_conformance_pack_compliance_summary()
                .set_conformance_pack_names(Some(pack_names))
                .send()
                .await
            {
                for summary in resp.conformance_pack_compliance_summary_list() {
                    if let Some(pack) = packs
                        .iter_mut()
                        .find(|p| p.name == summary.conformance_pack_name())
                    {
                        pack.compliance = summary
                            .conformance_pack_compliance_status()
                            .as_str()
                            .to_string();
                    }
                }
            }
        }

        packs
    }
}

// ── ConfigRule ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ConfigRule {
    pub name: String,
    pub arn: String,
    pub rule_id: String,
    pub source: String,
    pub identifier: String,
    pub trigger: String,
    pub compliance: String,
    pub non_compliant_count: i32,
    pub description: String,
    pub parameters: Vec<(String, String)>,
}

crate::sections! {
    pub enum ConfigRuleDetailSection,
    pub static CONFIG_RULE_SECTIONS = [
        Details "Details",
        NonCompliant "Non-Compliant" => crate::app::App::trigger_config_eval_load,
        Parameters "Parameters",
    ]
}

impl Resource for ConfigRule {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&CONFIG_RULE_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.rule_id
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn resource_type(&self) -> &str {
        "Config Rule"
    }
    fn is_noise(&self) -> bool {
        // A compliant rule needs no attention — hide by default.
        self.compliance == "COMPLIANT"
    }
    fn state(&self) -> ResourceState {
        match self.compliance.as_str() {
            "COMPLIANT" => ResourceState::Available,
            "NON_COMPLIANT" => ResourceState::Unavailable,
            _ => ResourceState::Unknown(self.compliance.clone()),
        }
    }

    fn state_label(&self) -> String {
        native_state_label(&self.compliance, || self.state())
    }
    fn tags(&self) -> &HashMap<String, String> {
        static EMPTY: std::sync::LazyLock<HashMap<String, String>> =
            std::sync::LazyLock::new(HashMap::new);
        &EMPTY
    }
    fn search_text(&self) -> String {
        // Use unique tokens: "noncompliant" vs "compliant" (no overlap since
        // fuzzy matching "noncompliant" won't prefer "compliant" over itself)
        let comp_token = match self.compliance.as_str() {
            "NON_COMPLIANT" => "noncompliant",
            "COMPLIANT" => "compliant",
            _ => "insufficient",
        };
        format!(
            "{} {} {} {}",
            self.name, self.identifier, self.description, comp_token,
        )
    }
    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Rule".to_string(), self.name.clone()),
            ("Compliance".to_string(), self.compliance.clone()),
            ("Non-Compliant".to_string(), self.non_compliant_count.to_string()),
            ("Source".to_string(), self.source.clone()),
            ("Identifier".to_string(), self.identifier.clone()),
            ("Trigger".to_string(), self.trigger.clone()),
        ]
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/config/home?region={}#/rules/details?configRuleName={}",
            region, region, self.name
        ))
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── ConfigConformancePack ─────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ConfigConformancePack {
    pub name: String,
    pub arn: String,
    pub compliance: String,
}

impl Resource for ConfigConformancePack {
    fn id(&self) -> &str {
        &self.arn
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn resource_type(&self) -> &str {
        "Conformance Pack"
    }
    fn state(&self) -> ResourceState {
        match self.compliance.as_str() {
            "COMPLIANT" => ResourceState::Available,
            "NON_COMPLIANT" => ResourceState::Unavailable,
            _ => ResourceState::Unknown(self.compliance.clone()),
        }
    }

    fn state_label(&self) -> String {
        native_state_label(&self.compliance, || self.state())
    }
    fn tags(&self) -> &HashMap<String, String> {
        static EMPTY: std::sync::LazyLock<HashMap<String, String>> =
            std::sync::LazyLock::new(HashMap::new);
        &EMPTY
    }
    fn search_text(&self) -> String {
        let comp_token = match self.compliance.as_str() {
            "NON_COMPLIANT" => "noncompliant",
            "COMPLIANT" => "compliant",
            _ => "insufficient",
        };
        format!("{} {}", self.name, comp_token)
    }
    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Name".to_string(), self.name.clone()),
            ("ARN".to_string(), self.arn.clone()),
            ("Compliance".to_string(), self.compliance.clone()),
        ]
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/config/home?region={}#/conformance-packs/details?conformancePackName={}",
            region, region, self.name
        ))
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── Lazy-loaded state types ───────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ConfigEvalResult {
    pub resource_type: String,
    pub resource_id: String,
    pub annotation: String,
    pub last_evaluated: Option<String>,
}

pub async fn fetch_noncompliant_resources(
    client: ConfigClient,
    rule_name: String,
) -> Result<Vec<ConfigEvalResult>> {
    let mut results = Vec::new();
    let mut paginator = client
        .get_compliance_details_by_config_rule()
        .config_rule_name(&rule_name)
        .compliance_types(aws_sdk_config::types::ComplianceType::NonCompliant)
        .into_paginator()
        .send();

    while let Some(Ok(page)) = paginator.next().await {
        for eval in page.evaluation_results() {
            let qualifier = eval.evaluation_result_identifier()
                .and_then(|id| id.evaluation_result_qualifier());
            let resource_type = qualifier
                .and_then(|q| q.resource_type())
                .unwrap_or_default()
                .to_string();
            let resource_id = qualifier
                .and_then(|q| q.resource_id())
                .unwrap_or_default()
                .to_string();
            let annotation = eval.annotation().unwrap_or_default().to_string();
            let last_evaluated = eval
                .result_recorded_time()
                .map(|d| fmt_epoch_secs(d.secs()));
            results.push(ConfigEvalResult {
                resource_type,
                resource_id,
                annotation,
                last_evaluated,
            });
        }
    }
    Ok(results)
}

// ── Timestamp helpers ─────────────────────────────────────────────────────────

fn fmt_epoch_secs(secs: i64) -> String {
    let days = secs / 86400;
    let time_rem = secs % 86400;
    let h = time_rem / 3600;
    let m = (time_rem % 3600) / 60;
    let (y, mo, d) = epoch_days_to_ymd(days);
    format!("{:04}-{:02}-{:02} {:02}:{:02} UTC", y, mo, d, h, m)
}

fn epoch_days_to_ymd(mut days: i64) -> (i32, u8, u8) {
    let mut year = 1970i32;
    loop {
        let dy = if is_leap(year) { 366 } else { 365 };
        if days < dy { break; }
        days -= dy;
        year += 1;
    }
    let dm = [31u8, if is_leap(year) { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut month = 1u8;
    for &d in &dm {
        if days < d as i64 { break; }
        days -= d as i64;
        month += 1;
    }
    (year, month, (days + 1) as u8)
}

fn is_leap(y: i32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}
