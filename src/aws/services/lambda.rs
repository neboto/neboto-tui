use crate::aws::client::AwsClients;
use crate::aws::resource::sg_refs;
use crate::aws::resource::{Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::aws::services::ec2::MetricsTimeRange;
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use aws_sdk_cloudwatch::types::Datapoint;
use aws_sdk_lambda::Client as LambdaClient;
use std::any::Any;
use std::collections::HashMap;
use tokio::sync::mpsc;

pub struct LambdaService {
    client: LambdaClient,
}

impl LambdaService {
    pub fn new(aws_clients: &AwsClients) -> Self {
        Self {
            client: aws_clients.lambda_client(),
        }
    }
}

#[async_trait]
impl AwsService for LambdaService {
    fn service_type(&self) -> ServiceType {
        ServiceType::Lambda
    }

    fn name(&self) -> &str {
        "Lambda"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        self.list_resources_streaming(tx, ServiceType::Lambda).await?;
        Ok(vec![])
    }

    async fn list_resources_streaming(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        let mut total = 0usize;
        let mut paginator = self.client.list_functions().into_paginator().send();

        while let Some(result) = paginator.next().await {
            match result {
                Ok(page) => {
                    let batch: Vec<Box<dyn Resource>> = page
                        .functions()
                        .iter()
                        .map(|f| Box::new(LambdaFunction::from_sdk(f)) as Box<dyn Resource>)
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
                        error: format!("Failed to list Lambda functions: {}", e),
                    });
                    return Ok(());
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

// ── LambdaFunction ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct LambdaFunction {
    pub function_name: String,
    pub function_arn: String,
    pub runtime: String,
    pub role: String,
    pub handler: String,
    pub code_size: i64,
    pub description: String,
    pub timeout: i32,
    pub memory_size: i32,
    pub last_modified: String,
    pub env_vars: HashMap<String, String>,
    pub vpc_id: Option<String>,
    pub subnet_ids: Vec<String>,
    pub security_group_ids: Vec<String>,
    pub layers: Vec<String>,
    /// CloudWatch Logs group the function writes to. Honours an explicit
    /// `LoggingConfig.LogGroup` override, else the default `/aws/lambda/<name>`.
    pub log_group: String,
    /// SQS queue / SNS topic ARN that async invocations land in after every
    /// retry fails. Comes back on `ListFunctions`, so it costs nothing.
    pub dead_letter_arn: Option<String>,
    pub tags: HashMap<String, String>,
}

impl LambdaFunction {
    pub fn from_sdk(f: &aws_sdk_lambda::types::FunctionConfiguration) -> Self {
        let env_vars: HashMap<String, String> = f
            .environment()
            .and_then(|e| e.variables())
            .cloned()
            .unwrap_or_default();

        let vpc_id = f
            .vpc_config()
            .and_then(|v| v.vpc_id())
            .map(|s| s.to_string());
        let subnet_ids: Vec<String> = f
            .vpc_config()
            .map(|v| v.subnet_ids().iter().map(|s| s.to_string()).collect())
            .unwrap_or_default();
        let security_group_ids: Vec<String> = f
            .vpc_config()
            .map(|v| v.security_group_ids().iter().map(|s| s.to_string()).collect())
            .unwrap_or_default();

        let layers: Vec<String> = f
            .layers()
            .iter()
            .filter_map(|l| l.arn().map(|s| s.to_string()))
            .collect();

        let function_name = f.function_name().unwrap_or("").to_string();
        let log_group = f
            .logging_config()
            .and_then(|l| l.log_group())
            .filter(|g| !g.is_empty())
            .map(|g| g.to_string())
            .unwrap_or_else(|| format!("/aws/lambda/{}", function_name));

        Self {
            function_name: f.function_name().unwrap_or("").to_string(),
            function_arn: f.function_arn().unwrap_or("").to_string(),
            runtime: f
                .runtime()
                .map(|r| r.as_str().to_string())
                .unwrap_or_else(|| "unknown".to_string()),
            role: f.role().unwrap_or("").to_string(),
            handler: f.handler().unwrap_or("").to_string(),
            code_size: f.code_size(),
            description: f.description().unwrap_or("").to_string(),
            timeout: f.timeout().unwrap_or(0),
            memory_size: f.memory_size().unwrap_or(128),
            last_modified: f.last_modified().unwrap_or("").to_string(),
            env_vars,
            vpc_id,
            subnet_ids,
            security_group_ids,
            layers,
            log_group,
            dead_letter_arn: f
                .dead_letter_config()
                .and_then(|d| d.target_arn())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string()),
            tags: HashMap::new(),
        }
    }

    pub fn formatted_code_size(&self) -> String {
        let bytes = self.code_size;
        if bytes < 1024 {
            format!("{} B", bytes)
        } else if bytes < 1024 * 1024 {
            format!("{:.1} KB", bytes as f64 / 1024.0)
        } else {
            format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
        }
    }
}

crate::sections! {
    pub enum LambdaDetailSection,
    pub static LAMBDA_SECTIONS = [
        Config "Config" => crate::app::App::trigger_lambda_overview_enter,
        Code "Code" => crate::app::App::trigger_lambda_code_load,
        Triggers "Triggers" => crate::app::App::trigger_lambda_triggers_load,
        Environment "Environment",
        Tags "Tags" => crate::app::App::trigger_lambda_code_load,
        Optimizer "Optimizer" => crate::app::App::trigger_optimizer_load,
    ]
}

impl Resource for LambdaFunction {
    fn references(&self) -> Vec<(String, String)> {
        let mut v = sg_refs(self.security_group_ids());
        let mut r = |l: &str, x: &str| v.push((l.to_string(), x.to_string()));
        for x in &self.subnet_ids { r("Subnet", x); }
        if let Some(x) = &self.vpc_id { r("VPC", x); }
        r("Role", &self.role);
        for x in &self.layers { r("Layer", x); }
        r("Log Group", &self.log_group);
        if let Some(x) = &self.dead_letter_arn { r("Dead Letter Target", x); }
        for (k, val) in &self.env_vars { r(&format!("Env {}", k), val); }
        v
    }

    fn security_group_ids(&self) -> Vec<String> {
        self.security_group_ids.clone()
    }

    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&LAMBDA_SECTIONS)
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws lambda get-function --function-name {}",
            crate::aws::resource::shell_quote(&self.function_name)
        ))
    }

    fn id(&self) -> &str {
        &self.function_arn
    }

    fn name(&self) -> &str {
        &self.function_name
    }

    fn resource_type(&self) -> &str {
        "Lambda Function"
    }

    fn state(&self) -> ResourceState {
        ResourceState::stateless()
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!(
            "{} {} {} {}",
            self.function_name,
            self.function_arn,
            self.runtime,
            self.handler,
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Function Name".to_string(), self.function_name.clone()),
            ("Runtime".to_string(), self.runtime.clone()),
            ("Handler".to_string(), self.handler.clone()),
            ("Memory".to_string(), format!("{} MB", self.memory_size)),
            ("Timeout".to_string(), format!("{} s", self.timeout)),
            ("Code Size".to_string(), self.formatted_code_size()),
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
            "https://{}.console.aws.amazon.com/lambda/home?region={}#/functions/{}",
            region, region, self.function_name
        ))
    }
}

