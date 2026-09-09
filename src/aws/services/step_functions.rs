use crate::aws::client::AwsClients;
use crate::aws::resource::{native_state_label, Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use aws_sdk_sfn::Client as SfnClient;
use futures::stream::StreamExt;
use std::any::Any;
use std::collections::HashMap;
use tokio::sync::mpsc;

/// How many state machines get an `ListExecutions` call in the execution phase.
/// Executions are the reason to open this service, so the cap is generous; an
/// account past it reports the overflow as a load warning.
const MAX_EXECUTION_MACHINES: usize = 100;
/// Executions listed per state machine (newest first).
const MAX_EXECUTIONS_PER_MACHINE: i32 = 25;
/// History events kept for one execution. The fetch runs in **reverse order**
/// so a cap keeps the newest events — which is where the failure lives.
const MAX_HISTORY_EVENTS: usize = 1000;

pub struct StepFunctionsService {
    client: SfnClient,
}

impl StepFunctionsService {
    pub fn new(aws_clients: &AwsClients) -> Self {
        Self {
            client: aws_clients.sfn_client(),
        }
    }
}

#[async_trait]
impl AwsService for StepFunctionsService {
    fn service_type(&self) -> ServiceType {
        ServiceType::StepFunctions
    }

    fn name(&self) -> &str {
        "Step Functions"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        self.list_resources_streaming(tx, ServiceType::StepFunctions)
            .await?;
        Ok(vec![])
    }

    async fn list_resources_streaming(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        let mut total = 0usize;
        // Machines kept for the execution phase: (arn, name). EXPRESS machines
        // are skipped — `ListExecutions` doesn't retain their executions.
        let mut standard: Vec<(String, String)> = Vec::new();

        let mut paginator = self.client.list_state_machines().into_paginator().send();

        while let Some(result) = paginator.next().await {
            match result {
                Ok(page) => {
                    let machines: Vec<SfnStateMachine> = page
                        .state_machines()
                        .iter()
                        .map(SfnStateMachine::from_summary)
                        .collect();

                    for m in &machines {
                        if !m.is_express() {
                            standard.push((m.arn.clone(), m.name.clone()));
                        }
                    }

                    let count = machines.len();
                    if count == 0 {
                        continue;
                    }
                    total += count;

                    let batch: Vec<Box<dyn Resource>> = machines
                        .into_iter()
                        .map(|m| Box::new(m) as Box<dyn Resource>)
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
                Err(e) => {
                    let _ = event_tx.send(Event::ResourceLoadError {
                        service: service_type,
                        error: format!(
                            "Failed to list state machines: {}",
                            crate::error::sdk_error_message(&e)
                        ),
                    });
                    return Ok(());
                }
            }
        }

        // ── Phase 1: executions (best effort — a permission gap on
        // ListExecutions must not break the state-machine list). ─────────────
        let overflow = standard.len().saturating_sub(MAX_EXECUTION_MACHINES);
        standard.truncate(MAX_EXECUTION_MACHINES);
        if overflow > 0 {
            let _ = event_tx.send(Event::ResourceLoadWarning {
                service: service_type,
                warning: format!(
                    "executions listed for the first {} state machines ({} not scanned)",
                    MAX_EXECUTION_MACHINES, overflow
                ),
            });
        }

        let mut failures = 0usize;
        let mut stream = futures::stream::iter(standard.into_iter().map(|(arn, name)| {
            let client = self.client.clone();
            async move {
                let res = fetch_executions(client, arn, name.clone()).await;
                (name, res)
            }
        }))
        .buffer_unordered(8);

        while let Some((_name, res)) = stream.next().await {
            match res {
                Ok(execs) if !execs.is_empty() => {
                    total += execs.len();
                    let batch: Vec<Box<dyn Resource>> = execs
                        .into_iter()
                        .map(|e| Box::new(e) as Box<dyn Resource>)
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
                Ok(_) => {}
                Err(_) => failures += 1,
            }
        }

        if failures > 0 {
            let _ = event_tx.send(Event::ResourceLoadWarning {
                service: service_type,
                warning: format!("failed to list executions for {} state machine(s)", failures),
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

// ── SfnStateMachine ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct SfnStateMachine {
    pub arn: String,
    pub name: String,
    pub kind: String, // STANDARD / EXPRESS
    pub created: Option<String>,
    pub tags: HashMap<String, String>,
}

impl SfnStateMachine {
    pub fn from_summary(sm: &aws_sdk_sfn::types::StateMachineListItem) -> Self {
        Self {
            arn: sm.state_machine_arn().to_string(),
            name: sm.name().to_string(),
            kind: sm.r#type().as_str().to_string(),
            created: Some(fmt_epoch_secs(sm.creation_date().secs())),
            tags: HashMap::new(),
        }
    }

    /// Map a state-machine type string to a friendly display label.
    pub fn type_label(kind: &str) -> &'static str {
        match kind {
            "STANDARD" => "Standard",
            "EXPRESS" => "Express",
            _ => "Unknown",
        }
    }

    /// EXPRESS state machines do not retain executions for `ListExecutions`.
    pub fn is_express(&self) -> bool {
        self.kind == "EXPRESS"
    }
}

crate::sections! {
    pub enum SfnDetailSection,
    pub static SFN_SECTIONS = [
        Details "Details" => crate::app::App::trigger_sfn_details_load,
        Definition "Definition" => crate::app::App::trigger_sfn_details_load,
        Executions "Executions",
        Tags "Tags" => crate::app::App::trigger_sfn_details_load,
    ]
}

impl Resource for SfnStateMachine {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&SFN_SECTIONS)
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws stepfunctions describe-state-machine --state-machine-arn {}",
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
        "State Machine"
    }

    fn state(&self) -> ResourceState {
        // The list call doesn't return status; treat listed machines as available.
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
                Self::type_label(&self.kind).to_string(),
            ),
            (
                "Created".to_string(),
                self.created.clone().unwrap_or_else(|| "—".to_string()),
            ),
        ]
    }

    fn console_url(&self, _region: &str) -> Option<String> {
        // arn:aws:states:<region>:<account>:stateMachine:<name>
        let parts: Vec<&str> = self.arn.split(':').collect();
        let region = parts.get(3).copied().unwrap_or("us-east-1");
        Some(format!(
            "https://{}.console.aws.amazon.com/states/home?region={}#/statemachines/view/{}",
            region, region, self.arn
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── SfnExecution ───────────────────────────────────────────────────────────────

/// One state-machine execution — a first-class resource so a running or failed
/// workflow can be found, filtered and drilled into from the Executions
/// sub-tab without first locating its state machine.
#[derive(Debug, Clone)]
pub struct SfnExecution {
    pub name: String,
    pub arn: String,
    pub status: String, // RUNNING / SUCCEEDED / FAILED / TIMED_OUT / ABORTED
    pub state_machine_arn: String,
    pub state_machine_name: String,
    /// `<state machine> / <execution>` — the list row, since the Executions
    /// sub-tab spans every state machine.
    pub display_name: String,
    pub started: Option<String>,
    pub stopped: Option<String>,
    pub started_secs: i64,
    pub stopped_secs: Option<i64>,
    /// Set on a distributed-map child execution.
    pub map_run_arn: Option<String>,
    pub item_count: Option<i32>,
    pub redrive_count: Option<i32>,
    pub tags: HashMap<String, String>,
}

impl SfnExecution {
    pub fn from_list_item(
        e: &aws_sdk_sfn::types::ExecutionListItem,
        state_machine_name: &str,
    ) -> Self {
        let name = e.name().to_string();
        Self {
            display_name: format!("{} / {}", state_machine_name, name),
            name,
            arn: e.execution_arn().to_string(),
            status: e.status().as_str().to_string(),
            state_machine_arn: e.state_machine_arn().to_string(),
            state_machine_name: state_machine_name.to_string(),
            started: Some(fmt_epoch_secs(e.start_date().secs())),
            stopped: e.stop_date().map(|d| fmt_epoch_secs(d.secs())),
            started_secs: e.start_date().secs(),
            stopped_secs: e.stop_date().map(|d| d.secs()),
            map_run_arn: e.map_run_arn().map(|s| s.to_string()),
            item_count: e.item_count(),
            redrive_count: e.redrive_count(),
            tags: HashMap::new(),
        }
    }

    pub fn is_running(&self) -> bool {
        self.status == "RUNNING"
    }

    pub fn failed(&self) -> bool {
        matches!(self.status.as_str(), "FAILED" | "TIMED_OUT" | "ABORTED")
    }

    /// Wall-clock duration — for a running execution, elapsed so far.
    pub fn duration_secs(&self) -> Option<i64> {
        let end = self.stopped_secs.unwrap_or_else(now_secs);
        (end >= self.started_secs).then_some(end - self.started_secs)
    }

    pub fn duration_label(&self) -> String {
        match self.duration_secs() {
            Some(d) if self.stopped_secs.is_none() => format!("{} (running)", fmt_duration(d)),
            Some(d) => fmt_duration(d),
            None => "—".to_string(),
        }
    }
}

crate::sections! {
    pub enum SfnExecDetailSection,
    pub static SFN_EXEC_SECTIONS = [
        Overview "Overview" => crate::app::App::trigger_sfn_execution_load,
        Input "Input" => crate::app::App::trigger_sfn_execution_load,
        Output "Output" => crate::app::App::trigger_sfn_execution_load,
        History "History" => crate::app::App::trigger_sfn_history_load,
    ]
}

impl Resource for SfnExecution {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&SFN_EXEC_SECTIONS)
    }

    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws stepfunctions describe-execution --execution-arn {}",
            crate::aws::resource::shell_quote(self.id())
        ))
    }

    fn id(&self) -> &str {
        &self.arn
    }

    // Deliberately not `self.name`: the Executions sub-tab spans every state
    // machine, so a bare execution name (often a UUID) doesn't identify the row.
    #[allow(clippy::misnamed_getters)]
    fn name(&self) -> &str {
        &self.display_name
    }

    fn resource_type(&self) -> &str {
        "State Machine Execution"
    }

    fn state(&self) -> ResourceState {
        execution_status_state(&self.status)
    }

    fn state_label(&self) -> String {
        native_state_label(&self.status, || self.state())
    }

    /// A succeeded execution is the routine case — `a` hides them so the
    /// running and failed workflows are all that's left.
    fn is_noise(&self) -> bool {
        self.status == "SUCCEEDED"
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!(
            "{} {} {} {} {}",
            self.name, self.state_machine_name, self.arn, self.status, self.state_machine_arn
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Name".to_string(), self.name.clone()),
            ("Status".to_string(), self.status.clone()),
            ("ARN".to_string(), self.arn.clone()),
        ]
    }

    fn console_url(&self, _region: &str) -> Option<String> {
        let parts: Vec<&str> = self.arn.split(':').collect();
        let region = parts.get(3).copied().unwrap_or("us-east-1");
        Some(format!(
            "https://{}.console.aws.amazon.com/states/home?region={}#/v2/executions/details/{}",
            region, region, self.arn
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Map an execution status string to a `ResourceState` so the detail rows can
/// be coloured (failed/timed-out/aborted → red).
pub fn execution_status_state(status: &str) -> ResourceState {
    match status {
        "SUCCEEDED" => ResourceState::Available,
        "RUNNING" => ResourceState::Pending,
        "FAILED" | "TIMED_OUT" | "ABORTED" => ResourceState::Unavailable,
        other => ResourceState::Unknown(other.to_string()),
    }
}

// ── Lazy: describe state machine (config + definition) ─────────────────────────

/// Config + definition fetched together from a single `DescribeStateMachine`.
#[derive(Debug, Clone)]
pub struct SfnDetails {
    pub role_arn: String,
    pub kind: String,
    pub status: String,
    pub logging_level: String,
    pub include_execution_data: bool,
    /// CloudWatch Logs group name parsed out of the logging destination, when
    /// logging is enabled to CloudWatch. Drives the `t` tail and the jumpable
    /// "Log Group" row.
    pub log_group: Option<String>,
    pub tracing_enabled: bool,
    pub created: Option<String>,
    pub definition: String, // ASL JSON (pretty-printed)
    pub tags: HashMap<String, String>,
}

/// `DescribeStateMachine` yields config + the ASL definition in one call;
/// also fetch tags so the Tags section is populated from the same describe.
pub async fn fetch_state_machine_details(client: SfnClient, arn: String) -> Result<SfnDetails> {
    let resp = client
        .describe_state_machine()
        .state_machine_arn(&arn)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;

    let role_arn = resp.role_arn().to_string();
    let kind = resp.r#type().as_str().to_string();
    let status = resp
        .status()
        .map(|s| s.as_str().to_string())
        .unwrap_or_else(|| "ACTIVE".to_string());
    let logging = resp.logging_configuration();
    let logging_level = logging
        .and_then(|c| c.level())
        .map(|l| l.as_str().to_string())
        .unwrap_or_else(|| "OFF".to_string());
    let include_execution_data = logging.map(|c| c.include_execution_data()).unwrap_or(false);
    let log_group = logging.and_then(log_group_from_config);
    let tracing_enabled = resp
        .tracing_configuration()
        .map(|t| t.enabled())
        .unwrap_or(false);
    let created = Some(fmt_epoch_secs(resp.creation_date().secs()));

    // Pretty-print the ASL definition (it's already JSON).
    let raw_def = resp.definition().to_string();
    let definition = serde_json::from_str::<serde_json::Value>(&raw_def)
        .ok()
        .and_then(|v| serde_json::to_string_pretty(&v).ok())
        .unwrap_or(raw_def);

    let tags = fetch_state_machine_tags(client, arn)
        .await
        .unwrap_or_default();

    Ok(SfnDetails {
        role_arn,
        kind,
        status,
        logging_level,
        include_execution_data,
        log_group,
        tracing_enabled,
        created,
        definition,
        tags,
    })
}

/// Pull the CloudWatch Logs group **name** out of a logging configuration.
/// The SDK carries an ARN (`arn:…:log-group:<name>[:*]`); the tail and the
/// Logs service both want the bare name.
fn log_group_from_config(cfg: &aws_sdk_sfn::types::LoggingConfiguration) -> Option<String> {
    cfg.destinations()
        .iter()
        .filter_map(|d| d.cloud_watch_logs_log_group())
        .filter_map(|g| g.log_group_arn())
        .find_map(log_group_name_from_arn)
}

/// `arn:aws:logs:<region>:<acct>:log-group:<name>[:*]` → `<name>`.
pub fn log_group_name_from_arn(arn: &str) -> Option<String> {
    let idx = arn.find(":log-group:")?;
    let name = &arn[idx + ":log-group:".len()..];
    let name = name.strip_suffix(":*").unwrap_or(name);
    (!name.is_empty()).then(|| name.to_string())
}

async fn fetch_state_machine_tags(
    client: SfnClient,
    arn: String,
) -> Result<HashMap<String, String>> {
    let resp = client
        .list_tags_for_resource()
        .resource_arn(&arn)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;

    let tags = resp
        .tags()
        .iter()
        .filter_map(|t| {
            t.key()
                .map(|k| (k.to_string(), t.value().unwrap_or_default().to_string()))
        })
        .collect();

    Ok(tags)
}

/// Resolve the CloudWatch Logs group a state machine writes to, for the `t`
/// tail. Logging is off by default on Step Functions, so an explicit,
/// actionable message beats an empty pane.
pub async fn resolve_sfn_log_group(
    client: SfnClient,
    state_machine_arn: String,
) -> Result<(String, Vec<String>)> {
    let resp = client
        .describe_state_machine()
        .state_machine_arn(&state_machine_arn)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;

    let level = resp
        .logging_configuration()
        .and_then(|c| c.level())
        .map(|l| l.as_str().to_string())
        .unwrap_or_else(|| "OFF".to_string());

    match resp.logging_configuration().and_then(log_group_from_config) {
        Some(group) => Ok((group, Vec::new())),
        None if level == "OFF" => Err(crate::error::Error::AwsSdk(
            "Logging is disabled for this state machine (set a log level to ALL/ERROR/FATAL)"
                .to_string(),
        )),
        None => Err(crate::error::Error::AwsSdk(
            "This state machine has no CloudWatch Logs destination configured".to_string(),
        )),
    }
}

/// List the most recent executions for a state machine (newest first, running
/// ones hoisted to the top so an in-flight workflow leads the batch).
pub async fn fetch_executions(
    client: SfnClient,
    arn: String,
    state_machine_name: String,
) -> Result<Vec<SfnExecution>> {
    let resp = client
        .list_executions()
        .state_machine_arn(&arn)
        .max_results(MAX_EXECUTIONS_PER_MACHINE)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;

    let mut executions: Vec<SfnExecution> = resp
        .executions()
        .iter()
        .map(|e| SfnExecution::from_list_item(e, &state_machine_name))
        .collect();

    // Newest first. The Executions sub-tab re-sorts the whole list the same
    // way (batches land in fetch-completion order), so keeping the two in step
    // means a state machine's own Executions section reads identically.
    executions.sort_by_key(|e| std::cmp::Reverse(e.started_secs));

    Ok(executions)
}

// ── Lazy: describe execution (input / output / failure) ────────────────────────

#[derive(Debug, Clone)]
pub struct SfnExecutionDetail {
    pub input: Option<String>,
    pub output: Option<String>,
    /// True when the payload was too large to return inline and was written to
    /// S3 instead — the pane says so rather than showing an empty section.
    pub input_truncated: bool,
    pub output_truncated: bool,
    pub error: Option<String>,
    pub cause: Option<String>,
    pub state_machine_version_arn: Option<String>,
    pub state_machine_alias_arn: Option<String>,
    pub redrive_count: Option<i32>,
    pub redrive_date: Option<String>,
    pub redrive_status: Option<String>,
    pub redrive_status_reason: Option<String>,
    pub trace_header: Option<String>,
}

/// Pretty-print a JSON payload; non-JSON strings pass through untouched.
fn pretty_json(raw: &str) -> String {
    serde_json::from_str::<serde_json::Value>(raw)
        .ok()
        .and_then(|v| serde_json::to_string_pretty(&v).ok())
        .unwrap_or_else(|| raw.to_string())
}

pub async fn fetch_execution_detail(
    client: SfnClient,
    execution_arn: String,
) -> Result<SfnExecutionDetail> {
    let resp = client
        .describe_execution()
        .execution_arn(&execution_arn)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;

    Ok(SfnExecutionDetail {
        input: resp.input().map(pretty_json),
        output: resp.output().map(pretty_json),
        input_truncated: resp.input_details().map(|d| d.included()) == Some(false),
        output_truncated: resp.output_details().map(|d| d.included()) == Some(false),
        error: resp.error().map(|s| s.to_string()),
        cause: resp.cause().map(pretty_json),
        state_machine_version_arn: resp.state_machine_version_arn().map(|s| s.to_string()),
        state_machine_alias_arn: resp.state_machine_alias_arn().map(|s| s.to_string()),
        redrive_count: resp.redrive_count(),
        redrive_date: resp.redrive_date().map(|d| fmt_epoch_secs(d.secs())),
        redrive_status: resp.redrive_status().map(|s| s.as_str().to_string()),
        redrive_status_reason: resp.redrive_status_reason().map(|s| s.to_string()),
        trace_header: resp.trace_header().map(|s| s.to_string()),
    })
}

// ── Lazy: execution history (the step-by-step timeline) ────────────────────────

/// One raw history event, flattened to the fields worth showing.
#[derive(Debug, Clone)]
pub struct SfnHistoryEvent {
    pub id: i64,
    pub ts_secs: i64,
    pub kind: String,
    /// State name, for the `State*` events.
    pub state: Option<String>,
    /// The invoked resource (Lambda ARN, `arn:aws:states:::sns:publish`, …).
    pub resource: Option<String>,
    pub error: Option<String>,
    pub cause: Option<String>,
}

/// A state the execution entered, with how long it took. Derived by pairing
/// `StateEntered`/`StateExited` — this is the "what is it doing right now"
/// view: the one span with no exit is the state currently running.
#[derive(Debug, Clone)]
pub struct SfnStateSpan {
    pub name: String,
    pub entered_secs: i64,
    pub duration_secs: Option<i64>,
    pub resource: Option<String>,
    pub error: Option<String>,
    pub cause: Option<String>,
}

impl SfnStateSpan {
    pub fn is_open(&self) -> bool {
        self.duration_secs.is_none()
    }
}

#[derive(Debug, Clone)]
pub struct SfnHistory {
    /// Chronological.
    pub events: Vec<SfnHistoryEvent>,
    /// Chronological by entry time.
    pub spans: Vec<SfnStateSpan>,
    /// The execution-level failure, when there is one.
    pub failure: Option<(String, String)>,
    /// The newest `MAX_HISTORY_EVENTS` were kept; older ones dropped.
    pub capped: bool,
}

/// Flatten one `HistoryEvent`. The SDK models ~40 per-type detail structs; the
/// handful below carry everything the timeline shows (state name, invoked
/// resource, error/cause) and the rest degrade to their type name.
fn flatten_history_event(e: &aws_sdk_sfn::types::HistoryEvent) -> SfnHistoryEvent {
    let mut state = None;
    let mut resource = None;
    let mut error = None;
    let mut cause = None;

    if let Some(d) = e.state_entered_event_details() {
        state = Some(d.name().to_string());
    }
    if let Some(d) = e.state_exited_event_details() {
        state = Some(d.name().to_string());
    }
    if let Some(d) = e.task_scheduled_event_details() {
        resource = Some(d.resource().to_string());
    }
    if let Some(d) = e.lambda_function_scheduled_event_details() {
        resource = Some(d.resource().to_string());
    }
    if let Some(d) = e.task_failed_event_details() {
        resource = Some(d.resource().to_string());
        error = d.error().map(|s| s.to_string());
        cause = d.cause().map(|s| s.to_string());
    }
    if let Some(d) = e.task_start_failed_event_details() {
        resource = Some(d.resource().to_string());
        error = d.error().map(|s| s.to_string());
        cause = d.cause().map(|s| s.to_string());
    }
    if let Some(d) = e.task_submit_failed_event_details() {
        resource = Some(d.resource().to_string());
        error = d.error().map(|s| s.to_string());
        cause = d.cause().map(|s| s.to_string());
    }
    if let Some(d) = e.task_timed_out_event_details() {
        resource = Some(d.resource().to_string());
        error = d.error().map(|s| s.to_string());
        cause = d.cause().map(|s| s.to_string());
    }
    if let Some(d) = e.lambda_function_failed_event_details() {
        error = d.error().map(|s| s.to_string());
        cause = d.cause().map(|s| s.to_string());
    }
    if let Some(d) = e.lambda_function_timed_out_event_details() {
        error = d.error().map(|s| s.to_string());
        cause = d.cause().map(|s| s.to_string());
    }
    if let Some(d) = e.lambda_function_start_failed_event_details() {
        error = d.error().map(|s| s.to_string());
        cause = d.cause().map(|s| s.to_string());
    }
    if let Some(d) = e.lambda_function_schedule_failed_event_details() {
        error = d.error().map(|s| s.to_string());
        cause = d.cause().map(|s| s.to_string());
    }
    if let Some(d) = e.activity_failed_event_details() {
        error = d.error().map(|s| s.to_string());
        cause = d.cause().map(|s| s.to_string());
    }
    if let Some(d) = e.activity_timed_out_event_details() {
        error = d.error().map(|s| s.to_string());
        cause = d.cause().map(|s| s.to_string());
    }
    if let Some(d) = e.execution_failed_event_details() {
        error = d.error().map(|s| s.to_string());
        cause = d.cause().map(|s| s.to_string());
    }
    if let Some(d) = e.execution_timed_out_event_details() {
        error = d.error().map(|s| s.to_string());
        cause = d.cause().map(|s| s.to_string());
    }
    if let Some(d) = e.execution_aborted_event_details() {
        error = d.error().map(|s| s.to_string());
        cause = d.cause().map(|s| s.to_string());
    }
    if let Some(d) = e.map_run_failed_event_details() {
        error = d.error().map(|s| s.to_string());
        cause = d.cause().map(|s| s.to_string());
    }
    if let Some(d) = e.evaluation_failed_event_details() {
        state = Some(d.state().to_string());
        error = d.error().map(|s| s.to_string());
        cause = d.cause().map(|s| s.to_string());
    }

    SfnHistoryEvent {
        id: e.id(),
        ts_secs: e.timestamp().secs(),
        kind: e.r#type().as_str().to_string(),
        state,
        resource,
        error,
        cause,
    }
}

/// Pair `StateEntered`/`StateExited` into spans, attaching whatever error and
/// invoked resource showed up while the state was open.
fn build_spans(events: &[SfnHistoryEvent]) -> Vec<SfnStateSpan> {
    let mut spans: Vec<SfnStateSpan> = Vec::new();
    // Indices into `spans` for states that haven't exited yet. Parallel/Map
    // branches interleave, so an exit closes the newest open span of that name.
    let mut open: Vec<usize> = Vec::new();

    for e in events {
        match e.kind.as_str() {
            k if k.ends_with("StateEntered") => {
                let Some(name) = e.state.clone() else { continue };
                spans.push(SfnStateSpan {
                    name,
                    entered_secs: e.ts_secs,
                    duration_secs: None,
                    resource: None,
                    error: None,
                    cause: None,
                });
                open.push(spans.len() - 1);
            }
            k if k.ends_with("StateExited") => {
                let Some(name) = e.state.clone() else { continue };
                if let Some(pos) = open.iter().rposition(|&i| spans[i].name == name) {
                    let idx = open.remove(pos);
                    spans[idx].duration_secs = Some(e.ts_secs - spans[idx].entered_secs);
                }
            }
            _ => {
                // Attach detail to the innermost open state.
                if let Some(&idx) = open.last() {
                    if spans[idx].resource.is_none() {
                        spans[idx].resource = e.resource.clone();
                    }
                    if e.error.is_some() && spans[idx].error.is_none() {
                        spans[idx].error = e.error.clone();
                        spans[idx].cause = e.cause.clone();
                    }
                }
            }
        }
    }

    spans
}

/// Fetch an execution's history. Runs **newest-first** (`reverse_order`) so
/// that hitting the cap on a long-running workflow keeps the recent events —
/// including the failure — rather than the first thousand steps.
pub async fn fetch_execution_history(
    client: SfnClient,
    execution_arn: String,
) -> Result<SfnHistory> {
    let mut newest_first: Vec<SfnHistoryEvent> = Vec::new();
    let mut token: Option<String> = None;
    let mut capped = false;

    loop {
        let mut req = client
            .get_execution_history()
            .execution_arn(&execution_arn)
            .reverse_order(true)
            .include_execution_data(true)
            .max_results(1000);
        if let Some(t) = &token {
            req = req.next_token(t);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;

        for e in resp.events() {
            newest_first.push(flatten_history_event(e));
        }

        if newest_first.len() >= MAX_HISTORY_EVENTS {
            capped = newest_first.len() > MAX_HISTORY_EVENTS || resp.next_token().is_some();
            newest_first.truncate(MAX_HISTORY_EVENTS);
            break;
        }

        match crate::aws::pagination::next_page_token(resp.next_token(), &token) {
            Some(t) => token = Some(t),
            None => break,
        }
    }

    // Back to chronological for display.
    newest_first.reverse();
    let events = newest_first;

    let failure = events.iter().rev().find_map(|e| {
        (e.kind.starts_with("ExecutionFailed")
            || e.kind.starts_with("ExecutionAborted")
            || e.kind.starts_with("ExecutionTimedOut"))
        .then(|| {
            (
                e.error.clone().unwrap_or_else(|| e.kind.clone()),
                e.cause.clone().unwrap_or_default(),
            )
        })
    });

    let spans = build_spans(&events);

    Ok(SfnHistory {
        events,
        spans,
        failure,
        capped,
    })
}

// ── Timestamp helpers ─────────────────────────────────────────────────────────

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Compact duration: `1.2s` / `45s` / `3m 12s` / `2h 05m`.
pub fn fmt_duration(secs: i64) -> String {
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m {:02}s", secs / 60, secs % 60)
    } else {
        format!("{}h {:02}m", secs / 3600, (secs % 3600) / 60)
    }
}

pub fn fmt_epoch_secs(secs: i64) -> String {
    let days = secs / 86400;
    let time_rem = secs % 86400;
    let h = time_rem / 3600;
    let m = (time_rem % 3600) / 60;
    let (y, mo, d) = epoch_days_to_ymd(days);
    format!("{:04}-{:02}-{:02} {:02}:{:02} UTC", y, mo, d, h, m)
}

/// `HH:MM:SS` — the history timeline is read relative to the execution, so the
/// date would just be repeated noise on every row.
pub fn fmt_epoch_time(secs: i64) -> String {
    let time_rem = secs % 86400;
    format!(
        "{:02}:{:02}:{:02}",
        time_rem / 3600,
        (time_rem % 3600) / 60,
        time_rem % 60
    )
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
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut month = 1u8;
    for &d in &dm {
        if days < d as i64 {
            break;
        }
        days -= d as i64;
        month += 1;
    }
    (year, month, (days + 1) as u8)
}

fn is_leap(y: i32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

// ── Metrics (AWS/States) ────────────────────────────────────────────────────────

pub use crate::aws::services::ec2::MetricsTimeRange;

#[derive(Debug, Clone)]
pub struct SfnMetricsData {
    pub time_range: MetricsTimeRange,
    pub started: Vec<(f64, f64)>,
    pub succeeded: Vec<(f64, f64)>,
    pub failed: Vec<(f64, f64)>,
    pub aborted: Vec<(f64, f64)>,
    pub timed_out: Vec<(f64, f64)>,
    pub duration: Vec<(f64, f64)>, // ExecutionTime, ms (avg)
    pub x_max: f64,
}

#[derive(Debug, Clone)]
pub enum SfnMetricsState {
    Loading,
    Loaded(SfnMetricsData),
}

fn parse_points<E>(
    resp: std::result::Result<
        aws_sdk_cloudwatch::operation::get_metric_statistics::GetMetricStatisticsOutput,
        E,
    >,
    start: i64,
    pick: impl Fn(&aws_sdk_cloudwatch::types::Datapoint) -> Option<f64>,
) -> Vec<(f64, f64)> {
    let dps = match resp {
        Ok(r) => r.datapoints().to_vec(),
        Err(_) => vec![],
    };
    let mut pts: Vec<(f64, f64)> = dps
        .iter()
        .filter_map(|dp| Some((dp.timestamp()?.secs() as f64 - start as f64, pick(dp)?)))
        .collect();
    pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    pts
}

/// Fetch the console-style execution metrics for one state machine
/// (`AWS/States`, dimension `StateMachineArn`).
pub async fn fetch_sfn_metrics(
    cw: aws_sdk_cloudwatch::Client,
    state_machine_arn: String,
    time_range: MetricsTimeRange,
) -> Result<SfnMetricsData> {
    use aws_sdk_cloudwatch::types::{Dimension, Statistic};

    let now = now_secs();
    let start = now - time_range.duration_secs();
    let period = time_range.period_secs();
    let start_dt = aws_sdk_cloudwatch::primitives::DateTime::from_secs(start);
    let end_dt = aws_sdk_cloudwatch::primitives::DateTime::from_secs(now);

    let dim = || {
        Dimension::builder()
            .name("StateMachineArn")
            .value(&state_machine_arn)
            .build()
    };
    let metric = |name: &'static str, stat: Statistic| {
        cw.get_metric_statistics()
            .namespace("AWS/States")
            .metric_name(name)
            .dimensions(dim())
            .start_time(start_dt)
            .end_time(end_dt)
            .period(period)
            .set_statistics(Some(vec![stat]))
            .send()
    };

    let (started, succeeded, failed, aborted, timed_out, duration) = tokio::join!(
        metric("ExecutionsStarted", Statistic::Sum),
        metric("ExecutionsSucceeded", Statistic::Sum),
        metric("ExecutionsFailed", Statistic::Sum),
        metric("ExecutionsAborted", Statistic::Sum),
        metric("ExecutionsTimedOut", Statistic::Sum),
        metric("ExecutionTime", Statistic::Average),
    );

    Ok(SfnMetricsData {
        time_range,
        started: parse_points(started, start, |dp| dp.sum()),
        succeeded: parse_points(succeeded, start, |dp| dp.sum()),
        failed: parse_points(failed, start, |dp| dp.sum()),
        aborted: parse_points(aborted, start, |dp| dp.sum()),
        timed_out: parse_points(timed_out, start, |dp| dp.sum()),
        duration: parse_points(duration, start, |dp| dp.average()),
        x_max: time_range.duration_secs() as f64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn execution_status_maps_to_state() {
        assert!(matches!(
            execution_status_state("SUCCEEDED"),
            ResourceState::Available
        ));
        assert!(matches!(
            execution_status_state("RUNNING"),
            ResourceState::Pending
        ));
        assert!(matches!(
            execution_status_state("FAILED"),
            ResourceState::Unavailable
        ));
        assert!(matches!(
            execution_status_state("TIMED_OUT"),
            ResourceState::Unavailable
        ));
        assert!(matches!(
            execution_status_state("ABORTED"),
            ResourceState::Unavailable
        ));
        assert!(matches!(
            execution_status_state("WEIRD"),
            ResourceState::Unknown(_)
        ));
    }

    #[test]
    fn type_label_maps() {
        assert_eq!(SfnStateMachine::type_label("STANDARD"), "Standard");
        assert_eq!(SfnStateMachine::type_label("EXPRESS"), "Express");
        assert_eq!(SfnStateMachine::type_label("xyz"), "Unknown");
    }

    #[test]
    fn log_group_arn_is_reduced_to_a_name() {
        assert_eq!(
            log_group_name_from_arn(
                "arn:aws:logs:us-east-1:123456789012:log-group:/aws/vendedlogs/states/foo:*"
            )
            .as_deref(),
            Some("/aws/vendedlogs/states/foo")
        );
        // Without the trailing `:*` wildcard.
        assert_eq!(
            log_group_name_from_arn("arn:aws:logs:us-east-1:1:log-group:/g").as_deref(),
            Some("/g")
        );
        assert_eq!(log_group_name_from_arn("arn:aws:s3:::bucket"), None);
    }

    #[test]
    fn duration_formats_by_magnitude() {
        assert_eq!(fmt_duration(9), "9s");
        assert_eq!(fmt_duration(75), "1m 15s");
        assert_eq!(fmt_duration(7325), "2h 02m");
    }

    fn ev(id: i64, ts: i64, kind: &str, state: Option<&str>) -> SfnHistoryEvent {
        SfnHistoryEvent {
            id,
            ts_secs: ts,
            kind: kind.to_string(),
            state: state.map(|s| s.to_string()),
            resource: None,
            error: None,
            cause: None,
        }
    }

    #[test]
    fn spans_pair_entered_and_exited() {
        let events = vec![
            ev(1, 100, "TaskStateEntered", Some("Validate")),
            ev(2, 110, "TaskStateExited", Some("Validate")),
            ev(3, 110, "TaskStateEntered", Some("Deploy")),
        ];
        let spans = build_spans(&events);
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].duration_secs, Some(10));
        // Deploy never exited — it's the state currently running.
        assert!(spans[1].is_open());
        assert_eq!(spans[1].name, "Deploy");
    }

    #[test]
    fn span_absorbs_the_failure_of_its_open_state() {
        let mut failed = ev(2, 105, "TaskFailed", None);
        failed.error = Some("States.TaskFailed".to_string());
        failed.cause = Some("boom".to_string());
        let events = vec![
            ev(1, 100, "TaskStateEntered", Some("Deploy")),
            failed,
            ev(3, 106, "ExecutionFailed", None),
        ];
        let spans = build_spans(&events);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].error.as_deref(), Some("States.TaskFailed"));
        assert_eq!(spans[0].cause.as_deref(), Some("boom"));
    }

    #[test]
    fn parallel_branches_close_the_newest_open_span_of_that_name() {
        // A Map/Parallel branch can re-enter the same state name before the
        // first one exits; the exit must not close the wrong span.
        let events = vec![
            ev(1, 100, "TaskStateEntered", Some("Work")),
            ev(2, 101, "TaskStateEntered", Some("Work")),
            ev(3, 105, "TaskStateExited", Some("Work")),
        ];
        let spans = build_spans(&events);
        assert!(spans[0].is_open());
        assert_eq!(spans[1].duration_secs, Some(4));
    }
}
