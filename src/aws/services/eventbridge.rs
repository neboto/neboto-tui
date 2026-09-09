use crate::aws::client::AwsClients;
use crate::aws::resource::{native_state_label, Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use aws_sdk_eventbridge::Client as EbClient;
use std::any::Any;
use std::collections::HashMap;
use tokio::sync::mpsc;

/// EventBridge service — sub-tabs Rules / Event Buses / Archives / Replays /
/// Schedules / Pipes. Rules are listed across every bus (default + custom) and
/// get a split detail pane (Trigger / Targets / Tags); targets are fetched
/// lazily per rule. Schedules (EventBridge Scheduler) and Pipes come from their
/// own SDK clients and stream as error-tolerant batches — a permission gap on
/// either records a warning without breaking the core bus/rule load.
pub struct EventBridgeService {
    client: EbClient,
    scheduler: aws_sdk_scheduler::Client,
    pipes: aws_sdk_pipes::Client,
}

impl EventBridgeService {
    pub fn new(aws_clients: &AwsClients) -> Self {
        Self {
            client: aws_clients.eventbridge_client(),
            scheduler: aws_clients.scheduler_client(),
            pipes: aws_clients.pipes_client(),
        }
    }
}

#[async_trait]
impl AwsService for EventBridgeService {
    fn service_type(&self) -> ServiceType {
        ServiceType::EventBridge
    }

    fn name(&self) -> &str {
        "EventBridge"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        self.list_resources_streaming(tx, ServiceType::EventBridge)
            .await?;
        Ok(vec![])
    }

    async fn list_resources_streaming(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        let mut total = 0usize;

        // ── Event buses (manual next_token pagination) ──────────────────────
        let mut buses: Vec<EbEventBus> = Vec::new();
        let mut bus_token: Option<String> = None;
        loop {
            let mut req = self.client.list_event_buses();
            if let Some(t) = &bus_token {
                req = req.next_token(t);
            }
            match req.send().await {
                Ok(page) => {
                    for b in page.event_buses() {
                        buses.push(EbEventBus::from_sdk(b));
                    }
                    bus_token = crate::aws::pagination::next_page_token(page.next_token(), &bus_token);
                    if bus_token.is_none() {
                        break;
                    }
                }
                Err(e) => {
                    let _ = event_tx.send(Event::ResourceLoadError {
                        service: service_type,
                        error: format!("Failed to list event buses: {}", e),
                    });
                    return Ok(());
                }
            }
        }

        // Tags for each bus, concurrently.
        let bus_tag_futs = buses.iter().map(|b| {
            let client = self.client.clone();
            let arn = b.arn.clone();
            async move { fetch_tags(&client, &arn).await }
        });
        let bus_tags = futures::future::join_all(bus_tag_futs).await;
        for (b, tags) in buses.iter_mut().zip(bus_tags) {
            b.tags = tags;
        }

        // Default bus first so the bulk of rules comes from a familiar place.
        buses.sort_by(|a, b| {
            let rank = |k: &str| match k {
                "default" => 0,
                "custom" => 1,
                _ => 2,
            };
            rank(&a.kind)
                .cmp(&rank(&b.kind))
                .then_with(|| a.name.cmp(&b.name))
        });

        let bus_names: Vec<String> = buses.iter().map(|b| b.name.clone()).collect();

        if !buses.is_empty() {
            let batch: Vec<Box<dyn Resource>> = buses
                .into_iter()
                .map(|b| Box::new(b) as Box<dyn Resource>)
                .collect();
            total += batch.len();
            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                service: service_type,
                resources: batch,
                progress: LoadProgress {
                    loaded_count: total,
                    total_count: None,
                    status_message: Some("Loading event buses…".to_string()),
                },
            });
        }

        // ── Rules per bus ───────────────────────────────────────────────────
        for bus in bus_names {
            let mut rules: Vec<EbRule> = Vec::new();
            let mut rule_token: Option<String> = None;
            loop {
                let mut req = self.client.list_rules().event_bus_name(&bus);
                if let Some(t) = &rule_token {
                    req = req.next_token(t);
                }
                match req.send().await {
                    Ok(page) => {
                        for r in page.rules() {
                            rules.push(EbRule::from_sdk(r, &bus));
                        }
                        rule_token = crate::aws::pagination::next_page_token(page.next_token(), &rule_token);
                        if rule_token.is_none() {
                            break;
                        }
                    }
                    Err(e) => {
                        // Mid-stream failure: the bus batch already streamed, so a
                        // fatal error would silently drop every later batch (U9).
                        let _ = event_tx.send(Event::ResourceLoadWarning {
                            service: service_type,
                            warning: format!(
                                "rules ({}): {}",
                                bus,
                                crate::error::sdk_error_message(&e)
                            ),
                        });
                        rules.clear();
                        break;
                    }
                }
            }
            if rules.is_empty() {
                continue;
            }

            // Tags per rule, concurrently.
            let tag_futs = rules.iter().map(|r| {
                let client = self.client.clone();
                let arn = r.arn.clone();
                async move { fetch_tags(&client, &arn).await }
            });
            let rule_tags = futures::future::join_all(tag_futs).await;
            for (r, tags) in rules.iter_mut().zip(rule_tags) {
                r.tags = tags;
            }

            let batch: Vec<Box<dyn Resource>> = rules
                .into_iter()
                .map(|r| Box::new(r) as Box<dyn Resource>)
                .collect();
            total += batch.len();
            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                service: service_type,
                resources: batch,
                progress: LoadProgress {
                    loaded_count: total,
                    total_count: None,
                    status_message: Some(format!("Loading rules ({})…", bus)),
                },
            });
        }

        // ── Archives (error-tolerant batch) ─────────────────────────────────
        let mut archives: Vec<Box<dyn Resource>> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut req = self.client.list_archives();
            if let Some(t) = &token {
                req = req.next_token(t);
            }
            match req.send().await {
                Ok(page) => {
                    for a in page.archives() {
                        archives.push(Box::new(EbArchive::from_sdk(a)));
                    }
                    token = crate::aws::pagination::next_page_token(page.next_token(), &token);
                    if token.is_none() {
                        break;
                    }
                }
                Err(e) => {
                    let _ = event_tx.send(Event::ResourceLoadWarning {
                        service: service_type,
                        warning: format!("archives: {}", crate::error::sdk_error_message(&e)),
                    });
                    archives.clear();
                    break;
                }
            }
        }
        if !archives.is_empty() {
            total += archives.len();
            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                service: service_type,
                resources: archives,
                progress: LoadProgress {
                    loaded_count: total,
                    total_count: None,
                    status_message: Some("Loading archives…".to_string()),
                },
            });
        }

        // ── Replays (error-tolerant batch) ──────────────────────────────────
        let mut replays: Vec<Box<dyn Resource>> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut req = self.client.list_replays();
            if let Some(t) = &token {
                req = req.next_token(t);
            }
            match req.send().await {
                Ok(page) => {
                    for r in page.replays() {
                        replays.push(Box::new(EbReplay::from_sdk(r)));
                    }
                    token = crate::aws::pagination::next_page_token(page.next_token(), &token);
                    if token.is_none() {
                        break;
                    }
                }
                Err(e) => {
                    let _ = event_tx.send(Event::ResourceLoadWarning {
                        service: service_type,
                        warning: format!("replays: {}", crate::error::sdk_error_message(&e)),
                    });
                    replays.clear();
                    break;
                }
            }
        }
        if !replays.is_empty() {
            total += replays.len();
            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                service: service_type,
                resources: replays,
                progress: LoadProgress {
                    loaded_count: total,
                    total_count: None,
                    status_message: Some("Loading replays…".to_string()),
                },
            });
        }

        // ── Schedules (EventBridge Scheduler — separate client, all groups) ─
        let mut schedules: Vec<Box<dyn Resource>> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut req = self.scheduler.list_schedules();
            if let Some(t) = &token {
                req = req.next_token(t);
            }
            match req.send().await {
                Ok(page) => {
                    for s in page.schedules() {
                        schedules.push(Box::new(EbSchedule::from_sdk(s)));
                    }
                    token = crate::aws::pagination::next_page_token(page.next_token(), &token);
                    if token.is_none() {
                        break;
                    }
                }
                Err(e) => {
                    let _ = event_tx.send(Event::ResourceLoadWarning {
                        service: service_type,
                        warning: format!("schedules: {}", crate::error::sdk_error_message(&e)),
                    });
                    schedules.clear();
                    break;
                }
            }
        }
        if !schedules.is_empty() {
            total += schedules.len();
            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                service: service_type,
                resources: schedules,
                progress: LoadProgress {
                    loaded_count: total,
                    total_count: None,
                    status_message: Some("Loading schedules…".to_string()),
                },
            });
        }

        // ── Pipes (separate client) ─────────────────────────────────────────
        let mut pipes: Vec<Box<dyn Resource>> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut req = self.pipes.list_pipes();
            if let Some(t) = &token {
                req = req.next_token(t);
            }
            match req.send().await {
                Ok(page) => {
                    for p in page.pipes() {
                        pipes.push(Box::new(EbPipe::from_sdk(p)));
                    }
                    token = crate::aws::pagination::next_page_token(page.next_token(), &token);
                    if token.is_none() {
                        break;
                    }
                }
                Err(e) => {
                    let _ = event_tx.send(Event::ResourceLoadWarning {
                        service: service_type,
                        warning: format!("pipes: {}", crate::error::sdk_error_message(&e)),
                    });
                    pipes.clear();
                    break;
                }
            }
        }
        if !pipes.is_empty() {
            total += pipes.len();
            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                service: service_type,
                resources: pipes,
                progress: LoadProgress {
                    loaded_count: total,
                    total_count: None,
                    status_message: Some("Loading pipes…".to_string()),
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

// ── EbEventBus ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct EbEventBus {
    pub name: String,
    pub arn: String,
    pub kind: String, // default / custom / partner
    pub policy: Option<String>,
    pub tags: HashMap<String, String>,
}

impl EbEventBus {
    pub fn from_sdk(b: &aws_sdk_eventbridge::types::EventBus) -> Self {
        let name = b.name().unwrap_or_default().to_string();
        let arn = b.arn().unwrap_or_default().to_string();
        let kind = if name == "default" {
            "default".to_string()
        } else if name.contains("aws.partner") || arn.contains("aws.partner") {
            "partner".to_string()
        } else {
            "custom".to_string()
        };
        Self {
            name,
            arn,
            kind,
            policy: b.policy().map(|p| p.to_string()).filter(|p| !p.is_empty()),
            tags: HashMap::new(),
        }
    }

    /// Pretty-printed resource policy (for the Permissions section / `e`), if any.
    pub fn pretty_policy(&self) -> Option<String> {
        self.policy.as_ref().map(|p| pretty_json(p))
    }
}

crate::sections! {
    pub enum EbEventBusDetailSection,
    pub static EB_EVENT_BUS_SECTIONS = [
        Overview "Overview",
        Permissions "Permissions",
        Tags "Tags",
    ]
}

impl Resource for EbEventBus {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&EB_EVENT_BUS_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.arn
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn resource_type(&self) -> &str {
        "Event Bus"
    }

    fn state(&self) -> ResourceState {
        ResourceState::stateless()
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!("{} {} {}", self.name, self.kind, self.arn)
    }

    fn details(&self) -> Vec<(String, String)> {
        let mut rows = vec![
            ("Name".to_string(), self.name.clone()),
            ("Type".to_string(), self.kind.clone()),
            ("ARN".to_string(), self.arn.clone()),
            (
                "Resource Policy".to_string(),
                if self.policy.is_some() {
                    "✓ set (e to edit)".to_string()
                } else {
                    "none".to_string()
                },
            ),
        ];
        if !self.tags.is_empty() {
            rows.push((String::new(), String::new()));
            rows.push(("Tags".to_string(), String::new()));
            let mut sorted: Vec<_> = self.tags.iter().collect();
            sorted.sort_by_key(|(k, _)| k.as_str());
            for (k, v) in sorted {
                rows.push((format!("  {}", k), v.clone()));
            }
        }
        rows
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/events/home?region={}#/eventbus/{}",
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

// ── EbRule ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct EbRule {
    pub name: String,
    pub arn: String,
    pub bus_name: String,
    pub state: String, // ENABLED / DISABLED
    pub description: String,
    pub schedule: Option<String>,
    pub event_pattern: Option<String>,
    pub managed_by: Option<String>,
    pub tags: HashMap<String, String>,
}

impl EbRule {
    pub fn from_sdk(r: &aws_sdk_eventbridge::types::Rule, bus: &str) -> Self {
        Self {
            name: r.name().unwrap_or_default().to_string(),
            arn: r.arn().unwrap_or_default().to_string(),
            bus_name: bus.to_string(),
            state: r.state().map(|s| s.as_str().to_string()).unwrap_or_default(),
            description: r.description().unwrap_or_default().to_string(),
            schedule: r
                .schedule_expression()
                .map(|s| s.to_string())
                .filter(|s| !s.is_empty()),
            event_pattern: r
                .event_pattern()
                .map(|s| s.to_string())
                .filter(|s| !s.is_empty()),
            managed_by: r
                .managed_by()
                .map(|s| s.to_string())
                .filter(|s| !s.is_empty()),
            tags: HashMap::new(),
        }
    }

    /// Pretty-printed event pattern (for the `v` viewer), if any.
    pub fn pretty_pattern(&self) -> Option<String> {
        self.event_pattern.as_ref().map(|p| pretty_json(p))
    }

    /// From `DescribeRule` — used by the Lambda-side reverse lookup
    /// (`fetch_lambda_eventbridge_rules`), which only gets rule *names* out of
    /// `ListRuleNamesByTarget` and needs a second call for the full rule.
    pub fn from_describe(
        bus: &str,
        r: &aws_sdk_eventbridge::operation::describe_rule::DescribeRuleOutput,
    ) -> Self {
        Self {
            name: r.name().unwrap_or_default().to_string(),
            arn: r.arn().unwrap_or_default().to_string(),
            bus_name: bus.to_string(),
            state: r.state().map(|s| s.as_str().to_string()).unwrap_or_default(),
            description: r.description().unwrap_or_default().to_string(),
            schedule: r
                .schedule_expression()
                .map(|s| s.to_string())
                .filter(|s| !s.is_empty()),
            event_pattern: r
                .event_pattern()
                .map(|s| s.to_string())
                .filter(|s| !s.is_empty()),
            managed_by: r
                .managed_by()
                .map(|s| s.to_string())
                .filter(|s| !s.is_empty()),
            tags: HashMap::new(),
        }
    }
}

crate::sections! {
    pub enum EbRuleDetailSection,
    pub static EB_RULE_SECTIONS = [
        Trigger "Trigger",
        Targets "Targets" => crate::app::App::trigger_eb_targets_load,
        Tags "Tags",
    ]
}

impl Resource for EbRule {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&EB_RULE_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.arn
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn resource_type(&self) -> &str {
        "EventBridge Rule"
    }

    fn state(&self) -> ResourceState {
        match self.state.as_str() {
            "ENABLED" => ResourceState::Available,
            "DISABLED" => ResourceState::Stopped,
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
        format!(
            "{} {} {} {}",
            self.name, self.bus_name, self.description, self.arn,
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        // Fallback only — the split pane (eb_rule_section_lines) is the real view.
        vec![
            ("Name".to_string(), self.name.clone()),
            ("State".to_string(), self.state.clone()),
            ("Bus".to_string(), self.bus_name.clone()),
        ]
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/events/home?region={}#/eventbus/{}/rules/{}",
            region, region, self.bus_name, self.name
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── Target (rendered lazily in the split pane) ────────────────────────────────

#[derive(Debug, Clone)]
pub struct EbTarget {
    pub id: String,
    pub arn: String,
    pub kind: String, // Lambda / SQS / SNS / Step Functions / ECS / …
    pub input_summary: String,
    pub dead_letter: Option<String>,
}

/// Targets are per (bus, rule) — fetched on first view of the Targets section.
pub async fn fetch_eb_targets(
    client: EbClient,
    bus: String,
    rule: String,
) -> Result<Vec<EbTarget>> {
    let resp = client
        .list_targets_by_rule()
        .event_bus_name(&bus)
        .rule(&rule)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;

    Ok(resp
        .targets()
        .iter()
        .map(|t| {
            let arn = t.arn().to_string();
            let input_summary = if let Some(input) = t.input() {
                let one = input.replace('\n', " ");
                truncate(&one, 60)
            } else if t.input_path().is_some() {
                format!("InputPath: {}", t.input_path().unwrap_or_default())
            } else if t.input_transformer().is_some() {
                "input transformer".to_string()
            } else {
                "(matched event)".to_string()
            };
            EbTarget {
                id: t.id().to_string(),
                kind: arn_service_label(&arn),
                dead_letter: t
                    .dead_letter_config()
                    .and_then(|d| d.arn())
                    .map(|s| s.to_string()),
                arn,
                input_summary,
            }
        })
        .collect())
}

async fn fetch_tags(client: &EbClient, arn: &str) -> HashMap<String, String> {
    if arn.is_empty() {
        return HashMap::new();
    }
    match client.list_tags_for_resource().resource_arn(arn).send().await {
        Ok(resp) => resp
            .tags()
            .iter()
            .map(|t| (t.key().to_string(), t.value().to_string()))
            .collect(),
        Err(_) => HashMap::new(),
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Human label for the AWS service an ARN points at (the target's downstream).
fn arn_service_label(arn: &str) -> String {
    // arn:aws:<service>:<region>:<acct>:<resource>
    let service = arn.split(':').nth(2).unwrap_or("");
    match service {
        "lambda" => "Lambda",
        "sqs" => "SQS",
        "sns" => "SNS",
        "states" => "Step Functions",
        "ecs" => "ECS",
        "events" => "EventBridge",
        "kinesis" => "Kinesis",
        "firehose" => "Firehose",
        "logs" => "CloudWatch Logs",
        "ssm" => "SSM",
        "codebuild" => "CodeBuild",
        "codepipeline" => "CodePipeline",
        "" => "—",
        other => other,
    }
    .to_string()
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() > max {
        format!("{}…", s.chars().take(max).collect::<String>())
    } else {
        s.to_string()
    }
}

fn pretty_json(s: &str) -> String {
    serde_json::from_str::<serde_json::Value>(s)
        .ok()
        .and_then(|v| serde_json::to_string_pretty(&v).ok())
        .unwrap_or_else(|| s.to_string())
}

// ── EbArchive ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct EbArchive {
    pub name: String,
    pub event_source_arn: String, // the archived bus's ARN — jumpable
    pub state: String,            // ENABLED / DISABLED / CREATING / UPDATING / *_FAILED
    pub state_reason: Option<String>,
    pub retention_days: i32, // 0 = indefinite
    pub size_bytes: i64,
    pub event_count: i64,
    pub created: Option<String>,
    tags: HashMap<String, String>, // ListArchives returns none
}

impl EbArchive {
    pub fn from_sdk(a: &aws_sdk_eventbridge::types::Archive) -> Self {
        Self {
            name: a.archive_name().unwrap_or_default().to_string(),
            event_source_arn: a.event_source_arn().unwrap_or_default().to_string(),
            state: a
                .state()
                .map(|s| s.as_str().to_string())
                .unwrap_or_default(),
            state_reason: a
                .state_reason()
                .map(|s| s.to_string())
                .filter(|s| !s.is_empty()),
            retention_days: a.retention_days().unwrap_or(0),
            size_bytes: a.size_bytes(),
            event_count: a.event_count(),
            created: a.creation_time().map(fmt_ts),
            tags: HashMap::new(),
        }
    }
}

impl Resource for EbArchive {
    fn id(&self) -> &str {
        &self.name
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn resource_type(&self) -> &str {
        "Archive"
    }

    fn state(&self) -> ResourceState {
        match self.state.as_str() {
            "ENABLED" => ResourceState::Available,
            "DISABLED" => ResourceState::Stopped,
            "CREATING" | "UPDATING" => ResourceState::Creating,
            "CREATE_FAILED" | "UPDATE_FAILED" => ResourceState::Unavailable,
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
        format!("{} {} {}", self.name, self.state, self.event_source_arn)
    }

    fn details(&self) -> Vec<(String, String)> {
        let mut rows = vec![
            ("Name".to_string(), self.name.clone()),
            ("State".to_string(), self.state.clone()),
        ];
        if let Some(reason) = &self.state_reason {
            rows.push(("State Reason".to_string(), format!("⚠ {}", reason)));
        }
        rows.push(("Source Bus".to_string(), self.event_source_arn.clone()));
        rows.push((
            "Retention".to_string(),
            if self.retention_days == 0 {
                "indefinite".to_string()
            } else {
                format!("{} days", self.retention_days)
            },
        ));
        rows.push(("Size".to_string(), fmt_bytes(self.size_bytes)));
        rows.push((
            "Event Count".to_string(),
            format!("{}", self.event_count),
        ));
        if let Some(c) = &self.created {
            rows.push(("Created".to_string(), c.clone()));
        }
        rows
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/events/home?region={}#/archives/{}",
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

// ── EbReplay ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct EbReplay {
    pub name: String,
    pub event_source_arn: String, // the archive replayed from
    pub state: String, // STARTING / RUNNING / CANCELLING / COMPLETED / CANCELLED / FAILED
    pub state_reason: Option<String>,
    pub event_start: Option<String>,
    pub event_end: Option<String>,
    pub last_replayed: Option<String>,
    pub replay_start: Option<String>,
    pub replay_end: Option<String>,
    tags: HashMap<String, String>,
}

impl EbReplay {
    pub fn from_sdk(r: &aws_sdk_eventbridge::types::Replay) -> Self {
        Self {
            name: r.replay_name().unwrap_or_default().to_string(),
            event_source_arn: r.event_source_arn().unwrap_or_default().to_string(),
            state: r
                .state()
                .map(|s| s.as_str().to_string())
                .unwrap_or_default(),
            state_reason: r
                .state_reason()
                .map(|s| s.to_string())
                .filter(|s| !s.is_empty()),
            event_start: r.event_start_time().map(fmt_ts),
            event_end: r.event_end_time().map(fmt_ts),
            last_replayed: r.event_last_replayed_time().map(fmt_ts),
            replay_start: r.replay_start_time().map(fmt_ts),
            replay_end: r.replay_end_time().map(fmt_ts),
            tags: HashMap::new(),
        }
    }
}

impl Resource for EbReplay {
    fn id(&self) -> &str {
        &self.name
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn resource_type(&self) -> &str {
        "Replay"
    }

    fn state(&self) -> ResourceState {
        match self.state.as_str() {
            "COMPLETED" => ResourceState::Available,
            "RUNNING" => ResourceState::Running,
            "STARTING" => ResourceState::Pending,
            "CANCELLING" => ResourceState::Deleting,
            "CANCELLED" => ResourceState::Stopped,
            "FAILED" => ResourceState::Unavailable,
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
        format!("{} {} {}", self.name, self.state, self.event_source_arn)
    }

    fn details(&self) -> Vec<(String, String)> {
        let mut rows = vec![
            ("Name".to_string(), self.name.clone()),
            ("State".to_string(), self.state.clone()),
        ];
        if let Some(reason) = &self.state_reason {
            rows.push(("State Reason".to_string(), format!("⚠ {}", reason)));
        }
        rows.push(("Source Archive".to_string(), self.event_source_arn.clone()));
        rows.push((String::new(), String::new()));
        rows.push(("Event Window".to_string(), String::new()));
        rows.push((
            "  From".to_string(),
            self.event_start.clone().unwrap_or_else(|| "—".to_string()),
        ));
        rows.push((
            "  To".to_string(),
            self.event_end.clone().unwrap_or_else(|| "—".to_string()),
        ));
        if let Some(last) = &self.last_replayed {
            rows.push(("  Last Replayed".to_string(), last.clone()));
        }
        rows.push((String::new(), String::new()));
        rows.push(("Run".to_string(), String::new()));
        rows.push((
            "  Started".to_string(),
            self.replay_start.clone().unwrap_or_else(|| "—".to_string()),
        ));
        rows.push((
            "  Ended".to_string(),
            self.replay_end.clone().unwrap_or_else(|| "—".to_string()),
        ));
        rows
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/events/home?region={}#/replays/{}",
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

// ── EbSchedule (EventBridge Scheduler) ────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct EbSchedule {
    pub name: String,
    pub arn: String,
    pub group: String,
    pub state: String, // ENABLED / DISABLED
    pub target_arn: Option<String>,
    pub created: Option<String>,
    pub modified: Option<String>,
    tags: HashMap<String, String>, // ListSchedules returns none
}

impl EbSchedule {
    pub fn from_sdk(s: &aws_sdk_scheduler::types::ScheduleSummary) -> Self {
        Self {
            name: s.name().unwrap_or_default().to_string(),
            arn: s.arn().unwrap_or_default().to_string(),
            group: s.group_name().unwrap_or("default").to_string(),
            state: s
                .state()
                .map(|st| st.as_str().to_string())
                .unwrap_or_default(),
            target_arn: s.target().map(|t| t.arn().to_string()),
            created: s.creation_date().map(fmt_ts),
            modified: s.last_modification_date().map(fmt_ts),
            tags: HashMap::new(),
        }
    }
}

crate::sections! {
    pub enum EbScheduleDetailSection,
    pub static EB_SCHEDULE_SECTIONS = [
        Overview "Overview" => crate::app::App::trigger_eb_schedule_detail_load,
        Target "Target" => crate::app::App::trigger_eb_schedule_detail_load,
    ]
}

impl Resource for EbSchedule {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&EB_SCHEDULE_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.arn
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn resource_type(&self) -> &str {
        "Schedule"
    }

    fn state(&self) -> ResourceState {
        match self.state.as_str() {
            "ENABLED" => ResourceState::Available,
            "DISABLED" => ResourceState::Stopped,
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
        format!(
            "{} {} {} {}",
            self.name,
            self.group,
            self.target_arn.as_deref().unwrap_or(""),
            self.arn,
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        // Fallback only — the split pane (eb_schedule_section_lines) is the real view.
        vec![
            ("Name".to_string(), self.name.clone()),
            ("Group".to_string(), self.group.clone()),
            ("State".to_string(), self.state.clone()),
        ]
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/scheduler/home?region={}#schedules/{}/{}",
            region, region, self.group, self.name
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// The `GetSchedule` payload backing both split-pane sections (Overview needs
/// the cron/rate expression, which `ListSchedules` summaries don't carry).
#[derive(Debug, Clone)]
pub struct EbScheduleDetail {
    pub expression: String,
    pub timezone: Option<String>,
    pub description: Option<String>,
    pub window_mode: String, // OFF / FLEXIBLE
    pub window_minutes: Option<i32>,
    pub start_date: Option<String>,
    pub end_date: Option<String>,
    pub kms_key_arn: Option<String>,
    pub after_completion: Option<String>, // NONE / DELETE
    pub target_arn: Option<String>,
    pub role_arn: Option<String>,
    pub input: Option<String>,
    pub retry_max_attempts: Option<i32>,
    pub retry_max_age_secs: Option<i32>,
    pub dead_letter_arn: Option<String>,
}

pub async fn fetch_schedule_detail(
    client: aws_sdk_scheduler::Client,
    group: String,
    name: String,
) -> Result<EbScheduleDetail> {
    let resp = client
        .get_schedule()
        .group_name(&group)
        .name(&name)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;

    let target = resp.target();
    Ok(EbScheduleDetail {
        expression: resp.schedule_expression().unwrap_or_default().to_string(),
        timezone: resp
            .schedule_expression_timezone()
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty()),
        description: resp
            .description()
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty()),
        window_mode: resp
            .flexible_time_window()
            .map(|w| w.mode().as_str().to_string())
            .unwrap_or_default(),
        window_minutes: resp
            .flexible_time_window()
            .and_then(|w| w.maximum_window_in_minutes()),
        start_date: resp.start_date().map(fmt_ts),
        end_date: resp.end_date().map(fmt_ts),
        kms_key_arn: resp.kms_key_arn().map(|s| s.to_string()),
        after_completion: resp
            .action_after_completion()
            .map(|a| a.as_str().to_string()),
        target_arn: target.map(|t| t.arn().to_string()),
        role_arn: target.map(|t| t.role_arn().to_string()),
        input: target
            .and_then(|t| t.input())
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty()),
        retry_max_attempts: target
            .and_then(|t| t.retry_policy())
            .and_then(|r| r.maximum_retry_attempts()),
        retry_max_age_secs: target
            .and_then(|t| t.retry_policy())
            .and_then(|r| r.maximum_event_age_in_seconds()),
        dead_letter_arn: target
            .and_then(|t| t.dead_letter_config())
            .and_then(|d| d.arn())
            .map(|s| s.to_string()),
    })
}

// ── EbPipe ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct EbPipe {
    pub name: String,
    pub arn: String,
    pub current_state: String, // RUNNING / STOPPED / *_FAILED / …
    pub desired_state: String,
    pub state_reason: Option<String>,
    pub source: String,
    pub enrichment: Option<String>,
    pub target: String,
    pub created: Option<String>,
    pub modified: Option<String>,
    tags: HashMap<String, String>, // arrive with the lazy DescribePipe
}

impl EbPipe {
    pub fn from_sdk(p: &aws_sdk_pipes::types::Pipe) -> Self {
        Self {
            name: p.name().unwrap_or_default().to_string(),
            arn: p.arn().unwrap_or_default().to_string(),
            current_state: p
                .current_state()
                .map(|s| s.as_str().to_string())
                .unwrap_or_default(),
            desired_state: p
                .desired_state()
                .map(|s| s.as_str().to_string())
                .unwrap_or_default(),
            state_reason: p
                .state_reason()
                .map(|s| s.to_string())
                .filter(|s| !s.is_empty()),
            source: p.source().unwrap_or_default().to_string(),
            enrichment: p
                .enrichment()
                .map(|s| s.to_string())
                .filter(|s| !s.is_empty()),
            target: p.target().unwrap_or_default().to_string(),
            created: p.creation_time().map(fmt_ts),
            modified: p.last_modified_time().map(fmt_ts),
            tags: HashMap::new(),
        }
    }
}

crate::sections! {
    pub enum EbPipeDetailSection,
    pub static EB_PIPE_SECTIONS = [
        Overview "Overview",
        Configuration "Configuration" => crate::app::App::trigger_eb_pipe_detail_load,
        Tags "Tags" => crate::app::App::trigger_eb_pipe_detail_load,
    ]
}

impl Resource for EbPipe {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&EB_PIPE_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.arn
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn resource_type(&self) -> &str {
        "Pipe"
    }

    fn state(&self) -> ResourceState {
        match self.current_state.as_str() {
            "RUNNING" => ResourceState::Running,
            "STOPPED" => ResourceState::Stopped,
            "CREATING" | "STARTING" | "UPDATING" => ResourceState::Creating,
            "STOPPING" | "DELETING" => ResourceState::Deleting,
            s if s.ends_with("_FAILED") => ResourceState::Unavailable,
            other => ResourceState::Unknown(other.to_string()),
        }
    }

    fn state_label(&self) -> String {
        native_state_label(&self.current_state, || self.state())
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!(
            "{} {} {} {} {}",
            self.name, self.current_state, self.source, self.target, self.arn,
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        // Fallback only — the split pane (eb_pipe_section_lines) is the real view.
        vec![
            ("Name".to_string(), self.name.clone()),
            ("State".to_string(), self.current_state.clone()),
            ("Source".to_string(), self.source.clone()),
            ("Target".to_string(), self.target.clone()),
        ]
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/events/home?region={}#/pipes/{}",
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

/// The `DescribePipe` payload backing the Configuration + Tags sections.
#[derive(Debug, Clone)]
pub struct EbPipeDetail {
    pub description: Option<String>,
    pub role_arn: Option<String>,
    pub kms_key: Option<String>,
    pub filter_patterns: Vec<String>,
    pub log_level: Option<String>,
    pub log_destination: Option<String>,
    pub tags: HashMap<String, String>,
}

pub async fn fetch_pipe_detail(
    client: aws_sdk_pipes::Client,
    name: String,
) -> Result<EbPipeDetail> {
    let resp = client
        .describe_pipe()
        .name(&name)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;

    let filter_patterns = resp
        .source_parameters()
        .and_then(|sp| sp.filter_criteria())
        .map(|fc| {
            fc.filters()
                .iter()
                .filter_map(|f| f.pattern())
                .map(|p| p.to_string())
                .collect()
        })
        .unwrap_or_default();

    // One human-readable line for wherever pipe execution logs land.
    let log_destination = resp.log_configuration().and_then(|lc| {
        lc.cloudwatch_logs_log_destination()
            .and_then(|d| d.log_group_arn())
            .map(|a| a.to_string())
            .or_else(|| {
                lc.s3_log_destination()
                    .and_then(|d| d.bucket_name())
                    .map(|b| format!("s3://{}", b))
            })
            .or_else(|| {
                lc.firehose_log_destination()
                    .and_then(|d| d.delivery_stream_arn())
                    .map(|a| a.to_string())
            })
    });

    Ok(EbPipeDetail {
        description: resp
            .description()
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty()),
        role_arn: resp.role_arn().map(|s| s.to_string()),
        kms_key: resp
            .kms_key_identifier()
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty()),
        filter_patterns,
        log_level: resp
            .log_configuration()
            .and_then(|lc| lc.level())
            .map(|l| l.as_str().to_string()),
        log_destination,
        tags: resp
            .tags()
            .map(|t| {
                t.iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect()
            })
            .unwrap_or_default(),
    })
}

fn fmt_ts(t: &aws_smithy_types::DateTime) -> String {
    t.fmt(aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default()
}

fn fmt_bytes(b: i64) -> String {
    const KB: f64 = 1024.0;
    let b = b as f64;
    if b >= KB * KB * KB {
        format!("{:.1} GiB", b / (KB * KB * KB))
    } else if b >= KB * KB {
        format!("{:.1} MiB", b / (KB * KB))
    } else if b >= KB {
        format!("{:.1} KiB", b / KB)
    } else {
        format!("{} B", b as i64)
    }
}

// ── Rule CloudWatch metrics (`m`) — AWS/Events, dim RuleName ──────────────────

use crate::aws::services::ec2::{parse_metric_datapoints, MetricsTimeRange};

#[derive(Debug, Clone)]
pub struct EbRuleMetricsData {
    pub time_range: MetricsTimeRange,
    pub triggered: Vec<(f64, f64)>,
    pub invocations: Vec<(f64, f64)>,
    pub failed_invocations: Vec<(f64, f64)>,
    pub dlq_invocations: Vec<(f64, f64)>,
    pub throttled: Vec<(f64, f64)>,
    pub x_max: f64,
}

#[derive(Debug, Clone)]
pub enum EbRuleMetricsState {
    Loading,
    Loaded(EbRuleMetricsData),
}

/// Per-rule delivery health: TriggeredRules is "did the rule match anything",
/// Invocations is "did it call its targets", and FailedInvocations /
/// DeadLetterInvocations / ThrottledRules are the three ways delivery goes
/// wrong (failed outright / shunted to the DLQ / rate-limited).
pub async fn fetch_eb_rule_metrics(
    cw: aws_sdk_cloudwatch::Client,
    rule_name: String,
    time_range: MetricsTimeRange,
) -> crate::error::Result<EbRuleMetricsData> {
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
            .namespace("AWS/Events")
            .metric_name(name)
            .dimensions(Dimension::builder().name("RuleName").value(&rule_name).build())
            .start_time(start_dt)
            .end_time(end_dt)
            .period(period)
            .set_statistics(Some(vec![Statistic::Sum]))
            .send()
    };

    let (triggered, invocations, failed, dlq, throttled) = tokio::join!(
        metric("TriggeredRules"),
        metric("Invocations"),
        metric("FailedInvocations"),
        metric("DeadLetterInvocations"),
        metric("ThrottledRules"),
    );

    Ok(EbRuleMetricsData {
        time_range,
        triggered: parse_metric_datapoints(triggered, start),
        invocations: parse_metric_datapoints(invocations, start),
        failed_invocations: parse_metric_datapoints(failed, start),
        dlq_invocations: parse_metric_datapoints(dlq, start),
        throttled: parse_metric_datapoints(throttled, start),
        x_max: time_range.duration_secs() as f64,
    })
}