// ── Lambda metrics (CloudWatch) ───────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct LambdaMetricsData {
    pub time_range: MetricsTimeRange,
    pub invocations: Vec<(f64, f64)>, // (secs_from_start, sum per period)
    pub duration: Vec<(f64, f64)>,    // (secs_from_start, avg ms)
    pub errors: Vec<(f64, f64)>,      // (secs_from_start, sum per period)
    pub throttles: Vec<(f64, f64)>,   // (secs_from_start, sum per period)
    pub concurrent: Vec<(f64, f64)>,  // (secs_from_start, max count)
    pub x_max: f64,
}

#[derive(Debug, Clone)]
pub enum LambdaMetricsState {
    Loading,
    Loaded(LambdaMetricsData),
}

fn parse_sum(datapoints: &[Datapoint], start_secs: i64) -> Vec<(f64, f64)> {
    let mut pts: Vec<(f64, f64)> = datapoints
        .iter()
        .filter_map(|dp| {
            let t = (dp.timestamp()?.secs() - start_secs) as f64;
            Some((t, dp.sum().unwrap_or(0.0)))
        })
        .collect();
    pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    pts
}

fn parse_avg(datapoints: &[Datapoint], start_secs: i64) -> Vec<(f64, f64)> {
    let mut pts: Vec<(f64, f64)> = datapoints
        .iter()
        .filter_map(|dp| {
            let t = (dp.timestamp()?.secs() - start_secs) as f64;
            Some((t, dp.average().unwrap_or(0.0)))
        })
        .collect();
    pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    pts
}

fn parse_max(datapoints: &[Datapoint], start_secs: i64) -> Vec<(f64, f64)> {
    let mut pts: Vec<(f64, f64)> = datapoints
        .iter()
        .filter_map(|dp| {
            let t = (dp.timestamp()?.secs() - start_secs) as f64;
            Some((t, dp.maximum().unwrap_or(0.0)))
        })
        .collect();
    pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    pts
}

pub async fn fetch_lambda_metrics(
    cw_client: aws_sdk_cloudwatch::Client,
    function_name: String,
    time_range: MetricsTimeRange,
) -> Result<LambdaMetricsData> {
    use aws_sdk_cloudwatch::types::{Dimension, Statistic};

    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let start_secs = now_secs - time_range.duration_secs();
    let period = time_range.period_secs();

    let start_dt = aws_sdk_cloudwatch::primitives::DateTime::from_secs(start_secs);
    let end_dt = aws_sdk_cloudwatch::primitives::DateTime::from_secs(now_secs);

    let make_dim = || {
        Dimension::builder()
            .name("FunctionName")
            .value(&function_name)
            .build()
    };

    let (inv_r, dur_r, err_r, thr_r, con_r) = tokio::join!(
        cw_client
            .get_metric_statistics()
            .namespace("AWS/Lambda")
            .metric_name("Invocations")
            .dimensions(make_dim())
            .start_time(start_dt.clone())
            .end_time(end_dt.clone())
            .period(period)
            .set_statistics(Some(vec![Statistic::Sum]))
            .send(),
        cw_client
            .get_metric_statistics()
            .namespace("AWS/Lambda")
            .metric_name("Duration")
            .dimensions(make_dim())
            .start_time(start_dt.clone())
            .end_time(end_dt.clone())
            .period(period)
            .set_statistics(Some(vec![Statistic::Average]))
            .send(),
        cw_client
            .get_metric_statistics()
            .namespace("AWS/Lambda")
            .metric_name("Errors")
            .dimensions(make_dim())
            .start_time(start_dt.clone())
            .end_time(end_dt.clone())
            .period(period)
            .set_statistics(Some(vec![Statistic::Sum]))
            .send(),
        cw_client
            .get_metric_statistics()
            .namespace("AWS/Lambda")
            .metric_name("Throttles")
            .dimensions(make_dim())
            .start_time(start_dt.clone())
            .end_time(end_dt.clone())
            .period(period)
            .set_statistics(Some(vec![Statistic::Sum]))
            .send(),
        cw_client
            .get_metric_statistics()
            .namespace("AWS/Lambda")
            .metric_name("ConcurrentExecutions")
            .dimensions(make_dim())
            .start_time(start_dt)
            .end_time(end_dt)
            .period(period)
            .set_statistics(Some(vec![Statistic::Maximum]))
            .send(),
    );

    let inv_dp = match inv_r { Ok(r) => r.datapoints().to_vec(), Err(_) => vec![] };
    let dur_dp = match dur_r { Ok(r) => r.datapoints().to_vec(), Err(_) => vec![] };
    let err_dp = match err_r { Ok(r) => r.datapoints().to_vec(), Err(_) => vec![] };
    let thr_dp = match thr_r { Ok(r) => r.datapoints().to_vec(), Err(_) => vec![] };
    let con_dp = match con_r { Ok(r) => r.datapoints().to_vec(), Err(_) => vec![] };

    Ok(LambdaMetricsData {
        time_range,
        invocations: parse_sum(&inv_dp, start_secs),
        duration: parse_avg(&dur_dp, start_secs),
        errors: parse_sum(&err_dp, start_secs),
        throttles: parse_sum(&thr_dp, start_secs),
        concurrent: parse_max(&con_dp, start_secs),
        x_max: time_range.duration_secs() as f64,
    })
}

// ── Lambda event source mappings (Triggers section) ───────────────────────────

/// One event source mapping from `ListEventSourceMappings` — the poll-based
/// triggers (SQS / Kinesis / DynamoDB Streams / Kafka / MQ / DocumentDB).
#[derive(Debug, Clone)]
pub struct LambdaEsmInfo {
    pub uuid: String,
    /// Absent for self-managed Kafka sources (endpoints live in `topics`).
    pub event_source_arn: Option<String>,
    pub state: String,
    pub state_transition_reason: Option<String>,
    pub last_processing_result: Option<String>,
    pub last_modified: Option<String>,
    pub batch_size: Option<i32>,
    pub max_batching_window_secs: Option<i32>,
    pub starting_position: Option<String>,
    pub maximum_retry_attempts: Option<i32>,
    pub maximum_record_age_secs: Option<i32>,
    pub bisect_batch_on_error: Option<bool>,
    pub parallelization_factor: Option<i32>,
    pub max_concurrency: Option<i32>,
    pub on_failure_destination: Option<String>,
    pub filter_patterns: Vec<String>,
    /// Kafka topics (MSK / self-managed) or MQ queues, whichever the source uses.
    pub topics: Vec<String>,
    pub queues: Vec<String>,
    pub report_batch_item_failures: bool,
}

impl LambdaEsmInfo {
    pub fn from_sdk(m: &aws_sdk_lambda::types::EventSourceMappingConfiguration) -> Self {
        Self {
            uuid: m.uuid().unwrap_or("").to_string(),
            event_source_arn: m.event_source_arn().map(|s| s.to_string()),
            state: m.state().unwrap_or("Unknown").to_string(),
            state_transition_reason: m.state_transition_reason().map(|s| s.to_string()),
            last_processing_result: m.last_processing_result().map(|s| s.to_string()),
            last_modified: m.last_modified().map(|d| {
                d.fmt(aws_smithy_types::date_time::Format::DateTime)
                    .unwrap_or_default()
            }),
            batch_size: m.batch_size(),
            max_batching_window_secs: m.maximum_batching_window_in_seconds(),
            starting_position: m.starting_position().map(|p| p.as_str().to_string()),
            maximum_retry_attempts: m.maximum_retry_attempts(),
            maximum_record_age_secs: m.maximum_record_age_in_seconds(),
            bisect_batch_on_error: m.bisect_batch_on_function_error(),
            parallelization_factor: m.parallelization_factor(),
            max_concurrency: m.scaling_config().and_then(|s| s.maximum_concurrency()),
            on_failure_destination: m
                .destination_config()
                .and_then(|d| d.on_failure())
                .and_then(|f| f.destination())
                .map(|s| s.to_string()),
            filter_patterns: m
                .filter_criteria()
                .map(|c| {
                    c.filters()
                        .iter()
                        .filter_map(|f| f.pattern().map(|p| p.to_string()))
                        .collect()
                })
                .unwrap_or_default(),
            topics: m.topics().to_vec(),
            queues: m.queues().to_vec(),
            report_batch_item_failures: m
                .function_response_types()
                .iter()
                .any(|t| t.as_str() == "ReportBatchItemFailures"),
        }
    }

    /// Human label for the source service, derived from the ARN's service
    /// segment (`arn:aws:<service>:…`); self-managed Kafka has no ARN.
    pub fn source_label(&self) -> &'static str {
        let Some(arn) = &self.event_source_arn else {
            return "Self-managed Kafka";
        };
        match arn.split(':').nth(2) {
            Some("sqs") => "SQS",
            Some("kinesis") => "Kinesis",
            Some("dynamodb") => "DynamoDB Streams",
            Some("kafka") => "MSK",
            Some("mq") => "Amazon MQ",
            Some("docdb") | Some("rds") => "DocumentDB",
            _ => "Event source",
        }
    }

    /// Whether the mapping is actively polling. Disabled / Disabling and the
    /// transitional Create/Update states all mean "not (reliably) draining".
    pub fn is_enabled(&self) -> bool {
        self.state.eq_ignore_ascii_case("Enabled")
    }
}

/// `ListEventSourceMappings` for one function (lazy Triggers section).
pub async fn fetch_lambda_esms(
    client: LambdaClient,
    function_name: String,
) -> Result<Vec<LambdaEsmInfo>> {
    let mut out = Vec::new();
    let mut paginator = client
        .list_event_source_mappings()
        .function_name(&function_name)
        .into_paginator()
        .send();
    while let Some(page) = paginator.next().await {
        let page = page?;
        out.extend(page.event_source_mappings().iter().map(LambdaEsmInfo::from_sdk));
    }
    Ok(out)
}

/// Defensive cap on the reverse EventBridge lookup below — in practice a
/// function is targeted by a handful of rules at most.
const MAX_LAMBDA_EB_RULES: usize = 25;

/// EventBridge rules that invoke this function (the push side of Triggers,
/// alongside the poll-based `fetch_lambda_esms` above). `ListTargetsByRule`
/// only answers "what does this rule target", not the reverse, so a naive
/// lookup would mean `ListRules` + `ListTargetsByRule` for every rule in the
/// account. Instead this uses `ListRuleNamesByTarget`, which is server-side
/// filtered by target ARN — one call per event bus (buses are few) rather
/// than one per rule, then a `DescribeRule` per match to get state/schedule/
/// pattern for display.
pub async fn fetch_lambda_eventbridge_rules(
    eb_client: aws_sdk_eventbridge::Client,
    function_arn: String,
) -> Result<Vec<crate::aws::services::eventbridge::EbRule>> {
    use crate::aws::services::eventbridge::EbRule;

    let mut buses: Vec<String> = Vec::new();
    let mut bus_token: Option<String> = None;
    loop {
        let mut req = eb_client.list_event_buses();
        if let Some(t) = &bus_token {
            req = req.next_token(t);
        }
        let page = req.send().await?;
        buses.extend(page.event_buses().iter().filter_map(|b| b.name().map(|s| s.to_string())));
        bus_token = crate::aws::pagination::next_page_token(page.next_token(), &bus_token);
        if bus_token.is_none() {
            break;
        }
    }

    let mut matches: Vec<(String, String)> = Vec::new(); // (bus, rule_name)
    'buses: for bus in &buses {
        let mut token: Option<String> = None;
        loop {
            let mut req = eb_client
                .list_rule_names_by_target()
                .target_arn(&function_arn)
                .event_bus_name(bus);
            if let Some(t) = &token {
                req = req.next_token(t);
            }
            let page = req.send().await?;
            for name in page.rule_names() {
                matches.push((bus.clone(), name.clone()));
                if matches.len() >= MAX_LAMBDA_EB_RULES {
                    break 'buses;
                }
            }
            token = crate::aws::pagination::next_page_token(page.next_token(), &token);
            if token.is_none() {
                break;
            }
        }
    }

    let describe_futs = matches.into_iter().map(|(bus, name)| {
        let client = eb_client.clone();
        async move {
            client
                .describe_rule()
                .name(&name)
                .event_bus_name(&bus)
                .send()
                .await
                .ok()
                .map(|out| EbRule::from_describe(&bus, &out))
        }
    });
    Ok(futures::future::join_all(describe_futs)
        .await
        .into_iter()
        .flatten()
        .collect())
}

// ── Lambda code (GetFunction + package download) ──────────────────────────────

/// One provisioned-concurrency allocation (per alias / version qualifier).
#[derive(Debug, Clone)]
pub struct LambdaProvisionedConcurrency {
    /// The alias or version the allocation is attached to — provisioned
    /// concurrency can never target `$LATEST`, so this is always a qualifier.
    pub qualifier: String,
    pub requested: Option<i32>,
    pub available: Option<i32>,
    pub allocated: Option<i32>,
    pub status: String,
    pub status_reason: Option<String>,
}

/// Concurrency limits + async-invoke handling (lazy, Config section). Three
/// calls bundled into one fetch because they answer one question between them:
/// "why is this function throttling, and where do failed events end up".
#[derive(Debug, Clone, Default)]
pub struct LambdaConcurrency {
    /// `None` = no reserved limit (draws from the account pool). `Some(0)` is
    /// very much not the same thing — it throttles every invocation.
    pub reserved: Option<i32>,
    pub provisioned: Vec<LambdaProvisionedConcurrency>,
    /// Async-invoke config (`GetFunctionEventInvokeConfig`). Absent when the
    /// function has never had one set, which is the common case.
    pub on_success: Option<String>,
    pub on_failure: Option<String>,
    pub max_retry_attempts: Option<i32>,
    pub max_event_age_secs: Option<i32>,
    pub has_event_invoke_config: bool,
    /// Per-call failures, kept rather than surfaced as a whole-section error:
    /// the three calls are independent and a denial on one shouldn't blank the
    /// other two.
    pub warnings: Vec<String>,
}

/// Reserved + provisioned concurrency and the async-invoke destination config.
/// Every call is individually best-effort — see `LambdaConcurrency::warnings`.
pub async fn fetch_lambda_concurrency(
    client: LambdaClient,
    function_name: String,
) -> Result<LambdaConcurrency> {
    let mut out = LambdaConcurrency::default();

    match client
        .get_function_concurrency()
        .function_name(&function_name)
        .send()
        .await
    {
        Ok(resp) => out.reserved = resp.reserved_concurrent_executions(),
        Err(e) => out
            .warnings
            .push(format!("reserved concurrency: {}", crate::error::sdk_error_message(&e))),
    }

    let mut pages = client
        .list_provisioned_concurrency_configs()
        .function_name(&function_name)
        .into_paginator()
        .send();
    loop {
        match pages.try_next().await {
            Ok(Some(page)) => {
                for c in page.provisioned_concurrency_configs() {
                    // The qualifier is only recoverable from the tail of the
                    // returned ARN — the list item carries no separate field.
                    let qualifier = c
                        .function_arn()
                        .and_then(|a| a.rsplit(':').next())
                        .unwrap_or("—")
                        .to_string();
                    out.provisioned.push(LambdaProvisionedConcurrency {
                        qualifier,
                        requested: c.requested_provisioned_concurrent_executions(),
                        available: c.available_provisioned_concurrent_executions(),
                        allocated: c.allocated_provisioned_concurrent_executions(),
                        status: c
                            .status()
                            .map(|s| s.as_str().to_string())
                            .unwrap_or_else(|| "—".to_string()),
                        status_reason: c.status_reason().map(|s| s.to_string()),
                    });
                }
            }
            Ok(None) => break,
            Err(e) => {
                out.warnings.push(format!(
                    "provisioned concurrency: {}",
                    crate::error::sdk_error_message(&e)
                ));
                break;
            }
        }
    }

    // A function with no async-invoke config returns ResourceNotFound. That is
    // the default state, not a problem — leave the section quiet.
    match client
        .get_function_event_invoke_config()
        .function_name(&function_name)
        .send()
        .await
    {
        Ok(resp) => {
            out.has_event_invoke_config = true;
            out.max_retry_attempts = resp.maximum_retry_attempts();
            out.max_event_age_secs = resp.maximum_event_age_in_seconds();
            if let Some(dest) = resp.destination_config() {
                out.on_success = dest
                    .on_success()
                    .and_then(|d| d.destination())
                    .map(|s| s.to_string());
                out.on_failure = dest
                    .on_failure()
                    .and_then(|d| d.destination())
                    .map(|s| s.to_string());
            }
        }
        Err(e) => {
            let msg = crate::error::sdk_error_message(&e);
            if !msg.contains("ResourceNotFound") && !msg.contains("does not have an EventInvokeConfig")
            {
                out.warnings.push(format!("async invoke config: {msg}"));
            }
        }
    }

    Ok(out)
}

/// Code-package metadata from `GetFunction` (lazy detail section).
#[derive(Debug, Clone)]
pub struct LambdaCodeInfo {
    /// `State` / `LastUpdateStatus` (+ reasons) — **only** `GetFunction`
    /// returns these; `ListFunctions` omits them, which is why the list row is
    /// stateless and the pane shows them here.
    pub state: Option<String>,
    pub state_reason: Option<String>,
    pub last_update_status: Option<String>,
    pub last_update_status_reason: Option<String>,
    pub package_type: String, // "Zip" | "Image"
    pub repository_type: Option<String>,
    pub code_location: Option<String>, // presigned URL (Zip packages)
    pub image_uri: Option<String>,     // ECR image (Image packages)
    pub code_sha256: Option<String>,
    pub runtime: String,
    pub handler: String,
    pub code_size: i64,
    /// `GetFunction` is the only place Lambda hands back tags —
    /// `ListFunctions` has none — so the Tags section and the ownership
    /// ribbon both read them from here.
    pub tags: HashMap<String, String>,
}

impl LambdaCodeInfo {
    /// Zip-packaged functions have a downloadable deployment package; container
    /// (Image) functions don't.
    pub fn is_zip(&self) -> bool {
        self.package_type.eq_ignore_ascii_case("Zip")
    }
}

/// `GetFunction` → code-package metadata (incl. the presigned download URL).
pub async fn fetch_lambda_code(
    client: LambdaClient,
    function_name: String,
) -> Result<LambdaCodeInfo> {
    let resp = client
        .get_function()
        .function_name(&function_name)
        .send()
        .await?;

    let code = resp.code();
    let config = resp.configuration();

    Ok(LambdaCodeInfo {
        state: config.and_then(|c| c.state()).map(|v| v.as_str().to_string()),
        state_reason: config.and_then(|c| c.state_reason()).map(|v| v.to_string()),
        last_update_status: config
            .and_then(|c| c.last_update_status())
            .map(|v| v.as_str().to_string()),
        last_update_status_reason: config
            .and_then(|c| c.last_update_status_reason())
            .map(|v| v.to_string()),
        package_type: config
            .and_then(|c| c.package_type())
            .map(|p| p.as_str().to_string())
            .unwrap_or_else(|| "Zip".to_string()),
        repository_type: code.and_then(|c| c.repository_type()).map(|s| s.to_string()),
        code_location: code.and_then(|c| c.location()).map(|s| s.to_string()),
        image_uri: code.and_then(|c| c.image_uri()).map(|s| s.to_string()),
        code_sha256: config.and_then(|c| c.code_sha256()).map(|s| s.to_string()),
        runtime: config
            .and_then(|c| c.runtime())
            .map(|r| r.as_str().to_string())
            .unwrap_or_else(|| "—".to_string()),
        handler: config.and_then(|c| c.handler()).unwrap_or("").to_string(),
        code_size: config.map(|c| c.code_size()).unwrap_or(0),
        tags: resp.tags().cloned().unwrap_or_default(),
    })
}

/// Download the function's deployment package (presigned URL from a *fresh*
/// `GetFunction` so the URL isn't expired) and unzip it into `dest_dir` (which
/// the caller makes unique per download, so history is kept). Returns
/// `(dest_dir, files_written)`.
pub async fn download_and_extract_lambda_code(
    client: LambdaClient,
    function_name: String,
    dest_dir: std::path::PathBuf,
) -> Result<(std::path::PathBuf, usize)> {
    let info = fetch_lambda_code(client, function_name).await?;
    if !info.is_zip() {
        return Err(crate::error::Error::AwsSdk(
            "Container-image function — no zip package to download".to_string(),
        ));
    }
    let url = info.code_location.ok_or_else(|| {
        crate::error::Error::AwsSdk("GetFunction returned no code location".to_string())
    })?;

    // Network download + unzip are blocking — keep them off the async runtime.
    let dest_for_return = dest_dir.clone();
    let count = tokio::task::spawn_blocking(move || -> std::io::Result<usize> {
        use std::io::Read;
        let io_err =
            |e: String| std::io::Error::new(std::io::ErrorKind::Other, e);

        // Pull the whole package into memory (Lambda packages are ≤ ~250 MB
        // unzipped, usually far smaller).
        let resp = ureq::get(&url).call().map_err(|e| io_err(e.to_string()))?;
        let mut bytes: Vec<u8> = Vec::new();
        resp.into_reader().read_to_end(&mut bytes)?;

        // Extraction dir (caller picks a unique, timestamped path so prior
        // downloads are kept as history rather than overwritten).
        std::fs::create_dir_all(&dest_dir)?;

        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
            .map_err(|e| io_err(e.to_string()))?;
        let mut count = 0usize;
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i).map_err(|e| io_err(e.to_string()))?;
            // `enclosed_name` rejects absolute / `..` paths (zip-slip safe).
            let outpath = match entry.enclosed_name() {
                Some(p) => dest_dir.join(p),
                None => continue,
            };
            if entry.is_dir() {
                std::fs::create_dir_all(&outpath)?;
            } else {
                if let Some(parent) = outpath.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let mut outfile = std::fs::File::create(&outpath)?;
                std::io::copy(&mut entry, &mut outfile)?;
                count += 1;
            }
        }
        Ok(count)
    })
    .await
    .map_err(|e| crate::error::Error::AwsSdk(format!("download task failed: {e}")))??;

    Ok((dest_for_return, count))
}
