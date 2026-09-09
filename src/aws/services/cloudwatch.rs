use crate::aws::client::AwsClients;
use crate::aws::resource::{native_state_label, Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use aws_sdk_cloudwatch::Client as CwClient;
use aws_sdk_cloudwatchlogs::Client as LogsClient;
use std::any::Any;
use std::collections::HashMap;
use tokio::sync::mpsc;

pub struct CloudWatchService {
    cw_client: CwClient,
    logs_client: LogsClient,
    oam_client: aws_sdk_oam::Client,
}

impl CloudWatchService {
    pub fn new(aws_clients: &AwsClients) -> Self {
        Self {
            cw_client: aws_clients.cloudwatch_client(),
            logs_client: aws_clients.logs_client(),
            oam_client: aws_clients.oam_client(),
        }
    }
}

#[async_trait]
impl AwsService for CloudWatchService {
    fn service_type(&self) -> ServiceType {
        ServiceType::CloudWatch
    }

    fn name(&self) -> &str {
        "CloudWatch"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        self.list_resources_streaming(tx, ServiceType::CloudWatch).await?;
        Ok(vec![])
    }

    async fn list_resources_streaming(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        let mut total = 0usize;

        // ── Alarms ──────────────────────────────────────────────────────────
        let mut paginator = self.cw_client.describe_alarms().into_paginator().send();
        while let Some(result) = paginator.next().await {
            match result {
                Ok(page) => {
                    let mut batch: Vec<Box<dyn Resource>> = page
                        .metric_alarms()
                        .iter()
                        .map(|a| Box::new(CwAlarm::from_sdk(a)) as Box<dyn Resource>)
                        .collect();
                    batch.extend(
                        page.composite_alarms()
                            .iter()
                            .map(|a| Box::new(CwCompositeAlarm::from_sdk(a)) as Box<dyn Resource>),
                    );
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
                            status_message: Some("Loading alarms…".to_string()),
                        },
                    });
                }
                Err(e) => {
                    let _ = event_tx.send(Event::ResourceLoadError {
                        service: service_type,
                        error: format!("Failed to list CloudWatch alarms: {}", e),
                    });
                    return Ok(());
                }
            }
        }

        // ── Log Groups ───────────────────────────────────────────────────────
        let mut paginator = self.logs_client.describe_log_groups().into_paginator().send();
        while let Some(result) = paginator.next().await {
            match result {
                Ok(page) => {
                    let batch: Vec<Box<dyn Resource>> = page
                        .log_groups()
                        .iter()
                        .map(|g| Box::new(CwLogGroup::from_sdk(g)) as Box<dyn Resource>)
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
                            status_message: Some("Loading log groups…".to_string()),
                        },
                    });
                }
                Err(e) => {
                    let _ = event_tx.send(Event::ResourceLoadError {
                        service: service_type,
                        error: format!("Failed to list CloudWatch log groups: {}", e),
                    });
                    return Ok(());
                }
            }
        }

        // ── Dashboards ───────────────────────────────────────────────────────
        let mut paginator = self.cw_client.list_dashboards().into_paginator().send();
        while let Some(result) = paginator.next().await {
            match result {
                Ok(page) => {
                    let batch: Vec<Box<dyn Resource>> = page
                        .dashboard_entries()
                        .iter()
                        .map(|d| Box::new(CwDashboard::from_sdk(d)) as Box<dyn Resource>)
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
                            status_message: Some("Loading dashboards…".to_string()),
                        },
                    });
                }
                // Dashboards are non-fatal — a missing list permission shouldn't
                // blank the alarms/log groups already loaded.
                Err(e) => {
                    let _ = event_tx.send(Event::ResourceLoadWarning {
                        service: service_type,
                        warning: format!(
                            "dashboards: {}",
                            crate::error::sdk_error_message(&e)
                        ),
                    });
                    break;
                }
            }
        }

        // ── Metric streams (Streams tab) ─────────────────────────────────────
        let mut paginator = self.cw_client.list_metric_streams().into_paginator().send();
        while let Some(result) = paginator.next().await {
            match result {
                Ok(page) => {
                    let batch: Vec<Box<dyn Resource>> = page
                        .entries()
                        .iter()
                        .map(|e| Box::new(CwMetricStream::from_sdk(e)) as Box<dyn Resource>)
                        .collect();
                    if batch.is_empty() {
                        continue;
                    }
                    total += batch.len();
                    let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                        service: service_type,
                        resources: batch,
                        progress: LoadProgress {
                            loaded_count: total,
                            total_count: None,
                            status_message: Some("Loading metric streams…".to_string()),
                        },
                    });
                }
                Err(e) => {
                    let _ = event_tx.send(Event::ResourceLoadWarning {
                        service: service_type,
                        warning: format!("metric streams: {}", crate::error::sdk_error_message(&e)),
                    });
                    break;
                }
            }
        }

        // ── Anomaly detectors + Contributor Insights rules (Insights tab) ────
        let mut paginator = self
            .cw_client
            .describe_anomaly_detectors()
            .into_paginator()
            .send();
        while let Some(result) = paginator.next().await {
            match result {
                Ok(page) => {
                    let batch: Vec<Box<dyn Resource>> = page
                        .anomaly_detectors()
                        .iter()
                        .map(|a| Box::new(CwAnomalyDetector::from_sdk(a)) as Box<dyn Resource>)
                        .collect();
                    if batch.is_empty() {
                        continue;
                    }
                    total += batch.len();
                    let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                        service: service_type,
                        resources: batch,
                        progress: LoadProgress {
                            loaded_count: total,
                            total_count: None,
                            status_message: Some("Loading anomaly detectors…".to_string()),
                        },
                    });
                }
                Err(e) => {
                    let _ = event_tx.send(Event::ResourceLoadWarning {
                        service: service_type,
                        warning: format!(
                            "anomaly detectors: {}",
                            crate::error::sdk_error_message(&e)
                        ),
                    });
                    break;
                }
            }
        }
        let mut paginator = self.cw_client.describe_insight_rules().into_paginator().send();
        while let Some(result) = paginator.next().await {
            match result {
                Ok(page) => {
                    let batch: Vec<Box<dyn Resource>> = page
                        .insight_rules()
                        .iter()
                        .map(|r| Box::new(CwInsightRule::from_sdk(r)) as Box<dyn Resource>)
                        .collect();
                    if batch.is_empty() {
                        continue;
                    }
                    total += batch.len();
                    let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                        service: service_type,
                        resources: batch,
                        progress: LoadProgress {
                            loaded_count: total,
                            total_count: None,
                            status_message: Some("Loading insight rules…".to_string()),
                        },
                    });
                }
                Err(e) => {
                    let _ = event_tx.send(Event::ResourceLoadWarning {
                        service: service_type,
                        warning: format!(
                            "contributor insights rules: {}",
                            crate::error::sdk_error_message(&e)
                        ),
                    });
                    break;
                }
            }
        }

        // ── Account-level Logs policies (Account Policies tab) ───────────────
        // Per-type failures stay silent (newer policy types don't exist in
        // every region/partition — a per-type warning would fire everywhere);
        // all five failing means something real, so that warns with a sample.
        {
            let (policies, errors) =
                fetch_account_policies(self.logs_client.clone()).await;
            if errors.len() >= 5 {
                let _ = event_tx.send(Event::ResourceLoadWarning {
                    service: service_type,
                    warning: format!("account policies: {}", errors[0]),
                });
            }
            if !policies.is_empty() {
                let batch: Vec<Box<dyn Resource>> = policies
                    .into_iter()
                    .map(|p| Box::new(p) as Box<dyn Resource>)
                    .collect();
                total += batch.len();
                let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                    service: service_type,
                    resources: batch,
                    progress: LoadProgress {
                        loaded_count: total,
                        total_count: None,
                        status_message: Some("Loading account policies…".to_string()),
                    },
                });
            }
        }

        // ── OAM sinks + links (Cross-Account tab) ────────────────────────────
        // Best-effort: both sides are listed (an account is normally one or the
        // other, the empty side just returns nothing). Two cheap paginated
        // lists; the sink policy / attached links / link config are lazy.
        match crate::aws::services::oam::fetch_sinks(self.oam_client.clone()).await {
            Ok(sinks) if !sinks.is_empty() => {
                let batch: Vec<Box<dyn Resource>> = sinks
                    .into_iter()
                    .map(|s| Box::new(s) as Box<dyn Resource>)
                    .collect();
                total += batch.len();
                let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                    service: service_type,
                    resources: batch,
                    progress: LoadProgress {
                        loaded_count: total,
                        total_count: None,
                        status_message: Some("Loading cross-account sinks…".to_string()),
                    },
                });
            }
            Ok(_) => {}
            Err(e) => {
                let _ = event_tx.send(Event::ResourceLoadWarning {
                    service: service_type,
                    warning: format!("cross-account sinks: {}", e),
                });
            }
        }
        match crate::aws::services::oam::fetch_links(self.oam_client.clone()).await {
            Ok(links) if !links.is_empty() => {
                let batch: Vec<Box<dyn Resource>> = links
                    .into_iter()
                    .map(|l| Box::new(l) as Box<dyn Resource>)
                    .collect();
                total += batch.len();
                let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                    service: service_type,
                    resources: batch,
                    progress: LoadProgress {
                        loaded_count: total,
                        total_count: None,
                        status_message: Some("Loading cross-account links…".to_string()),
                    },
                });
            }
            Ok(_) => {}
            Err(e) => {
                let _ = event_tx.send(Event::ResourceLoadWarning {
                    service: service_type,
                    warning: format!("cross-account links: {}", e),
                });
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

// ── CwAlarm ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct CwAlarm {
    pub alarm_name: String,
    pub alarm_arn: String,
    pub alarm_description: String,
    pub state: String,
    pub namespace: String,
    pub metric_name: String,
    pub period: i32,
    pub statistic: String,
    pub threshold: f64,
    pub comparison_operator: String,
    pub evaluation_periods: Option<i32>,
    pub datapoints_to_alarm: Option<i32>,
    pub dimensions: Vec<(String, String)>,
    pub alarm_actions: Vec<String>,
    pub ok_actions: Vec<String>,
    pub insufficient_data_actions: Vec<String>,
    pub treat_missing_data: String,
    pub tags: HashMap<String, String>,
}

impl CwAlarm {
    pub fn from_sdk(a: &aws_sdk_cloudwatch::types::MetricAlarm) -> Self {
        let dimensions = a
            .dimensions()
            .iter()
            .map(|d| {
                (
                    d.name().unwrap_or("").to_string(),
                    d.value().unwrap_or("").to_string(),
                )
            })
            .collect();

        Self {
            alarm_name: a.alarm_name().unwrap_or("").to_string(),
            alarm_arn: a.alarm_arn().unwrap_or("").to_string(),
            alarm_description: a.alarm_description().unwrap_or("").to_string(),
            state: a
                .state_value()
                .map(|s| s.as_str().to_string())
                .unwrap_or_default(),
            namespace: a.namespace().unwrap_or("").to_string(),
            metric_name: a.metric_name().unwrap_or("").to_string(),
            period: a.period().unwrap_or(60),
            statistic: a
                .statistic()
                .map(|s| s.as_str().to_string())
                .unwrap_or_default(),
            threshold: a.threshold().unwrap_or(0.0),
            comparison_operator: a
                .comparison_operator()
                .map(|c| c.as_str().to_string())
                .unwrap_or_default(),
            evaluation_periods: a.evaluation_periods(),
            datapoints_to_alarm: a.datapoints_to_alarm(),
            dimensions,
            alarm_actions: a.alarm_actions().iter().map(|s| s.to_string()).collect(),
            ok_actions: a.ok_actions().iter().map(|s| s.to_string()).collect(),
            insufficient_data_actions: a
                .insufficient_data_actions()
                .iter()
                .map(|s| s.to_string())
                .collect(),
            treat_missing_data: a.treat_missing_data().unwrap_or("").to_string(),
            tags: HashMap::new(),
        }
    }

    pub fn state_display(&self) -> &str {
        match self.state.as_str() {
            "OK" => "OK",
            "ALARM" => "ALARM",
            "INSUFFICIENT_DATA" => "INSUFFICIENT",
            other => other,
        }
    }

    pub fn comparison_display(&self) -> String {
        match self.comparison_operator.as_str() {
            "GreaterThanOrEqualToThreshold" => ">=".to_string(),
            "GreaterThanThreshold" => ">".to_string(),
            "LessThanThreshold" => "<".to_string(),
            "LessThanOrEqualToThreshold" => "<=".to_string(),
            "LessThanLowerOrGreaterThanUpperThreshold" => "<low or >high".to_string(),
            other => other.to_string(),
        }
    }
}

crate::sections! {
    pub enum CwAlarmDetailSection,
    pub static CW_ALARM_SECTIONS = [
        Config "Config",
        Actions "Actions" => crate::app::App::trigger_cw_history_load,
        History "History" => crate::app::App::trigger_cw_history_load,
    ]
}

impl Resource for CwAlarm {
    fn references(&self) -> Vec<(String, String)> {
        let mut v: Vec<(String, String)> = self
            .dimensions
            .iter()
            .map(|(k, val)| (format!("Dimension {}", k), val.clone()))
            .collect();
        for x in self
            .alarm_actions
            .iter()
            .chain(&self.ok_actions)
            .chain(&self.insufficient_data_actions)
        {
            v.push(("Action".to_string(), x.clone()));
        }
        v
    }

    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&CW_ALARM_SECTIONS)
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws cloudwatch describe-alarms --alarm-names {}",
            crate::aws::resource::shell_quote(&self.alarm_name)
        ))
    }

    fn id(&self) -> &str {
        &self.alarm_arn
    }

    fn name(&self) -> &str {
        &self.alarm_name
    }

    fn resource_type(&self) -> &str {
        "CW Alarm"
    }

    fn is_noise(&self) -> bool {
        // An OK alarm isn't firing — hide by default so what's in ALARM stands out.
        self.state == "OK"
    }

    fn state(&self) -> ResourceState {
        match self.state.as_str() {
            "OK" => ResourceState::Available,
            "ALARM" => ResourceState::Unavailable,
            "INSUFFICIENT_DATA" => ResourceState::Unknown("INSUFFICIENT".to_string()),
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
        // Include the state so "alarm"/"ok"/"insufficient" filter the list (mirrors
        // the Trusted Advisor status-searchability fix).
        let state_label = match self.state.as_str() {
            "ALARM" => "alarm in alarm",
            "OK" => "ok",
            "INSUFFICIENT_DATA" => "insufficient data",
            other => other,
        };
        format!(
            "{} {} {} {} {}",
            self.alarm_name, self.alarm_arn, self.namespace, self.metric_name, state_label
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Alarm".to_string(), self.alarm_name.clone()),
            ("State".to_string(), self.state.clone()),
            ("Namespace".to_string(), self.namespace.clone()),
            ("Metric".to_string(), self.metric_name.clone()),
        ]
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/cloudwatch/home?region={}#alarmsV2:alarm/{}",
            region,
            region,
            urlencoding_simple(&self.alarm_name)
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── CwCompositeAlarm ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct CwCompositeAlarm {
    pub alarm_name: String,
    pub alarm_arn: String,
    pub alarm_description: String,
    pub state: String,
    pub state_reason: String,
    pub alarm_rule: String,
    pub actions_enabled: bool,
    pub alarm_actions: Vec<String>,
    pub ok_actions: Vec<String>,
    pub insufficient_data_actions: Vec<String>,
    pub tags: HashMap<String, String>,
}

impl CwCompositeAlarm {
    pub fn from_sdk(a: &aws_sdk_cloudwatch::types::CompositeAlarm) -> Self {
        Self {
            alarm_name: a.alarm_name().unwrap_or("").to_string(),
            alarm_arn: a.alarm_arn().unwrap_or("").to_string(),
            alarm_description: a.alarm_description().unwrap_or("").to_string(),
            state: a
                .state_value()
                .map(|s| s.as_str().to_string())
                .unwrap_or_default(),
            state_reason: a.state_reason().unwrap_or("").to_string(),
            alarm_rule: a.alarm_rule().unwrap_or("").to_string(),
            actions_enabled: a.actions_enabled().unwrap_or(false),
            alarm_actions: a.alarm_actions().iter().map(|s| s.to_string()).collect(),
            ok_actions: a.ok_actions().iter().map(|s| s.to_string()).collect(),
            insufficient_data_actions: a
                .insufficient_data_actions()
                .iter()
                .map(|s| s.to_string())
                .collect(),
            tags: HashMap::new(),
        }
    }

    pub fn state_display(&self) -> &str {
        match self.state.as_str() {
            "OK" => "OK",
            "ALARM" => "ALARM",
            "INSUFFICIENT_DATA" => "INSUFFICIENT",
            other => other,
        }
    }
}

/// Extract the child-alarm references (names or ARNs) from a composite alarm
/// rule. Every reference in a rule is quoted — `ALARM("x") OR OK("y")` — so the
/// quoted substrings are exactly the children, in rule order (deduped).
pub fn alarm_rule_children(rule: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut in_q = false;
    for c in rule.chars() {
        if c == '"' {
            if in_q {
                let tok = std::mem::take(&mut cur);
                if !tok.is_empty() && !out.contains(&tok) {
                    out.push(tok);
                }
                in_q = false;
            } else {
                in_q = true;
            }
        } else if in_q {
            cur.push(c);
        }
    }
    out
}

crate::sections! {
    pub enum CwCompositeAlarmDetailSection,
    pub static CW_COMPOSITE_ALARM_SECTIONS = [
        Rule "Rule",
        Actions "Actions",
    ]
}

impl Resource for CwCompositeAlarm {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&CW_COMPOSITE_ALARM_SECTIONS)
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws cloudwatch describe-alarms --alarm-types CompositeAlarm --alarm-names {}",
            crate::aws::resource::shell_quote(&self.alarm_name)
        ))
    }

    fn id(&self) -> &str {
        &self.alarm_arn
    }

    fn name(&self) -> &str {
        &self.alarm_name
    }

    fn resource_type(&self) -> &str {
        // Deliberately "CW Alarm" (not "CW Composite Alarm") so composites pass
        // the Alarms sub-tab's type filter (`resource_type() == "CW Alarm"`) and
        // appear alongside metric alarms. The split-pane renderer / detection
        // downcasts on the concrete type, so they stay distinguishable; the
        // detail pane header still labels them "Composite".
        "CW Alarm"
    }

    fn is_noise(&self) -> bool {
        // An OK alarm isn't firing — hide by default so what's in ALARM stands out.
        self.state == "OK"
    }

    fn state(&self) -> ResourceState {
        match self.state.as_str() {
            "OK" => ResourceState::Available,
            "ALARM" => ResourceState::Unavailable,
            "INSUFFICIENT_DATA" => ResourceState::Unknown("INSUFFICIENT".to_string()),
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
        let state_label = match self.state.as_str() {
            "ALARM" => "alarm in alarm",
            "OK" => "ok",
            "INSUFFICIENT_DATA" => "insufficient data",
            other => other,
        };
        format!(
            "{} {} composite {} {}",
            self.alarm_name, self.alarm_arn, self.alarm_rule, state_label
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Alarm".to_string(), self.alarm_name.clone()),
            ("Type".to_string(), "Composite".to_string()),
            ("State".to_string(), self.state.clone()),
            ("Rule".to_string(), self.alarm_rule.clone()),
        ]
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/cloudwatch/home?region={}#alarmsV2:alarm/{}",
            region,
            region,
            urlencoding_simple(&self.alarm_name)
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── CwDashboard ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct CwDashboard {
    pub name: String,
    pub arn: String,
    pub last_modified: Option<i64>,
    pub size_bytes: i64,
    pub tags: HashMap<String, String>,
}

impl CwDashboard {
    pub fn from_sdk(d: &aws_sdk_cloudwatch::types::DashboardEntry) -> Self {
        Self {
            name: d.dashboard_name().unwrap_or("").to_string(),
            arn: d.dashboard_arn().unwrap_or("").to_string(),
            last_modified: d.last_modified().map(|t| t.secs()),
            size_bytes: d.size().unwrap_or(0),
            tags: HashMap::new(),
        }
    }
}

// One section. The widget *summary* list this pane used to lead with was a
// stand-in for a dashboard nobody could see; `m` now draws the real grid, so
// the summary was a worse copy of it sitting in the way. What's left is the
// thing the grid can't show you — the body itself.
crate::sections! {
    pub enum CwDashboardDetailSection,
    pub static CW_DASHBOARD_SECTIONS = [
        Raw "JSON" => crate::app::App::trigger_cw_dashboard_load,
    ]
}

impl Resource for CwDashboard {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&CW_DASHBOARD_SECTIONS)
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws cloudwatch get-dashboard --dashboard-name {}",
            crate::aws::resource::shell_quote(self.id())
        ))
    }

    fn id(&self) -> &str {
        &self.name
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn resource_type(&self) -> &str {
        "CW Dashboard"
    }

    fn state(&self) -> ResourceState {
        ResourceState::stateless()
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!("{} {} dashboard", self.name, self.arn)
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![("Dashboard".to_string(), self.name.clone())]
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/cloudwatch/home?region={}#dashboards:name={}",
            region,
            region,
            urlencoding_simple(&self.name)
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// One metric line from a widget's `properties.metrics` array, with the
/// console's dot-repeat notation already expanded (see `parse_widget_metrics`).
#[derive(Debug, Clone, PartialEq)]
pub struct CwWidgetMetric {
    pub namespace: String,
    pub metric_name: String,
    pub dimensions: Vec<(String, String)>,
    /// The series label the console would draw in the legend.
    pub label: String,
    /// Per-metric overrides of the widget-level `stat` / `period` / `region`.
    pub stat: Option<String>,
    pub period: Option<i32>,
    pub region: Option<String>,
    /// Metric-math entries (`{"expression": …}`). The expression references its
    /// siblings by their author-assigned `id`, so both travel together.
    pub expression: Option<String>,
    /// The author's `id` for this line, if any — what an expression refers to.
    pub id: Option<String>,
    /// `false` for a line that only exists to feed an expression (the console's
    /// `"visible": false`). Fetched with `ReturnData=false` and never drawn.
    pub visible: bool,
    /// `"yAxis": "right"` — plotted against the widget's second scale.
    pub right_axis: bool,
}

/// A `properties.annotations.horizontal` entry — a reference line the author
/// drew across a widget (a limit, an SLO, an alarm threshold).
#[derive(Debug, Clone, PartialEq)]
pub struct CwAnnotation {
    pub label: String,
    pub value: f64,
}

/// One parsed widget from a dashboard body — the readable Widgets section and
/// the `m` grid renderer both read this.
#[derive(Debug, Clone)]
pub struct CwDashboardWidget {
    pub kind: String,          // metric / text / alarm / log / …
    pub title: String,         // properties.title (or "")
    pub alarm_arns: Vec<String>, // alarm widgets: referenced alarm ARNs (jumpable)
    // Position on the console's 24-column grid. Absent x/y auto-flow (see
    // `parse_dashboard_body`), matching how the console lays such a body out.
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
    pub view: String,          // properties.view: timeSeries / singleValue / gauge / bar / pie
    pub markdown: String,      // text widgets: the raw markdown body
    pub metrics: Vec<CwWidgetMetric>,
    pub stat: Option<String>,  // widget-level defaults for its metric lines
    pub period: Option<i32>,
    pub region: Option<String>,
    /// Reference lines drawn across the plot (`annotations.horizontal`).
    pub annotations: Vec<CwAnnotation>,
    /// `yAxis.left` bounds — what makes a gauge a gauge (it has no meaning
    /// without a scale), and a hint for the chart's y-range otherwise.
    pub y_min: Option<f64>,
    pub y_max: Option<f64>,
    /// `yAxis.right` bounds, for widgets that pair two different scales.
    pub y_right_min: Option<f64>,
    pub y_right_max: Option<f64>,
    /// singleValue widgets with `"sparkline": true` draw a trend under the number.
    pub sparkline: bool,
    /// `"stacked": true` — series are cumulative, showing a total and its
    /// composition rather than independent lines.
    pub stacked: bool,
}

#[derive(Debug, Clone)]
pub struct CwDashboardBody {
    pub raw_json: String,
    pub widgets: Vec<CwDashboardWidget>,
    pub settings: CwDashboardSettings,
}

/// Body-level display settings — the window the author chose to view their own
/// dashboard through, which is a better opening range than any default of ours.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CwDashboardSettings {
    /// Nearest preset covering the body's relative `start` (`"-PT3H"`).
    /// Absolute start/end timestamps are ignored — the pane's ranges are all
    /// relative to now.
    pub range: Option<MetricsTimeRange>,
    /// `"periodOverride": "inherit"` — every widget's own period is discarded in
    /// favour of one derived from the time range.
    pub period_inherit: bool,
}

/// Read the body's `start` / `periodOverride`.
///
/// `start` is an ISO-8601 duration relative to now (`-PT3H`, `-P7D`). It rarely
/// lands on one of our presets, so it resolves to the **smallest preset that
/// covers it** — showing a little more than the author asked for beats showing
/// less than they needed.
pub fn parse_dashboard_settings(body: &str) -> CwDashboardSettings {
    let value: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(_) => return CwDashboardSettings::default(),
    };
    CwDashboardSettings {
        range: value
            .get("start")
            .and_then(|s| s.as_str())
            .and_then(parse_iso8601_duration)
            .map(covering_range),
        period_inherit: value
            .get("periodOverride")
            .and_then(|p| p.as_str())
            .map(|p| p.eq_ignore_ascii_case("inherit"))
            .unwrap_or(false),
    }
}

/// `-PT3H` / `-P7D` / `-P1DT12H` → seconds. Absolute timestamps and the
/// month/year designators (which aren't fixed-length) yield `None`.
fn parse_iso8601_duration(s: &str) -> Option<i64> {
    let s = s.trim().trim_start_matches('-').strip_prefix('P')?;
    let (date, time) = match s.split_once('T') {
        Some((d, t)) => (d, t),
        None => (s, ""),
    };

    let mut total: i64 = 0;
    let mut number = String::new();
    for (part, units) in [(date, [('D', 86_400), ('W', 604_800)].as_slice()),
                          (time, [('H', 3_600), ('M', 60), ('S', 1)].as_slice())] {
        for ch in part.chars() {
            if ch.is_ascii_digit() {
                number.push(ch);
                continue;
            }
            let n: i64 = number.parse().ok()?;
            number.clear();
            // A designator we don't recognise (M for months, Y) can't be turned
            // into a fixed number of seconds — give up rather than guess.
            total += n * units.iter().find(|(c, _)| *c == ch).map(|(_, mul)| *mul)?;
        }
    }
    (total > 0).then_some(total)
}

/// Smallest preset whose window covers `secs`, saturating at the longest.
fn covering_range(secs: i64) -> MetricsTimeRange {
    for r in [
        MetricsTimeRange::OneHour,
        MetricsTimeRange::SixHours,
        MetricsTimeRange::TwentyFourHours,
    ] {
        if secs <= r.duration_secs() {
            return r;
        }
    }
    MetricsTimeRange::SevenDays
}

/// Fetch + parse a dashboard's body (lazy, on first view of its detail pane).
/// Pretty-prints the raw JSON for the Raw section and extracts a readable
/// per-widget summary for the Widgets section.
pub async fn fetch_dashboard(client: CwClient, name: String) -> Result<CwDashboardBody> {
    let resp = client
        .get_dashboard()
        .dashboard_name(&name)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;

    let body = resp.dashboard_body().unwrap_or("").to_string();
    let (raw_json, widgets) = parse_dashboard_body(&body);
    Ok(CwDashboardBody {
        raw_json,
        widgets,
        settings: parse_dashboard_settings(&body),
    })
}

/// Parse a dashboard body JSON into (pretty-printed JSON, widget summaries).
/// Defensive: malformed JSON falls back to the raw string + no widgets.
pub fn parse_dashboard_body(body: &str) -> (String, Vec<CwDashboardWidget>) {
    let value: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(_) => return (body.to_string(), Vec::new()),
    };
    let pretty = serde_json::to_string_pretty(&value).unwrap_or_else(|_| body.to_string());

    let mut widgets = Vec::new();
    // Auto-flow cursor for bodies that omit x/y (the console packs those
    // left-to-right, wrapping at the 24th column).
    let (mut flow_x, mut flow_y, mut flow_row_h) = (0u16, 0u16, 0u16);
    if let Some(arr) = value.get("widgets").and_then(|w| w.as_array()) {
        for w in arr {
            let props = w.get("properties");
            let kind = w
                .get("type")
                .and_then(|t| t.as_str())
                .unwrap_or("widget")
                .to_string();
            let title = props
                .and_then(|p| p.get("title"))
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .to_string();

            let mut alarm_arns = Vec::new();
            let mut markdown = String::new();

            match kind.as_str() {
                "text" => {
                    markdown = props
                        .and_then(|p| p.get("markdown"))
                        .and_then(|t| t.as_str())
                        .unwrap_or("")
                        .to_string();
                }
                "alarm" => {
                    if let Some(alarms) = props
                        .and_then(|p| p.get("alarms"))
                        .and_then(|a| a.as_array())
                    {
                        for a in alarms {
                            if let Some(arn) = a.as_str() {
                                alarm_arns.push(arn.to_string());
                            }
                        }
                    }
                }
                _ => {}
            }

            let width = w.get("width").and_then(|v| v.as_u64()).unwrap_or(6).min(24) as u16;
            let height = w.get("height").and_then(|v| v.as_u64()).unwrap_or(6) as u16;
            let (x, y) = match (
                w.get("x").and_then(|v| v.as_u64()),
                w.get("y").and_then(|v| v.as_u64()),
            ) {
                (Some(x), Some(y)) => (x.min(23) as u16, y as u16),
                _ => {
                    if flow_x + width > 24 {
                        flow_x = 0;
                        flow_y += flow_row_h;
                        flow_row_h = 0;
                    }
                    let pos = (flow_x, flow_y);
                    flow_x += width;
                    flow_row_h = flow_row_h.max(height);
                    pos
                }
            };

            widgets.push(CwDashboardWidget {
                kind,
                title,
                alarm_arns,
                x,
                y,
                width,
                height,
                view: props
                    .and_then(|p| p.get("view"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("timeSeries")
                    .to_string(),
                markdown,
                metrics: parse_widget_metrics(props),
                stat: props
                    .and_then(|p| p.get("stat"))
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                period: props
                    .and_then(|p| p.get("period"))
                    .and_then(|v| v.as_i64())
                    .map(|v| v as i32),
                region: props
                    .and_then(|p| p.get("region"))
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                annotations: parse_horizontal_annotations(props),
                y_min: props
                    .and_then(|p| p.pointer("/yAxis/left/min"))
                    .and_then(|v| v.as_f64()),
                y_max: props
                    .and_then(|p| p.pointer("/yAxis/left/max"))
                    .and_then(|v| v.as_f64()),
                y_right_min: props
                    .and_then(|p| p.pointer("/yAxis/right/min"))
                    .and_then(|v| v.as_f64()),
                y_right_max: props
                    .and_then(|p| p.pointer("/yAxis/right/max"))
                    .and_then(|v| v.as_f64()),
                sparkline: props
                    .and_then(|p| p.get("sparkline"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                stacked: props
                    .and_then(|p| p.get("stacked"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
            });
        }
    }

    (pretty, widgets)
}

/// Read `properties.annotations.horizontal`. Each entry is either a single
/// `{label, value}` line or a **band** — a two-element array marking a shaded
/// range. A band's edges are the interesting part in a terminal (there is no
/// shading), so both are kept as lines.
fn parse_horizontal_annotations(props: Option<&serde_json::Value>) -> Vec<CwAnnotation> {
    let arr = match props
        .and_then(|p| p.pointer("/annotations/horizontal"))
        .and_then(|v| v.as_array())
    {
        Some(a) => a,
        None => return Vec::new(),
    };

    let one = |v: &serde_json::Value| -> Option<CwAnnotation> {
        Some(CwAnnotation {
            label: v.get("label").and_then(|l| l.as_str()).unwrap_or("").to_string(),
            value: v.get("value").and_then(|n| n.as_f64())?,
        })
    };

    arr.iter()
        .flat_map(|entry| match entry.as_array() {
            Some(band) => band.iter().filter_map(one).collect::<Vec<_>>(),
            None => one(entry).into_iter().collect(),
        })
        .collect()
}

/// Expand a widget's `properties.metrics` array into concrete metric specs.
///
/// The console writes each line as `[Namespace, MetricName, (DimName, DimValue)…,
/// {options}?]`, and abbreviates any token equal to the one directly above it as
/// `"."` — so a four-series widget usually spells the namespace out once. A line
/// may instead be a lone `{"expression": …}` object (metric math), which we keep
/// as a spec so the widget can say what it is, but never fetch.
fn parse_widget_metrics(props: Option<&serde_json::Value>) -> Vec<CwWidgetMetric> {
    let arr = match props.and_then(|p| p.get("metrics")).and_then(|m| m.as_array()) {
        Some(a) => a,
        None => return Vec::new(),
    };

    let mut out: Vec<CwWidgetMetric> = Vec::new();
    // Tokens of the previous *metric* line — what a "." resolves against. An
    // expression line has no tokens and must not reset it.
    let mut prev: Vec<String> = Vec::new();

    for entry in arr {
        let items = match entry.as_array() {
            Some(i) => i,
            None => continue,
        };
        // The options object is always last (and may be the only element).
        let opts = items.last().and_then(|v| v.as_object());
        let token_count = if opts.is_some() { items.len().saturating_sub(1) } else { items.len() };

        let get = |k: &str| opts.and_then(|o| o.get(k));
        let expression = get("expression").and_then(|v| v.as_str()).map(str::to_string);
        let label = get("label").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let stat = get("stat").and_then(|v| v.as_str()).map(str::to_string);
        let period = get("period").and_then(|v| v.as_i64()).map(|v| v as i32);
        let region = get("region").and_then(|v| v.as_str()).map(str::to_string);
        let id = get("id").and_then(|v| v.as_str()).map(str::to_string);
        let visible = get("visible").and_then(|v| v.as_bool()).unwrap_or(true);
        let right_axis = get("yAxis").and_then(|v| v.as_str()) == Some("right");

        if token_count == 0 {
            if expression.is_some() {
                out.push(CwWidgetMetric {
                    namespace: String::new(),
                    metric_name: String::new(),
                    dimensions: Vec::new(),
                    label,
                    stat,
                    period,
                    region,
                    expression,
                    id,
                    visible,
                    right_axis,
                });
            }
            continue;
        }

        let mut tokens: Vec<String> = Vec::with_capacity(token_count);
        for (i, item) in items.iter().take(token_count).enumerate() {
            let raw = item.as_str().unwrap_or("");
            if raw == "." {
                tokens.push(prev.get(i).cloned().unwrap_or_default());
            } else {
                tokens.push(raw.to_string());
            }
        }

        let namespace = tokens.first().cloned().unwrap_or_default();
        let metric_name = tokens.get(1).cloned().unwrap_or_default();
        let dimensions: Vec<(String, String)> = tokens[2.min(tokens.len())..]
            .chunks(2)
            .filter(|c| c.len() == 2 && !c[0].is_empty())
            .map(|c| (c[0].clone(), c[1].clone()))
            .collect();

        let label = if !label.is_empty() {
            label
        } else if let Some((_, v)) = dimensions.last() {
            format!("{} {}", v, metric_name)
        } else {
            metric_name.clone()
        };

        prev = tokens;
        out.push(CwWidgetMetric {
            namespace,
            metric_name,
            dimensions,
            label,
            stat,
            period,
            region,
            expression,
            id,
            visible,
            right_axis,
        });
    }

    out
}

// ── Dashboard rendering (`m`) — the widget grid, charted ──────────────────────

/// Ceiling on how many series one dashboard fetch asks for. `GetMetricData`
/// allows 500 queries per call, but it also *bills per metric requested* — a
/// dashboard with a hundred-series widget would be a surprise charge on a
/// keystroke. Widgets past the cap render as "not fetched" rather than silently
/// looking empty.
pub const MAX_DASHBOARD_QUERIES: usize = 100;

/// One returned time series. A plain metric query yields exactly one; a
/// `SEARCH()` expression yields many, all sharing the query id and told apart
/// only by their label — which is why series are stored as a list per id.
#[derive(Debug, Clone)]
pub struct CwSeries {
    pub label: String,
    pub points: Vec<(f64, f64)>,
}

/// One alarm referenced by an alarm widget, resolved to its current state.
#[derive(Debug, Clone)]
pub struct CwAlarmChip {
    pub name: String,
    pub state: String,
    pub reason: String,
}

#[derive(Debug, Clone)]
pub struct CwDashboardMetricsData {
    pub time_range: MetricsTimeRange,
    pub x_max: f64,
    /// Every widget in body order — non-metric ones included, so the grid can
    /// draw text panels and placeholders in their real positions.
    pub widgets: Vec<CwDashboardWidget>,
    /// Series keyed by `GetMetricData` query id (`dashboard_query_id`).
    pub series: HashMap<String, Vec<CwSeries>>,
    /// Alarm states for alarm widgets, keyed by alarm name.
    pub alarms: HashMap<String, CwAlarmChip>,
    /// Regions whose widgets couldn't be fetched at all (no credentials there,
    /// the call failed) — distinct from regions we *did* reach.
    pub skipped_regions: Vec<String>,
    /// Set when the request had to be retried without its metric-math queries.
    pub math_error: Option<String>,
    pub capped: bool,
}

#[derive(Debug, Clone)]
pub enum CwDashboardMetricsState {
    Loading,
    Loaded(Box<CwDashboardMetricsData>),
    Error(String),
}

/// The `GetMetricData` query id for one metric line.
///
/// Ids must be globally unique within a request but an author's `id` is only
/// unique within its own widget (every widget starts over at `m1`), so each is
/// namespaced by widget index. Lines with no author id never take part in
/// metric math and get a positional id.
pub fn dashboard_query_id(widget_idx: usize, metric_idx: usize, author_id: Option<&str>) -> String {
    match author_id {
        Some(id) if !id.is_empty() => format!("w{}_{}", widget_idx, id),
        _ => format!("w{}m{}", widget_idx, metric_idx),
    }
}

/// Rewrite an expression's references to its widget-local metric ids into the
/// namespaced request ids. Identifiers inside single-quoted strings are left
/// alone — a `SEARCH('{AWS/Lambda,FunctionName} MetricName="Errors"', 'Sum')`
/// argument is data, not a reference, and rewriting inside it would corrupt the
/// search.
fn rewrite_expression(expr: &str, ids: &HashMap<String, String>) -> String {
    let mut out = String::with_capacity(expr.len());
    let mut word = String::new();
    let mut in_quote = false;

    let flush = |word: &mut String, out: &mut String| {
        if !word.is_empty() {
            match ids.get(word.as_str()) {
                Some(mapped) => out.push_str(mapped),
                None => out.push_str(word),
            }
            word.clear();
        }
    };

    for ch in expr.chars() {
        if in_quote {
            out.push(ch);
            if ch == '\'' {
                in_quote = false;
            }
            continue;
        }
        if ch == '\'' {
            flush(&mut word, &mut out);
            in_quote = true;
            out.push(ch);
        } else if ch.is_alphanumeric() || ch == '_' {
            word.push(ch);
        } else {
            flush(&mut word, &mut out);
            out.push(ch);
        }
    }
    flush(&mut word, &mut out);
    out
}

/// Fetch a dashboard's body, every series it charts, and the state of every
/// alarm it references.
///
/// Re-fetches the body rather than reading `lazy.cw_dashboards` so the pane has
/// no ordering dependency on the detail section having been opened first —
/// `GetDashboard` is a cheap control-plane read and `m` is an explicit press.
///
/// Widgets can pin their own `region`, so queries are grouped by region and each
/// group runs against its own client. A region that fails is recorded and the
/// rest still render — one unreachable region must not blank the dashboard.
pub async fn fetch_dashboard_metrics(
    config: aws_config::SdkConfig,
    name: String,
    current_region: String,
    time_range: MetricsTimeRange,
    honor_body_range: bool,
) -> Result<CwDashboardMetricsData> {
    let home = crate::aws::client::cloudwatch_client_for(&config, &current_region);
    let body = fetch_dashboard(home, name).await?;
    let widgets = body.widgets;

    // The author's own window wins on the first look — but only there. Once the
    // user has stepped the range with `[`/`]` the caller clears the flag, so a
    // refresh can't yank them back to the body's default. Resolving it here
    // rather than in the caller keeps it to one round trip: the range isn't
    // known until the body has been read.
    let time_range = match (honor_body_range, body.settings.range) {
        (true, Some(r)) => r,
        _ => time_range,
    };

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let start = now - time_range.duration_secs();

    let (by_region, capped) = build_dashboard_queries(
        &widgets,
        &current_region,
        time_range.period_secs(),
        body.settings.period_inherit,
    );

    let mut series: HashMap<String, Vec<CwSeries>> = HashMap::new();
    let mut skipped_regions: Vec<String> = Vec::new();
    let mut math_error: Option<String> = None;

    for (region, queries) in by_region {
        let cw = crate::aws::client::cloudwatch_client_for(&config, &region);
        match run_metric_data(&cw, &queries, start, now).await {
            Ok(results) => merge_series(&mut series, results),
            Err(first) => {
                // A single bad expression fails the whole request (a reference
                // to an id we skipped, an unsupported function). Retry without
                // the math so the plain metrics still draw, and say so.
                let plain: Vec<aws_sdk_cloudwatch::types::MetricDataQuery> = queries
                    .iter()
                    .filter(|q| q.expression().is_none())
                    .cloned()
                    .collect();
                let retried = if plain.len() < queries.len() && !plain.is_empty() {
                    run_metric_data(&cw, &plain, start, now).await
                } else {
                    Err(first.clone())
                };
                match retried {
                    Ok(results) => {
                        merge_series(&mut series, results);
                        math_error.get_or_insert(first);
                    }
                    Err(e) => {
                        skipped_regions.push(format!("{} ({})", region, e));
                    }
                }
            }
        }
    }

    let alarms = fetch_dashboard_alarms(&config, &current_region, &widgets).await;

    Ok(CwDashboardMetricsData {
        time_range,
        x_max: time_range.duration_secs() as f64,
        widgets,
        series,
        alarms,
        skipped_regions,
        math_error,
        capped,
    })
}

/// Substitute CloudWatch's **dynamic label** tokens — `${PROP('Dim.Name')}`,
/// `${AVG}`, `${MAX}` and friends — that a dashboard author writes into a
/// series label.
///
/// `GetMetricData` normally resolves these server-side and returns the finished
/// text, so this is a fallback for labels that reach us untouched. Left alone
/// they render literally, and a legend reading `${PROP('Dim.AvailabilityZone')}`
/// four times over tells you nothing about which line is which. A token we
/// can't resolve is dropped rather than shown raw.
pub fn resolve_dynamic_label(
    label: &str,
    m: &CwWidgetMetric,
    w: &CwDashboardWidget,
    points: &[(f64, f64)],
) -> String {
    if !label.contains("${") {
        return label.to_string();
    }

    let mut out = String::with_capacity(label.len());
    let mut rest = label;
    while let Some(open) = rest.find("${") {
        out.push_str(&rest[..open]);
        let after = &rest[open + 2..];
        match after.find('}') {
            Some(close) => {
                out.push_str(&resolve_label_token(after[..close].trim(), m, w, points));
                rest = &after[close + 1..];
            }
            None => {
                // Unterminated — emit the remainder verbatim rather than eat it.
                out.push_str(&rest[open..]);
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    // Dropping a token usually leaves a double space or a dangling edge.
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn resolve_label_token(
    token: &str,
    m: &CwWidgetMetric,
    w: &CwDashboardWidget,
    points: &[(f64, f64)],
) -> String {
    // ${PROP('Dim.AvailabilityZone')} / ${PROP('MetricName')} / …
    if let Some(inner) = token
        .strip_prefix("PROP(")
        .and_then(|s| s.strip_suffix(')'))
    {
        let key = inner.trim().trim_matches('\'').trim_matches('"');
        if let Some(dim) = key.strip_prefix("Dim.") {
            return m
                .dimensions
                .iter()
                .find(|(n, _)| n == dim)
                .map(|(_, v)| v.clone())
                .unwrap_or_default();
        }
        return match key {
            "MetricName" => m.metric_name.clone(),
            "Namespace" => m.namespace.clone(),
            "Stat" => m
                .stat
                .clone()
                .or_else(|| w.stat.clone())
                .unwrap_or_else(|| "Average".to_string()),
            "Period" => m.period.or(w.period).map(|p| p.to_string()).unwrap_or_default(),
            "Region" => m
                .region
                .clone()
                .or_else(|| w.region.clone())
                .unwrap_or_default(),
            // AccountId / AccountLabel and anything newer: nothing local to
            // substitute, so drop it.
            _ => String::new(),
        };
    }

    // Statistic tokens, computed off the series we were handed. The console
    // accepts formatting arguments after a comma (`${MAX, right}`) — the
    // keyword is all that carries meaning here.
    let ys = || points.iter().map(|(_, y)| *y);
    let value = match token.split(',').next().unwrap_or(token).trim() {
        "AVG" if !points.is_empty() => Some(ys().sum::<f64>() / points.len() as f64),
        "SUM" => Some(ys().sum::<f64>()),
        "MIN" => ys().fold(None, |a: Option<f64>, y| Some(a.map_or(y, |a| a.min(y)))),
        "MAX" => ys().fold(None, |a: Option<f64>, y| Some(a.map_or(y, |a| a.max(y)))),
        "FIRST" => points.first().map(|(_, y)| *y),
        "LAST" => points.last().map(|(_, y)| *y),
        _ => None,
    };
    value.map(fmt_label_number).unwrap_or_default()
}

fn fmt_label_number(v: f64) -> String {
    if v == 0.0 {
        "0".to_string()
    } else if v.abs() >= 1_000_000.0 {
        format!("{:.1}M", v / 1_000_000.0)
    } else if v.abs() >= 1_000.0 {
        format!("{:.1}k", v / 1_000.0)
    } else if v.abs() >= 10.0 {
        format!("{:.0}", v)
    } else {
        format!("{:.2}", v)
    }
}

/// Turn a dashboard's widgets into `GetMetricData` queries, grouped by the
/// region each is pinned to. Returns the groups and whether the
/// `MAX_DASHBOARD_QUERIES` cap truncated the walk.
///
/// Pure, so the query shape is testable — it is the part CloudWatch validates
/// strictly and rejects the whole request over.
#[allow(clippy::type_complexity)]
fn build_dashboard_queries(
    widgets: &[CwDashboardWidget],
    current_region: &str,
    default_period: i32,
    period_inherit: bool,
) -> (
    HashMap<String, Vec<aws_sdk_cloudwatch::types::MetricDataQuery>>,
    bool,
) {
    use aws_sdk_cloudwatch::types::{Dimension, Metric, MetricDataQuery, MetricStat};

    // region → queries. Every metric widget contributes, whatever its view: a
    // gauge, a big number and a bar chart all need the same series, they just
    // draw it differently.
    let mut by_region: HashMap<String, Vec<MetricDataQuery>> = HashMap::new();
    let mut total = 0usize;
    let mut capped = false;

    'widgets: for (wi, w) in widgets.iter().enumerate() {
        if w.kind != "metric" {
            continue;
        }

        // Author id → request id for this widget, resolved before any expression
        // is rewritten (an expression may reference a line defined below it).
        let ids: HashMap<String, String> = w
            .metrics
            .iter()
            .enumerate()
            .filter_map(|(mi, m)| {
                m.id.as_ref()
                    .map(|id| (id.clone(), dashboard_query_id(wi, mi, Some(id))))
            })
            .collect();

        for (mi, m) in w.metrics.iter().enumerate() {
            if total >= MAX_DASHBOARD_QUERIES {
                capped = true;
                break 'widgets;
            }
            let region = m
                .region
                .clone()
                .or_else(|| w.region.clone())
                .unwrap_or_else(|| current_region.to_string());
            let query_id = dashboard_query_id(wi, mi, m.id.as_deref());
            // A widget's own period can be finer than the range supports (a 1m
            // widget over 7d), which CloudWatch rejects outright. Under
            // `periodOverride: inherit` the author asked for widget periods to
            // be ignored entirely in favour of the dashboard's time range.
            let period = if period_inherit {
                default_period
            } else {
                m.period.or(w.period).unwrap_or(default_period).max(default_period)
            };

            let query = if let Some(expr) = &m.expression {
                let mut b = MetricDataQuery::builder()
                    .id(&query_id)
                    .expression(rewrite_expression(expr, &ids))
                    // **Required** for a SEARCH() expression — without it the
                    // whole request fails "Period is required when using
                    // SEARCH", taking every other widget's data with it. Plain
                    // math expressions ignore it, so it is set unconditionally.
                    .period(period)
                    .return_data(m.visible);
                if !m.label.is_empty() {
                    b = b.label(&m.label);
                }
                b.build()
            } else if m.metric_name.is_empty() {
                continue;
            } else {
                let dims: Vec<Dimension> = m
                    .dimensions
                    .iter()
                    .map(|(n, v)| Dimension::builder().name(n).value(v).build())
                    .collect();
                let stat = m
                    .stat
                    .clone()
                    .or_else(|| w.stat.clone())
                    .unwrap_or_else(|| "Average".to_string());

                MetricDataQuery::builder()
                    .id(&query_id)
                    .label(&m.label)
                    .metric_stat(
                        MetricStat::builder()
                            .metric(
                                Metric::builder()
                                    .namespace(&m.namespace)
                                    .metric_name(&m.metric_name)
                                    .set_dimensions(if dims.is_empty() { None } else { Some(dims) })
                                    .build(),
                            )
                            .period(period)
                            .stat(stat)
                            .build(),
                    )
                    // Expressions are resolved server-side, so a hidden input
                    // line needs no data returned — ReturnData just mirrors the
                    // author's intent and saves transferring what nobody draws.
                    .return_data(m.visible)
                    .build()
            };

            by_region.entry(region).or_default().push(query);
            total += 1;
        }
    }

    (by_region, capped)
}

/// One returned result row: (query id, series label, points). The label is what
/// separates the several series a `SEARCH()` returns under one id.
type MetricResultRow = (String, String, Vec<(f64, f64)>);

/// One paginated `GetMetricData` call. Split out so the metric-math fallback can
/// re-run the same request with the expression queries dropped.
async fn run_metric_data(
    cw: &CwClient,
    queries: &[aws_sdk_cloudwatch::types::MetricDataQuery],
    start: i64,
    now: i64,
) -> std::result::Result<Vec<MetricResultRow>, String> {
    let mut out: Vec<MetricResultRow> = Vec::new();
    let mut token: Option<String> = None;

    loop {
        let mut req = cw
            .get_metric_data()
            .set_metric_data_queries(Some(queries.to_vec()))
            .start_time(aws_sdk_cloudwatch::primitives::DateTime::from_secs(start))
            .end_time(aws_sdk_cloudwatch::primitives::DateTime::from_secs(now));
        if let Some(t) = &token {
            req = req.next_token(t);
        }
        let resp = req.send().await.map_err(|e| crate::error::sdk_error_message(&e))?;

        for r in resp.metric_data_results() {
            let id = r.id().unwrap_or_default().to_string();
            let label = r.label().unwrap_or_default().to_string();
            let points: Vec<(f64, f64)> = r
                .timestamps()
                .iter()
                .zip(r.values().iter())
                .map(|(t, v)| (t.secs() as f64 - start as f64, *v))
                .collect();
            out.push((id, label, points));
        }

        match crate::aws::pagination::next_page_token(resp.next_token(), &token) {
            Some(t) => token = Some(t),
            None => break,
        }
    }

    Ok(out)
}

/// Fold one call's results into the id→series map. Paging splits a single series
/// across responses (same id *and* label), while a `SEARCH()` expression returns
/// many distinct series under one id — so results merge by (id, label), and each
/// series is sorted once at the end because `GetMetricData` returns newest-first.
fn merge_series(
    series: &mut HashMap<String, Vec<CwSeries>>,
    results: Vec<MetricResultRow>,
) {
    for (id, label, points) in results {
        let bucket = series.entry(id).or_default();
        match bucket.iter_mut().find(|s| s.label == label) {
            Some(existing) => existing.points.extend(points),
            None => bucket.push(CwSeries { label, points }),
        }
    }
    for bucket in series.values_mut() {
        for s in bucket.iter_mut() {
            s.points
                .sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        }
    }
}

/// `DescribeAlarms` limit on names per call.
const ALARM_NAMES_PER_CALL: usize = 100;

/// Resolve every alarm an alarm widget references to its current state.
///
/// Alarm ARNs carry their region, so they're grouped and each group asks its own
/// region — an alarm widget pointing at another region is common on a
/// cross-region overview dashboard. Entirely best-effort: a widget whose alarms
/// don't resolve renders the names without states rather than failing the pane.
async fn fetch_dashboard_alarms(
    config: &aws_config::SdkConfig,
    current_region: &str,
    widgets: &[CwDashboardWidget],
) -> HashMap<String, CwAlarmChip> {
    let mut by_region: HashMap<String, Vec<String>> = HashMap::new();
    for w in widgets.iter().filter(|w| w.kind == "alarm") {
        for arn in &w.alarm_arns {
            // arn:aws:cloudwatch:<region>:<acct>:alarm:<name> — the name itself
            // may contain colons, so split on the marker, not from the right.
            let (head, alarm_name) = match arn.split_once(":alarm:") {
                Some(parts) => parts,
                None => continue,
            };
            let region = head
                .split(':')
                .nth(3)
                .filter(|r| !r.is_empty())
                .unwrap_or(current_region)
                .to_string();
            let names = by_region.entry(region).or_default();
            if !names.iter().any(|n| n == alarm_name) {
                names.push(alarm_name.to_string());
            }
        }
    }

    let mut out: HashMap<String, CwAlarmChip> = HashMap::new();
    for (region, names) in by_region {
        let cw = crate::aws::client::cloudwatch_client_for(config, &region);
        for chunk in names.chunks(ALARM_NAMES_PER_CALL) {
            let resp = match cw
                .describe_alarms()
                .set_alarm_names(Some(chunk.to_vec()))
                .send()
                .await
            {
                Ok(r) => r,
                Err(_) => continue, // best-effort: names render stateless
            };
            for a in resp.metric_alarms() {
                let name = a.alarm_name().unwrap_or_default().to_string();
                out.insert(
                    name.clone(),
                    CwAlarmChip {
                        name,
                        state: a.state_value().map(|s| s.as_str().to_string()).unwrap_or_default(),
                        reason: a.state_reason().unwrap_or_default().to_string(),
                    },
                );
            }
            // A dashboard can point at composite alarms too, and they come back
            // in their own list rather than alongside the metric alarms.
            for a in resp.composite_alarms() {
                let name = a.alarm_name().unwrap_or_default().to_string();
                out.insert(
                    name.clone(),
                    CwAlarmChip {
                        name,
                        state: a.state_value().map(|s| s.as_str().to_string()).unwrap_or_default(),
                        reason: a.state_reason().unwrap_or_default().to_string(),
                    },
                );
            }
        }
    }
    out
}

// ── CwMetric (metric explorer) ────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct CwMetric {
    pub id: String,
    pub namespace: String,
    pub metric_name: String,
    pub dimensions: Vec<(String, String)>,
    pub tags: HashMap<String, String>,
}

impl CwMetric {
    pub fn from_sdk(m: &aws_sdk_cloudwatch::types::Metric) -> Self {
        let namespace = m.namespace().unwrap_or("").to_string();
        let metric_name = m.metric_name().unwrap_or("").to_string();
        let dimensions: Vec<(String, String)> = m
            .dimensions()
            .iter()
            .map(|d| {
                (
                    d.name().unwrap_or("").to_string(),
                    d.value().unwrap_or("").to_string(),
                )
            })
            .collect();
        let dims_label = dimensions
            .iter()
            .map(|(n, v)| format!("{}={}", n, v))
            .collect::<Vec<_>>()
            .join(", ");
        // Unique, readable id: namespace/metric + dims (so two metrics that share
        // a name but differ by dimensions remain distinct list rows).
        let id = if dims_label.is_empty() {
            format!("{}/{}", namespace, metric_name)
        } else {
            format!("{}/{} [{}]", namespace, metric_name, dims_label)
        };
        Self {
            id,
            namespace,
            metric_name,
            dimensions,
            tags: HashMap::new(),
        }
    }

    pub fn dims_label(&self) -> String {
        self.dimensions
            .iter()
            .map(|(n, v)| format!("{}={}", n, v))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

impl Resource for CwMetric {
    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        &self.metric_name
    }

    fn resource_type(&self) -> &str {
        "CW Metric"
    }

    fn state(&self) -> ResourceState {
        ResourceState::stateless()
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!("{} {} {}", self.namespace, self.metric_name, self.dims_label())
    }

    fn details(&self) -> Vec<(String, String)> {
        let mut rows = vec![
            ("Namespace".to_string(), self.namespace.clone()),
            ("Metric".to_string(), self.metric_name.clone()),
        ];
        for (n, v) in &self.dimensions {
            rows.push((format!("Dim {}", n), v.clone()));
        }
        rows.push(("".to_string(), "press m to chart".to_string()));
        rows
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/cloudwatch/home?region={}#metricsV2:graph=~()",
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

/// List metrics across the account for the metric explorer (the Metrics
/// sub-tab). Paginated and capped — a busy account can have tens of thousands;
/// `capped` is surfaced so the UI can say the list is partial.
pub async fn fetch_metrics(client: CwClient) -> Result<(Vec<CwMetric>, bool)> {
    const CAP: usize = 3000;
    let mut out: Vec<CwMetric> = Vec::new();
    let mut capped = false;
    let mut paginator = client.list_metrics().into_paginator().send();
    while let Some(page) = paginator.next().await {
        let page = page.map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
        for m in page.metrics() {
            out.push(CwMetric::from_sdk(m));
            if out.len() >= CAP {
                capped = true;
                break;
            }
        }
        if capped {
            break;
        }
    }
    Ok((out, capped))
}

// ── CwLogGroup ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct CwLogGroup {
    pub log_group_name: String,
    pub arn: String,
    pub retention_in_days: Option<i32>,
    pub stored_bytes: i64,
    pub creation_time_ms: Option<i64>,
    pub kms_key_id: Option<String>,
    pub log_group_class: Option<String>,
    pub metric_filter_count: Option<i32>,
    pub tags: HashMap<String, String>,
}

impl CwLogGroup {
    pub fn from_sdk(g: &aws_sdk_cloudwatchlogs::types::LogGroup) -> Self {
        Self {
            log_group_name: g.log_group_name().unwrap_or("").to_string(),
            arn: g.arn().unwrap_or("").to_string(),
            retention_in_days: g.retention_in_days(),
            stored_bytes: g.stored_bytes().unwrap_or(0),
            creation_time_ms: g.creation_time(),
            kms_key_id: g.kms_key_id().map(|s| s.to_string()),
            log_group_class: g.log_group_class().map(|c| c.as_str().to_string()),
            metric_filter_count: g.metric_filter_count(),
            tags: HashMap::new(),
        }
    }

    pub fn retention_display(&self) -> String {
        match self.retention_in_days {
            Some(d) => format!("{d} days"),
            None => "Never expire".to_string(),
        }
    }

    pub fn stored_bytes_display(&self) -> String {
        let b = self.stored_bytes;
        if b < 1024 {
            format!("{b} B")
        } else if b < 1024 * 1024 {
            format!("{:.1} KB", b as f64 / 1024.0)
        } else if b < 1024 * 1024 * 1024 {
            format!("{:.1} MB", b as f64 / (1024.0 * 1024.0))
        } else {
            format!("{:.2} GB", b as f64 / (1024.0 * 1024.0 * 1024.0))
        }
    }
}

crate::sections! {
    pub enum CwLogGroupDetailSection,
    pub static CW_LOG_GROUP_SECTIONS = [
        Details "Details",
        Streams "Streams" => crate::app::App::trigger_cw_log_streams_load,
        Filters "Filters" => crate::app::App::trigger_cw_log_group_filters_load,
    ]
}

impl Resource for CwLogGroup {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&CW_LOG_GROUP_SECTIONS)
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws logs tail {} --since 15m",
            crate::aws::resource::shell_quote(&self.log_group_name)
        ))
    }

    fn id(&self) -> &str {
        &self.arn
    }

    fn name(&self) -> &str {
        &self.log_group_name
    }

    fn resource_type(&self) -> &str {
        "CW Log Group"
    }

    fn state(&self) -> ResourceState {
        ResourceState::stateless()
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!("{} {}", self.log_group_name, self.arn)
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Log Group".to_string(), self.log_group_name.clone()),
            ("Retention".to_string(), self.retention_display()),
            ("Stored".to_string(), self.stored_bytes_display()),
        ]
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/cloudwatch/home?region={}#logsV2:log-groups/log-group/{}",
            region,
            region,
            urlencoding_simple(&self.log_group_name)
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── Alarm history (lazy-loaded) ───────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct CwAlarmHistoryItem {
    pub timestamp: String,
    /// Epoch seconds — the timeline lens merge-sorts and range-filters on it.
    pub ts_secs: i64,
    pub summary: String,
    pub item_type: String,
}

pub async fn fetch_alarm_history(
    client: CwClient,
    alarm_name: String,
) -> Result<Vec<CwAlarmHistoryItem>> {
    let resp = client
        .describe_alarm_history()
        .alarm_name(&alarm_name)
        .max_records(25)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;

    let items = resp
        .alarm_history_items()
        .iter()
        .map(|h| CwAlarmHistoryItem {
            timestamp: h
                .timestamp()
                .map(|t| fmt_epoch_secs(t.secs()))
                .unwrap_or_default(),
            ts_secs: h.timestamp().map(|t| t.secs()).unwrap_or(0),
            summary: h.history_summary().unwrap_or("").to_string(),
            item_type: h
                .history_item_type()
                .map(|t| t.as_str().to_string())
                .unwrap_or_default(),
        })
        .collect();

    Ok(items)
}

// ── Alarm metric overlay (`m` action on a CwAlarm) ─────────────────────────

use crate::aws::services::ec2::MetricsTimeRange;

#[derive(Debug, Clone)]
pub struct CwAlarmMetricsData {
    pub time_range: MetricsTimeRange,
    pub points: Vec<(f64, f64)>,
    pub threshold: f64,
    pub x_max: f64,
}

#[derive(Debug, Clone)]
pub enum CwAlarmMetricsState {
    Loading,
    Loaded(CwAlarmMetricsData),
}

/// Fetch the time series for an alarm's own metric so it can be charted with
/// the threshold drawn as a reference line. Uses the alarm's namespace /
/// metric / dimensions / statistic; the period is widened to the view's
/// period on long ranges to stay under the 1440-datapoint cap. Metric-math /
/// anomaly-detection alarms (no single metric/statistic) come back empty.
#[allow(clippy::too_many_arguments)]
pub async fn fetch_alarm_metrics(
    client: CwClient,
    namespace: String,
    metric_name: String,
    dimensions: Vec<(String, String)>,
    statistic: String,
    period: i32,
    threshold: f64,
    time_range: MetricsTimeRange,
) -> Result<CwAlarmMetricsData> {
    use aws_sdk_cloudwatch::types::{Dimension, Statistic};

    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let start_secs = now_secs - time_range.duration_secs();
    let period = period.max(time_range.period_secs());

    let start_dt = aws_sdk_cloudwatch::primitives::DateTime::from_secs(start_secs);
    let end_dt = aws_sdk_cloudwatch::primitives::DateTime::from_secs(now_secs);

    let stat = match statistic.as_str() {
        "Sum" => Statistic::Sum,
        "Minimum" => Statistic::Minimum,
        "Maximum" => Statistic::Maximum,
        "SampleCount" => Statistic::SampleCount,
        _ => Statistic::Average,
    };

    let dims: Vec<Dimension> = dimensions
        .iter()
        .map(|(n, v)| Dimension::builder().name(n).value(v).build())
        .collect();

    let resp = client
        .get_metric_statistics()
        .namespace(&namespace)
        .metric_name(&metric_name)
        .set_dimensions(if dims.is_empty() { None } else { Some(dims) })
        .start_time(start_dt)
        .end_time(end_dt)
        .period(period)
        .set_statistics(Some(vec![stat]))
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;

    let mut points: Vec<(f64, f64)> = resp
        .datapoints()
        .iter()
        .filter_map(|dp| {
            let t = (dp.timestamp()?.secs() - start_secs) as f64;
            let v = match statistic.as_str() {
                "Sum" => dp.sum(),
                "Minimum" => dp.minimum(),
                "Maximum" => dp.maximum(),
                "SampleCount" => dp.sample_count(),
                _ => dp.average(),
            }?;
            Some((t, v))
        })
        .collect();
    points.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

    Ok(CwAlarmMetricsData {
        time_range,
        points,
        threshold,
        x_max: time_range.duration_secs() as f64,
    })
}

// ── Log group filters (lazy Filters section of the CwLogGroup split pane) ──

#[derive(Debug, Clone)]
pub struct CwLogGroupFilters {
    /// (filter name, destination ARN)
    pub subscription_filters: Vec<(String, String)>,
    /// (filter name, filter pattern)
    pub metric_filters: Vec<(String, String)>,
}

/// Fetch the subscription and metric filters attached to a log group — "what
/// ships these logs out / what alarms derive from them". Each call is
/// independently error-tolerant so one missing permission doesn't blank both.
pub async fn fetch_log_group_filters(
    client: LogsClient,
    log_group_name: String,
) -> Result<CwLogGroupFilters> {
    let (sub_r, met_r) = tokio::join!(
        client
            .describe_subscription_filters()
            .log_group_name(&log_group_name)
            .send(),
        client
            .describe_metric_filters()
            .log_group_name(&log_group_name)
            .send(),
    );

    let subscription_filters = match sub_r {
        Ok(r) => r
            .subscription_filters()
            .iter()
            .map(|f| {
                (
                    f.filter_name().unwrap_or("").to_string(),
                    f.destination_arn().unwrap_or("").to_string(),
                )
            })
            .collect(),
        Err(_) => vec![],
    };

    let metric_filters = match met_r {
        Ok(r) => r
            .metric_filters()
            .iter()
            .map(|f| {
                (
                    f.filter_name().unwrap_or("").to_string(),
                    f.filter_pattern().unwrap_or("").to_string(),
                )
            })
            .collect(),
        Err(_) => vec![],
    };

    Ok(CwLogGroupFilters {
        subscription_filters,
        metric_filters,
    })
}

// ── Log streams (lazy-loaded) ─────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct CwLogStream {
    pub name: String,
    pub last_event_ms: Option<i64>,
}

/// The most-recently-active streams in a log group (the console's default
/// log-group view). Ordered by last-event time, newest first, capped at 50.
pub async fn fetch_log_streams(
    client: LogsClient,
    log_group_name: String,
) -> Result<Vec<CwLogStream>> {
    let resp = client
        .describe_log_streams()
        .log_group_name(&log_group_name)
        .order_by(aws_sdk_cloudwatchlogs::types::OrderBy::LastEventTime)
        .descending(true)
        .limit(50)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;

    let streams = resp
        .log_streams()
        .iter()
        .map(|s| CwLogStream {
            name: s.log_stream_name().unwrap_or("").to_string(),
            last_event_ms: s.last_event_timestamp(),
        })
        .collect();

    Ok(streams)
}

pub fn fmt_epoch_secs(secs: i64) -> String {
    let days = secs / 86400;
    let time_rem = secs % 86400;
    let h = time_rem / 3600;
    let m = (time_rem % 3600) / 60;
    let (y, mo, d) = epoch_days_to_ymd(days);
    format!("{:04}-{:02}-{:02} {:02}:{:02}", y, mo, d, h, m)
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
        31, 30, 31, 30, 31, 31, 30, 31, 30, 31,
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

fn urlencoding_simple(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' | '/' => {
                c.to_string()
            }
            other => format!("%{:02X}", other as u32),
        })
        .collect()
}

// ── Live log tail (polling-based) ─────────────────────────────────────────────

/// One rendered log line for the live tail view.
#[derive(Debug, Clone)]
pub struct LogTailLine {
    pub ts_ms: i64,
    pub ts: String,      // HH:MM:SS (local-agnostic, UTC)
    pub stream: String,  // short stream name (last '/' segment), or ""
    pub message: String,
    /// A wrapped continuation of a multi-line event (stack trace, pretty JSON).
    /// Carries its parent's `ts`/`stream` so the `/` filter still matches on
    /// them, but the renderer pads instead of repeating them down the column.
    pub continuation: bool,
}

/// Make one physical line of a raw CloudWatch message safe to write into a
/// ratatui cell.
///
/// Log payloads are arbitrary application output: ANSI colour codes, tabs,
/// carriage returns, NULs. Ratatui writes each `char` straight into the buffer
/// and counts it as 0- or 1-wide, so its truncation-to-pane-width doesn't stop
/// the *terminal* from acting on it — a `\r` or an escape sequence moves the
/// real cursor and the rest of the row paints from the terminal's left edge,
/// over the list pane. Strip the escapes, expand tabs, drop the rest.
fn sanitize_log_line(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => match chars.peek() {
                // CSI: params/intermediates then a final byte in @..~
                Some('[') => {
                    chars.next();
                    for c in chars.by_ref() {
                        if ('\u{40}'..='\u{7e}').contains(&c) {
                            break;
                        }
                    }
                }
                // OSC: runs until BEL or the ESC of a string terminator
                Some(']') => {
                    chars.next();
                    while let Some(c) = chars.next() {
                        if c == '\u{7}' {
                            break;
                        }
                        if c == '\u{1b}' {
                            chars.next();
                            break;
                        }
                    }
                }
                // Two-character escape (or a trailing bare ESC)
                _ => {
                    chars.next();
                }
            },
            '\t' => out.push_str("    "),
            // \r, \x08, NUL, and the C1 range — all cursor-movers or invisible
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out.trim_end().to_string()
}

/// Split a raw event into terminal-safe display lines. A multi-line event
/// (stack trace, pretty-printed JSON) becomes one line per row, the way the
/// console shows it, so it scrolls and filters per line instead of being one
/// row with embedded newlines the terminal would act on.
fn log_display_lines(raw: &str) -> Vec<String> {
    let mut out: Vec<String> = raw.split('\n').map(sanitize_log_line).collect();
    // The old whole-message `trim_end()`, now per physical line.
    while out.len() > 1 && out.last().is_some_and(|l| l.is_empty()) {
        out.pop();
    }
    out
}

/// Expand one event into its display lines, sharing `ts`/`stream` so the
/// in-buffer filter still matches a continuation by its parent's stream.
fn push_log_lines(out: &mut Vec<LogTailLine>, ts_ms: i64, stream: &str, raw: &str) {
    let ts = fmt_hms(ts_ms);
    for (n, message) in log_display_lines(raw).into_iter().enumerate() {
        out.push(LogTailLine {
            ts_ms,
            ts: ts.clone(),
            stream: stream.to_string(),
            message,
            continuation: n > 0,
        });
    }
}

/// Format an epoch-millis timestamp as `HH:MM:SS` (UTC) for the tail.
fn fmt_hms(ts_ms: i64) -> String {
    let secs = ts_ms / 1000;
    let rem = secs.rem_euclid(86400);
    format!("{:02}:{:02}:{:02}", rem / 3600, (rem % 3600) / 60, rem % 60)
}

/// One poll of a log group (optionally restricted to specific streams) for
/// events at or after `cursor_ms`, via `filter_log_events`. Returns the new
/// lines (chronological) and the cursor to use next time: `max_ts + 1` when
/// events were seen, else the unchanged `cursor_ms` (so an idle tail doesn't
/// drift its start time forward). Paginated and capped at 5 pages/poll.
pub async fn poll_log_tail(
    logs: LogsClient,
    group: String,
    streams: Vec<String>,
    cursor_ms: i64,
) -> Result<(Vec<LogTailLine>, i64)> {
    let mut lines: Vec<LogTailLine> = Vec::new();
    let mut max_ts = cursor_ms;
    let mut token: Option<String> = None;
    let mut pages = 0;

    loop {
        let mut req = logs
            .filter_log_events()
            .log_group_name(&group)
            .start_time(cursor_ms)
            .limit(1000);
        if !streams.is_empty() {
            req = req.set_log_stream_names(Some(streams.clone()));
        }
        if let Some(t) = &token {
            req = req.next_token(t);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;

        for e in resp.events() {
            let ts_ms = e.timestamp().unwrap_or(0);
            if ts_ms > max_ts {
                max_ts = ts_ms;
            }
            let stream = e
                .log_stream_name()
                .map(|s| s.rsplit('/').next().unwrap_or(s).to_string())
                .unwrap_or_default();
            push_log_lines(&mut lines, ts_ms, &stream, e.message().unwrap_or_default());
        }

        token = crate::aws::pagination::next_page_token(resp.next_token(), &token);
        pages += 1;
        if token.is_none() || pages >= 5 {
            break;
        }
    }

    lines.sort_by_key(|l| l.ts_ms);
    let next_cursor = if lines.is_empty() { cursor_ms } else { max_ts + 1 };
    Ok((lines, next_cursor))
}

// ── Quick log search ("Search log group") ─────────────────────────────────────

/// Time-range presets for a one-shot log search (the console's relative-range
/// dropdown). `Custom` is intentionally omitted in v1 — presets cover the cases.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LogSearchRange {
    M15,
    #[default]
    H1,
    H3,
    H12,
    D1,
    D3,
}

impl LogSearchRange {
    pub fn label(self) -> &'static str {
        match self {
            LogSearchRange::M15 => "15m",
            LogSearchRange::H1 => "1h",
            LogSearchRange::H3 => "3h",
            LogSearchRange::H12 => "12h",
            LogSearchRange::D1 => "1d",
            LogSearchRange::D3 => "3d",
        }
    }

    pub fn lookback_ms(self) -> i64 {
        let mins: i64 = match self {
            LogSearchRange::M15 => 15,
            LogSearchRange::H1 => 60,
            LogSearchRange::H3 => 3 * 60,
            LogSearchRange::H12 => 12 * 60,
            LogSearchRange::D1 => 24 * 60,
            LogSearchRange::D3 => 3 * 24 * 60,
        };
        mins * 60 * 1000
    }

    /// Cycle to the next preset (wraps M15→H1→…→D3→M15).
    pub fn cycle(self) -> Self {
        match self {
            LogSearchRange::M15 => LogSearchRange::H1,
            LogSearchRange::H1 => LogSearchRange::H3,
            LogSearchRange::H3 => LogSearchRange::H12,
            LogSearchRange::H12 => LogSearchRange::D1,
            LogSearchRange::D1 => LogSearchRange::D3,
            LogSearchRange::D3 => LogSearchRange::M15,
        }
    }

    /// Widen the window one step, saturating at the largest preset (`]`).
    pub fn wider(self) -> Self {
        match self {
            LogSearchRange::M15 => LogSearchRange::H1,
            LogSearchRange::H1 => LogSearchRange::H3,
            LogSearchRange::H3 => LogSearchRange::H12,
            LogSearchRange::H12 => LogSearchRange::D1,
            LogSearchRange::D1 | LogSearchRange::D3 => LogSearchRange::D3,
        }
    }

    /// Narrow the window one step, saturating at the smallest preset (`[`).
    pub fn narrower(self) -> Self {
        match self {
            LogSearchRange::D3 => LogSearchRange::D1,
            LogSearchRange::D1 => LogSearchRange::H12,
            LogSearchRange::H12 => LogSearchRange::H3,
            LogSearchRange::H3 => LogSearchRange::H1,
            LogSearchRange::H1 | LogSearchRange::M15 => LogSearchRange::M15,
        }
    }
}

/// Cap a single search so a chatty group can't run away. When hit, the caller
/// flags the result as truncated so the UI can say so.
const LOG_SEARCH_MAX_LINES: usize = 5_000;
const LOG_SEARCH_MAX_PAGES: usize = 10;

/// One-shot, server-side search of a log group over `[now-range, now]` with an
/// optional CloudWatch Logs **filter pattern** (empty = all events), across all
/// streams (or a specific set). Returns matching lines (chronological) and
/// whether the result was capped. `now_ms` is passed in so this is testable.
pub async fn search_log_events(
    logs: LogsClient,
    group: String,
    streams: Vec<String>,
    filter_pattern: String,
    range: LogSearchRange,
    now_ms: i64,
) -> Result<(Vec<LogTailLine>, bool)> {
    let start_ms = now_ms - range.lookback_ms();
    let mut lines: Vec<LogTailLine> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0;
    let mut capped = false;

    loop {
        let mut req = logs
            .filter_log_events()
            .log_group_name(&group)
            .start_time(start_ms)
            .end_time(now_ms)
            .limit(1000);
        if !filter_pattern.trim().is_empty() {
            req = req.filter_pattern(filter_pattern.clone());
        }
        if !streams.is_empty() {
            req = req.set_log_stream_names(Some(streams.clone()));
        }
        if let Some(t) = &token {
            req = req.next_token(t);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;

        for e in resp.events() {
            let ts_ms = e.timestamp().unwrap_or(0);
            let stream = e
                .log_stream_name()
                .map(|s| s.rsplit('/').next().unwrap_or(s).to_string())
                .unwrap_or_default();
            push_log_lines(&mut lines, ts_ms, &stream, e.message().unwrap_or_default());
        }

        if lines.len() >= LOG_SEARCH_MAX_LINES {
            lines.truncate(LOG_SEARCH_MAX_LINES);
            capped = true;
            break;
        }

        token = crate::aws::pagination::next_page_token(resp.next_token(), &token);
        pages += 1;
        if token.is_none() {
            break;
        }
        if pages >= LOG_SEARCH_MAX_PAGES {
            capped = true;
            break;
        }
    }

    lines.sort_by_key(|l| l.ts_ms);
    Ok((lines, capped))
}

// ── Log-group CloudWatch metrics (`m`) — AWS/Logs, dim LogGroupName ───────────

#[derive(Debug, Clone)]
pub struct LogGroupMetricsData {
    pub time_range: MetricsTimeRange,
    pub incoming_events: Vec<(f64, f64)>,
    pub incoming_bytes: Vec<(f64, f64)>,
    pub x_max: f64,
}

#[derive(Debug, Clone)]
pub enum LogGroupMetricsState {
    Loading,
    Loaded(LogGroupMetricsData),
}

/// Ingestion volume for one log group — "is this group receiving anything"
/// without opening a tail. Flat zero with a producer supposedly attached is
/// the misconfigured-logging signal.
pub async fn fetch_log_group_metrics(
    cw: aws_sdk_cloudwatch::Client,
    group_name: String,
    time_range: MetricsTimeRange,
) -> Result<LogGroupMetricsData> {
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
            .namespace("AWS/Logs")
            .metric_name(name)
            .dimensions(Dimension::builder().name("LogGroupName").value(&group_name).build())
            .start_time(start_dt)
            .end_time(end_dt)
            .period(period)
            .set_statistics(Some(vec![Statistic::Sum]))
            .send()
    };

    let (events, bytes) = tokio::join!(metric("IncomingLogEvents"), metric("IncomingBytes"));

    Ok(LogGroupMetricsData {
        time_range,
        incoming_events: crate::aws::services::ec2::parse_metric_datapoints(events, start),
        incoming_bytes: crate::aws::services::ec2::parse_metric_datapoints(bytes, start),
        x_max: time_range.duration_secs() as f64,
    })
}

// ── CwMetricStream (Streams sub-tab) ─────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct CwMetricStream {
    pub name: String,
    pub arn: String,
    pub firehose_arn: String,
    pub state: String,
    pub output_format: String,
    pub created_ms: Option<i64>,
    pub updated_ms: Option<i64>,
    pub tags: HashMap<String, String>,
}

impl CwMetricStream {
    pub fn from_sdk(e: &aws_sdk_cloudwatch::types::MetricStreamEntry) -> Self {
        Self {
            name: e.name().unwrap_or("").to_string(),
            arn: e.arn().unwrap_or("").to_string(),
            firehose_arn: e.firehose_arn().unwrap_or("").to_string(),
            state: e.state().unwrap_or("").to_string(),
            output_format: e
                .output_format()
                .map(|f| f.as_str().to_string())
                .unwrap_or_default(),
            created_ms: e.creation_date().map(|d| d.secs() * 1000),
            updated_ms: e.last_update_date().map(|d| d.secs() * 1000),
            tags: HashMap::new(),
        }
    }
}

crate::sections! {
    pub enum CwMetricStreamDetailSection,
    pub static CW_METRIC_STREAM_SECTIONS = [
        Details "Details",
        Filters "Filters" => crate::app::App::trigger_cw_metric_stream_load,
    ]
}

impl Resource for CwMetricStream {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&CW_METRIC_STREAM_SECTIONS)
    }

    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws cloudwatch get-metric-stream --name {}",
            crate::aws::resource::shell_quote(&self.name)
        ))
    }

    fn id(&self) -> &str {
        &self.arn
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn resource_type(&self) -> &str {
        "CW Metric Stream"
    }

    fn state(&self) -> ResourceState {
        match self.state.as_str() {
            "running" => ResourceState::Available,
            "stopped" => ResourceState::Stopped,
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
            "{} {} {} metric stream {}",
            self.name, self.arn, self.firehose_arn, self.state
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Stream".to_string(), self.name.clone()),
            ("State".to_string(), self.state.clone()),
            ("Destination".to_string(), self.firehose_arn.clone()),
        ]
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/cloudwatch/home?region={}#metric-streams:streamsList/{}",
            region,
            region,
            urlencoding_simple(&self.name)
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// GetMetricStream depth for the Filters section: namespace include/exclude
/// filters, the delivery role, and any extra-statistics configurations.
#[derive(Debug, Clone)]
pub struct CwMetricStreamDetail {
    /// Namespace → the metric names limited to (empty = every metric in it).
    pub include_filters: Vec<(String, Vec<String>)>,
    pub exclude_filters: Vec<(String, Vec<String>)>,
    pub role_arn: String,
    pub include_linked_accounts: bool,
    /// Per statistics-configuration: (comma-joined stats, "ns · metric" rows).
    pub stats_configs: Vec<(String, Vec<String>)>,
}

pub async fn fetch_metric_stream(
    client: CwClient,
    name: String,
) -> std::result::Result<CwMetricStreamDetail, String> {
    let resp = client
        .get_metric_stream()
        .name(&name)
        .send()
        .await
        .map_err(|e| crate::error::sdk_error_message(&e))?;
    let map_filters = |fs: &[aws_sdk_cloudwatch::types::MetricStreamFilter]| {
        fs.iter()
            .map(|f| {
                (
                    f.namespace().unwrap_or("").to_string(),
                    f.metric_names().iter().map(|m| m.to_string()).collect(),
                )
            })
            .collect::<Vec<(String, Vec<String>)>>()
    };
    Ok(CwMetricStreamDetail {
        include_filters: map_filters(resp.include_filters()),
        exclude_filters: map_filters(resp.exclude_filters()),
        role_arn: resp.role_arn().unwrap_or("").to_string(),
        include_linked_accounts: resp.include_linked_accounts_metrics().unwrap_or(false),
        stats_configs: resp
            .statistics_configurations()
            .iter()
            .map(|c| {
                (
                    c.additional_statistics().join(", "),
                    c.include_metrics()
                        .iter()
                        .map(|m| {
                            format!(
                                "{} · {}",
                                m.namespace().unwrap_or(""),
                                m.metric_name().unwrap_or("")
                            )
                        })
                        .collect(),
                )
            })
            .collect(),
    })
}

// ── CwAnomalyDetector (Insights sub-tab) ─────────────────────────────────────

/// A metric anomaly detector — the trained band alarms with
/// `ANOMALY_DETECTION_BAND` evaluate against. Flat detail pane: everything the
/// API returns fits, so no lazy sections.
#[derive(Debug, Clone)]
pub struct CwAnomalyDetector {
    pub label: String,
    pub detector_type: String,
    pub namespace: String,
    pub metric_name: String,
    pub stat: String,
    pub dimensions: Vec<(String, String)>,
    pub expression: String,
    pub state: String,
    pub periodic_spikes: Option<bool>,
    pub source_account: String,
    pub synth_id: String,
    pub tags: HashMap<String, String>,
}

impl CwAnomalyDetector {
    pub fn from_sdk(a: &aws_sdk_cloudwatch::types::AnomalyDetector) -> Self {
        let mut namespace = String::new();
        let mut metric_name = String::new();
        let mut stat = String::new();
        let mut dimensions: Vec<(String, String)> = Vec::new();
        let mut expression = String::new();
        let mut source_account = String::new();
        let detector_type;

        if let Some(m) = a.metric_math_anomaly_detector() {
            detector_type = "Metric math".to_string();
            expression = m
                .metric_data_queries()
                .iter()
                .filter_map(|q| q.expression().map(|e| e.to_string()))
                .collect::<Vec<_>>()
                .join("; ");
        } else if let Some(s) = a.single_metric_anomaly_detector() {
            detector_type = "Single metric".to_string();
            namespace = s.namespace().unwrap_or("").to_string();
            metric_name = s.metric_name().unwrap_or("").to_string();
            stat = s.stat().unwrap_or("").to_string();
            source_account = s.account_id().unwrap_or("").to_string();
            dimensions = s
                .dimensions()
                .iter()
                .map(|d| {
                    (
                        d.name().unwrap_or("").to_string(),
                        d.value().unwrap_or("").to_string(),
                    )
                })
                .collect();
        } else {
            // Detectors created through older API versions come back on the
            // deprecated top-level fields, with both typed structs empty.
            detector_type = "Single metric".to_string();
            #[allow(deprecated)]
            {
                namespace = a.namespace().unwrap_or("").to_string();
                metric_name = a.metric_name().unwrap_or("").to_string();
                stat = a.stat().unwrap_or("").to_string();
                dimensions = a
                    .dimensions()
                    .iter()
                    .map(|d| {
                        (
                            d.name().unwrap_or("").to_string(),
                            d.value().unwrap_or("").to_string(),
                        )
                    })
                    .collect();
            }
        }

        let label = if !expression.is_empty() {
            expression.clone()
        } else {
            format!("{}/{} ({})", namespace, metric_name, stat)
        };
        let dim_part = dimensions
            .iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect::<Vec<_>>()
            .join(",");
        let synth_id = format!("anomaly:{}:{}", label, dim_part);

        Self {
            label,
            detector_type,
            namespace,
            metric_name,
            stat,
            dimensions,
            expression,
            state: a
                .state_value()
                .map(|s| s.as_str().to_string())
                .unwrap_or_default(),
            periodic_spikes: a.metric_characteristics().and_then(|c| c.periodic_spikes()),
            source_account,
            synth_id,
            tags: HashMap::new(),
        }
    }
}

impl Resource for CwAnomalyDetector {
    fn cli_command(&self) -> Option<String> {
        Some("aws cloudwatch describe-anomaly-detectors".to_string())
    }

    fn id(&self) -> &str {
        &self.synth_id
    }

    fn name(&self) -> &str {
        &self.label
    }

    fn resource_type(&self) -> &str {
        "CW Anomaly Detector"
    }

    fn state(&self) -> ResourceState {
        match self.state.as_str() {
            "TRAINED" => ResourceState::Available,
            "PENDING_TRAINING" => ResourceState::Unknown("TRAINING".to_string()),
            "TRAINED_INSUFFICIENT_DATA" => ResourceState::Unknown("INSUFFICIENT".to_string()),
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
            "{} {} {} anomaly detector {}",
            self.label, self.namespace, self.metric_name, self.state
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        let mut rows = vec![
            ("Type".to_string(), self.detector_type.clone()),
            ("State".to_string(), self.state.clone()),
        ];
        if !self.expression.is_empty() {
            rows.push(("Expression".to_string(), self.expression.clone()));
        } else {
            rows.push(("Namespace".to_string(), self.namespace.clone()));
            rows.push(("Metric".to_string(), self.metric_name.clone()));
            rows.push(("Stat".to_string(), self.stat.clone()));
        }
        for (k, v) in &self.dimensions {
            rows.push((format!("  {}", k), v.clone()));
        }
        if !self.source_account.is_empty() {
            rows.push(("Source account".to_string(), self.source_account.clone()));
        }
        if let Some(spikes) = self.periodic_spikes {
            rows.push((
                "Periodic spikes".to_string(),
                if spikes { "expected (excluded from training)" } else { "not expected" }
                    .to_string(),
            ));
        }
        rows
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/cloudwatch/home?region={}#anomaly-detection:",
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

// ── CwInsightRule (Insights sub-tab) ─────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct CwInsightRule {
    pub name: String,
    pub state: String,
    pub schema: String,
    pub definition: String,
    pub managed: bool,
    pub on_transformed_logs: bool,
    pub tags: HashMap<String, String>,
}

impl CwInsightRule {
    pub fn from_sdk(r: &aws_sdk_cloudwatch::types::InsightRule) -> Self {
        Self {
            name: r.name().unwrap_or("").to_string(),
            state: r.state().unwrap_or("").to_string(),
            schema: r.schema().unwrap_or("").to_string(),
            definition: r.definition().unwrap_or("").to_string(),
            managed: r.managed_rule().unwrap_or(false),
            on_transformed_logs: r.apply_on_transformed_logs().unwrap_or(false),
            tags: HashMap::new(),
        }
    }

    /// Definition JSON reformatted for the pane; unparseable text stays as-is.
    pub fn definition_pretty(&self) -> String {
        serde_json::from_str::<serde_json::Value>(&self.definition)
            .ok()
            .and_then(|v| serde_json::to_string_pretty(&v).ok())
            .unwrap_or_else(|| self.definition.clone())
    }
}

crate::sections! {
    pub enum CwInsightRuleDetailSection,
    pub static CW_INSIGHT_RULE_SECTIONS = [
        Details "Details",
        Definition "Definition",
    ]
}

impl Resource for CwInsightRule {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&CW_INSIGHT_RULE_SECTIONS)
    }

    fn cli_command(&self) -> Option<String> {
        Some("aws cloudwatch describe-insight-rules".to_string())
    }

    fn id(&self) -> &str {
        &self.name
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn resource_type(&self) -> &str {
        "CW Insight Rule"
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
            "{} contributor insights rule {} {}",
            self.name,
            self.state,
            if self.managed { "managed" } else { "custom" }
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Rule".to_string(), self.name.clone()),
            ("State".to_string(), self.state.clone()),
            (
                "Managed".to_string(),
                if self.managed { "yes" } else { "no" }.to_string(),
            ),
        ]
    }

    fn raw_content(&self) -> Option<String> {
        Some(self.definition_pretty())
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/cloudwatch/home?region={}#contributorInsights:",
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

// ── CwAccountPolicy (Account Policies sub-tab) ───────────────────────────────

/// An account-level CloudWatch Logs policy — data protection, org-enforced
/// subscription filters, field indexes, transformers, metric extraction. One
/// row per (type, name).
#[derive(Debug, Clone)]
pub struct CwAccountPolicy {
    pub policy_name: String,
    pub policy_type: String,
    pub type_display: String,
    pub scope: String,
    pub selection_criteria: String,
    pub last_updated_ms: Option<i64>,
    pub document: String,
    pub account_id: String,
    pub synth_id: String,
    pub tags: HashMap<String, String>,
}

impl CwAccountPolicy {
    pub fn from_sdk(p: &aws_sdk_cloudwatchlogs::types::AccountPolicy) -> Self {
        let policy_type = p
            .policy_type()
            .map(|t| t.as_str().to_string())
            .unwrap_or_default();
        let type_display = match policy_type.as_str() {
            "DATA_PROTECTION_POLICY" => "Data protection",
            "SUBSCRIPTION_FILTER_POLICY" => "Subscription filter",
            "FIELD_INDEX_POLICY" => "Field index",
            "TRANSFORMER_POLICY" => "Transformer",
            "METRIC_EXTRACTION_POLICY" => "Metric extraction",
            other => other,
        }
        .to_string();
        let policy_name = p.policy_name().unwrap_or("").to_string();
        let document = p
            .policy_document()
            .and_then(|d| {
                serde_json::from_str::<serde_json::Value>(d)
                    .ok()
                    .and_then(|v| serde_json::to_string_pretty(&v).ok())
            })
            .or_else(|| p.policy_document().map(|d| d.to_string()))
            .unwrap_or_default();
        Self {
            synth_id: format!("{}:{}", policy_type, policy_name),
            policy_name,
            policy_type,
            type_display,
            scope: p.scope().map(|s| s.as_str().to_string()).unwrap_or_default(),
            selection_criteria: p.selection_criteria().unwrap_or("").to_string(),
            last_updated_ms: p.last_updated_time(),
            document,
            account_id: p.account_id().unwrap_or("").to_string(),
            tags: HashMap::new(),
        }
    }
}

crate::sections! {
    pub enum CwAccountPolicyDetailSection,
    pub static CW_ACCOUNT_POLICY_SECTIONS = [
        Details "Details",
        Document "Document",
    ]
}

impl Resource for CwAccountPolicy {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&CW_ACCOUNT_POLICY_SECTIONS)
    }

    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws logs describe-account-policies --policy-type {}",
            crate::aws::resource::shell_quote(&self.policy_type)
        ))
    }

    fn id(&self) -> &str {
        &self.synth_id
    }

    fn name(&self) -> &str {
        &self.policy_name
    }

    fn resource_type(&self) -> &str {
        "CW Account Policy"
    }

    fn state(&self) -> ResourceState {
        ResourceState::stateless()
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!(
            "{} {} {} account policy logs",
            self.policy_name, self.policy_type, self.type_display
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Policy".to_string(), self.policy_name.clone()),
            ("Type".to_string(), self.type_display.clone()),
            ("Scope".to_string(), self.scope.clone()),
        ]
    }

    fn raw_content(&self) -> Option<String> {
        Some(self.document.clone())
    }

    fn console_url(&self, _region: &str) -> Option<String> {
        None
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// All account-level Logs policies, one DescribeAccountPolicies call per
/// policy type. Per-type failures are collected, not surfaced individually —
/// newer types don't exist in every region/partition, so only the caller's
/// "all types failed" case is worth a warning.
pub async fn fetch_account_policies(
    client: LogsClient,
) -> (Vec<CwAccountPolicy>, Vec<String>) {
    use aws_sdk_cloudwatchlogs::types::PolicyType;
    let mut out = Vec::new();
    let mut errors = Vec::new();
    for pt in [
        PolicyType::DataProtectionPolicy,
        PolicyType::SubscriptionFilterPolicy,
        PolicyType::FieldIndexPolicy,
        PolicyType::TransformerPolicy,
        PolicyType::MetricExtractionPolicy,
    ] {
        match client
            .describe_account_policies()
            .policy_type(pt)
            .send()
            .await
        {
            Ok(resp) => out.extend(
                resp.account_policies()
                    .iter()
                    .map(CwAccountPolicy::from_sdk),
            ),
            Err(e) => errors.push(crate::error::sdk_error_message(&e)),
        }
    }
    (out, errors)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_search_range_lookback_and_cycle() {
        assert_eq!(LogSearchRange::M15.lookback_ms(), 15 * 60 * 1000);
        assert_eq!(LogSearchRange::H1.lookback_ms(), 60 * 60 * 1000);
        assert_eq!(LogSearchRange::D3.lookback_ms(), 3 * 24 * 60 * 60 * 1000);
        assert_eq!(LogSearchRange::default(), LogSearchRange::H1);

        // cycle wraps through every preset and back to the start.
        let mut r = LogSearchRange::M15;
        let mut seen = vec![r];
        for _ in 0..5 {
            r = r.cycle();
            seen.push(r);
        }
        assert_eq!(
            seen,
            vec![
                LogSearchRange::M15,
                LogSearchRange::H1,
                LogSearchRange::H3,
                LogSearchRange::H12,
                LogSearchRange::D1,
                LogSearchRange::D3,
            ]
        );
        assert_eq!(r.cycle(), LogSearchRange::M15);
    }

    #[test]
    fn log_search_range_wider_narrower_saturate() {
        // wider() steps up and saturates at D3 (no wrap, unlike cycle()).
        assert_eq!(LogSearchRange::M15.wider(), LogSearchRange::H1);
        assert_eq!(LogSearchRange::D1.wider(), LogSearchRange::D3);
        assert_eq!(LogSearchRange::D3.wider(), LogSearchRange::D3);
        // narrower() steps down and saturates at M15.
        assert_eq!(LogSearchRange::D3.narrower(), LogSearchRange::D1);
        assert_eq!(LogSearchRange::H1.narrower(), LogSearchRange::M15);
        assert_eq!(LogSearchRange::M15.narrower(), LogSearchRange::M15);
    }

    #[test]
    fn composite_alarm_rule_children_parsing() {
        // Names and ARNs, AND/OR/NOT, deduped, in rule order.
        let rule = r#"ALARM("cpu-high") OR (ALARM("mem-high") AND NOT OK("cpu-high"))"#;
        assert_eq!(
            alarm_rule_children(rule),
            vec!["cpu-high".to_string(), "mem-high".to_string()]
        );

        let arn = r#"ALARM("arn:aws:cloudwatch:us-east-1:123:alarm:a")"#;
        assert_eq!(
            alarm_rule_children(arn),
            vec!["arn:aws:cloudwatch:us-east-1:123:alarm:a".to_string()]
        );

        // No references / constant-only rules yield nothing.
        assert!(alarm_rule_children("TRUE").is_empty());
        assert!(alarm_rule_children("").is_empty());
    }

    #[test]
    fn dashboard_body_parsing() {
        let body = r##"{
            "widgets": [
                { "type": "metric", "properties": {
                    "title": "CPU", "region": "us-east-1",
                    "metrics": [["AWS/EC2","CPUUtilization","InstanceId","i-1"],
                                ["AWS/ELB","RequestCount"]] } },
                { "type": "text", "properties": { "markdown": "# Hello\nworld" } },
                { "type": "alarm", "properties": {
                    "title": "Alarms",
                    "alarms": ["arn:aws:cloudwatch:us-east-1:1:alarm:cpu"] } }
            ]
        }"##;
        let (pretty, widgets) = parse_dashboard_body(body);
        assert!(pretty.contains("\"widgets\"")); // pretty-printed JSON round-trips
        assert_eq!(widgets.len(), 3);

        assert_eq!(widgets[0].kind, "metric");
        assert_eq!(widgets[0].title, "CPU");
        assert_eq!(widgets[0].metrics.len(), 2);

        assert_eq!(widgets[1].kind, "text");
        assert!(widgets[1].markdown.starts_with("# Hello"));

        assert_eq!(widgets[2].kind, "alarm");
        assert_eq!(
            widgets[2].alarm_arns,
            vec!["arn:aws:cloudwatch:us-east-1:1:alarm:cpu".to_string()]
        );

        // Malformed JSON falls back to raw + no widgets.
        let (raw, w) = parse_dashboard_body("not json");
        assert_eq!(raw, "not json");
        assert!(w.is_empty());
    }

    /// The console abbreviates every token it can as `"."`, so a widget's second
    /// and later metric lines are mostly dots. Resolving them against the wrong
    /// line (or not at all) charts a namespace-less metric that silently returns
    /// no data — indistinguishable from a quiet resource.
    #[test]
    fn widget_metric_dot_repeat_expands_against_the_previous_line() {
        let props = serde_json::json!({
            "metrics": [
                ["AWS/EC2", "CPUUtilization", "InstanceId", "i-1", {"stat": "Maximum"}],
                [".", ".", ".", "i-2"],
                [".", "NetworkIn", ".", "."],
                [{"expression": "m1+m2", "id": "e1", "label": "total"}]
            ]
        });
        let m = parse_widget_metrics(Some(&props));
        assert_eq!(m.len(), 4);

        assert_eq!(m[0].namespace, "AWS/EC2");
        assert_eq!(m[0].dimensions, vec![("InstanceId".into(), "i-1".into())]);
        assert_eq!(m[0].stat.as_deref(), Some("Maximum"));
        // No explicit label → the console's dimension-value + metric-name form.
        assert_eq!(m[0].label, "i-1 CPUUtilization");

        // Dots inherit position-wise from the line above.
        assert_eq!(m[1].namespace, "AWS/EC2");
        assert_eq!(m[1].metric_name, "CPUUtilization");
        assert_eq!(m[1].dimensions, vec![("InstanceId".into(), "i-2".into())]);
        // …and the *previous line*, not the first: i-2 carries forward.
        assert_eq!(m[2].metric_name, "NetworkIn");
        assert_eq!(m[2].dimensions, vec![("InstanceId".into(), "i-2".into())]);

        // An expression line is kept (so the widget can say why it's blank) but
        // carries no metric, and must not reset what "." resolves against.
        assert_eq!(m[3].expression.as_deref(), Some("m1+m2"));
        assert!(m[3].metric_name.is_empty());
        assert_eq!(m[3].label, "total");
    }

    /// Metric-math ids are only unique **within** a widget — every widget starts
    /// over at `m1` — but `GetMetricData` needs them unique across the request.
    /// Namespacing them means the expression text has to be rewritten to match,
    /// or CloudWatch rejects the whole call and the dashboard renders blank.
    #[test]
    fn expression_ids_are_namespaced_per_widget() {
        assert_eq!(dashboard_query_id(3, 0, Some("m1")), "w3_m1");
        assert_eq!(dashboard_query_id(3, 0, None), "w3m0");
        // Two widgets, same author id, different request ids.
        assert_ne!(dashboard_query_id(0, 0, Some("m1")), dashboard_query_id(1, 0, Some("m1")));

        let ids: HashMap<String, String> = [
            ("m1".to_string(), "w3_m1".to_string()),
            ("m2".to_string(), "w3_m2".to_string()),
        ]
        .into_iter()
        .collect();

        assert_eq!(rewrite_expression("m1+m2", &ids), "w3_m1+w3_m2");
        assert_eq!(rewrite_expression("(m1/m2)*100", &ids), "(w3_m1/w3_m2)*100");
        // Unknown identifiers (functions, other ids) pass through untouched.
        assert_eq!(rewrite_expression("SUM(m1, e9)", &ids), "SUM(w3_m1, e9)");
        // A token that merely *contains* an id is not a reference.
        assert_eq!(rewrite_expression("m10+m1x", &ids), "m10+m1x");
    }

    /// A dashboard body carries the window its author chose to view it through
    /// (`"start": "-PT3H"`), which beats any default of ours. It rarely lands on
    /// a preset, so it resolves to the smallest one that *covers* it — rounding
    /// down would show less than the author needed.
    #[test]
    fn body_start_resolves_to_the_covering_preset() {
        let s = |body: &str| parse_dashboard_settings(body).range;

        assert_eq!(s(r#"{"start":"-PT1H"}"#), Some(MetricsTimeRange::OneHour));
        assert_eq!(s(r#"{"start":"-PT30M"}"#), Some(MetricsTimeRange::OneHour));
        assert_eq!(s(r#"{"start":"-PT3H"}"#), Some(MetricsTimeRange::SixHours));
        assert_eq!(s(r#"{"start":"-PT12H"}"#), Some(MetricsTimeRange::TwentyFourHours));
        assert_eq!(s(r#"{"start":"-P7D"}"#), Some(MetricsTimeRange::SevenDays));
        assert_eq!(s(r#"{"start":"-P1W"}"#), Some(MetricsTimeRange::SevenDays));
        assert_eq!(s(r#"{"start":"-P1DT12H"}"#), Some(MetricsTimeRange::SevenDays));
        // Longer than any preset saturates rather than wrapping to the shortest.
        assert_eq!(s(r#"{"start":"-P30D"}"#), Some(MetricsTimeRange::SevenDays));

        // No start, an absolute timestamp, or a month/year designator (which has
        // no fixed length in seconds): keep whatever range the pane is on.
        assert_eq!(s(r#"{"widgets":[]}"#), None);
        assert_eq!(s(r#"{"start":"2026-01-01T00:00:00Z"}"#), None);
        assert_eq!(s(r#"{"start":"-P1M"}"#), None);
        assert_eq!(s("not json"), None);

        assert!(parse_dashboard_settings(r#"{"periodOverride":"inherit"}"#).period_inherit);
        assert!(!parse_dashboard_settings(r#"{"periodOverride":"auto"}"#).period_inherit);
        assert!(!parse_dashboard_settings(r#"{}"#).period_inherit);
    }

    /// `periodOverride: inherit` means the author wants every widget's own
    /// period discarded in favour of the dashboard's time range.
    #[test]
    fn period_inherit_discards_widget_periods() {
        let body = r##"{"widgets":[
            {"type":"metric","x":0,"y":0,"width":6,"height":6,
             "properties":{"period":900,
               "metrics":[["AWS/EC2","CPUUtilization","InstanceId","i-1"]]}}
        ]}"##;
        let widgets = parse_dashboard_body(body).1;

        // Without inherit the widget's own 900s wins over the 300s range floor.
        let (normal, _) = build_dashboard_queries(&widgets, "us-east-1", 300, false);
        assert_eq!(normal["us-east-1"][0].metric_stat().unwrap().period(), Some(900));

        let (inherited, _) = build_dashboard_queries(&widgets, "us-east-1", 300, true);
        assert_eq!(inherited["us-east-1"][0].metric_stat().unwrap().period(), Some(300));
    }

    /// The widget properties phase 3 added.
    #[test]
    fn stacked_and_right_axis_properties_are_captured() {
        let body = r##"{"widgets":[
            {"type":"metric","x":0,"y":0,"width":12,"height":6,
             "properties":{"stacked":true,"yAxis":{"right":{"min":0,"max":1}},
               "metrics":[["AWS/ELB","RequestCount"],
                          ["AWS/ELB","Latency",{"yAxis":"right"}]]}}
        ]}"##;
        let w = &parse_dashboard_body(body).1[0];
        assert!(w.stacked);
        assert_eq!((w.y_right_min, w.y_right_max), (Some(0.0), Some(1.0)));
        assert!(!w.metrics[0].right_axis);
        assert!(w.metrics[1].right_axis);
    }

    /// Dynamic-label tokens render literally if nothing substitutes them, and a
    /// legend reading `${PROP('Dim.AvailabilityZone')}` four times says nothing
    /// about which line is which. A token with no local answer is dropped, not
    /// shown raw.
    #[test]
    fn dynamic_label_tokens_resolve_or_disappear() {
        let body = r##"{"widgets": [
            {"type":"metric","x":0,"y":0,"width":12,"height":6,
             "properties":{"region":"us-east-1","stat":"Sum","period":300,
               "metrics":[["AWS/NetworkFirewall","Packets",
                           "AvailabilityZone","us-east-1a","FirewallName","fw-1"]]}}
        ]}"##;
        let w = &parse_dashboard_body(body).1[0];
        let m = &w.metrics[0];
        let points = vec![(0.0, 10.0), (60.0, 30.0), (120.0, 20.0)];

        let r = |s: &str| resolve_dynamic_label(s, m, w, &points);

        assert_eq!(r("${PROP('Dim.AvailabilityZone')}"), "us-east-1a");
        assert_eq!(r("${PROP('Dim.FirewallName')} ${PROP('MetricName')}"), "fw-1 Packets");
        assert_eq!(r("${PROP('Namespace')}"), "AWS/NetworkFirewall");
        assert_eq!(r("${PROP('Stat')} over ${PROP('Period')}s"), "Sum over 300s");

        // Statistic tokens come off the series itself.
        assert_eq!(r("max ${MAX}"), "max 30");
        assert_eq!(r("min ${MIN}"), "min 10");
        assert_eq!(r("avg ${AVG}"), "avg 20");
        assert_eq!(r("${LAST}"), "20");
        assert_eq!(r("${SUM}"), "60");

        // A dimension the metric doesn't have, and an unknown property: dropped,
        // and the collapse doesn't leave a double space behind.
        assert_eq!(r("a ${PROP('Dim.Nope')} b"), "a b");
        assert_eq!(r("${PROP('AccountLabel')}"), "");
        // Text with no tokens is untouched; an unterminated one stays visible
        // rather than swallowing the rest of the label.
        assert_eq!(r("plain label"), "plain label");
        assert_eq!(r("oops ${MAX"), "oops ${MAX");
    }

    /// `GetMetricData` rejects a `SEARCH()` expression that carries no `Period`
    /// — "Period is required when using SEARCH" — and the rejection is
    /// **request-wide**, so one such widget blanks every other widget in its
    /// region. AWS's own Network Firewall dashboard is built entirely from
    /// SEARCH expressions, which is how this surfaced.
    #[test]
    fn search_expressions_carry_a_period() {
        let body = r##"{"widgets": [
            {"type":"metric","x":0,"y":0,"width":12,"height":6,
             "properties":{"title":"Firewall","region":"us-east-1","metrics":[
               [{"expression":"SUM(SEARCH('{AWS/NetworkFirewall,AvailabilityZone,FirewallName} MetricName=\"Packets\"','Sum'))","id":"e1"}]
             ]}}
        ]}"##;
        let widgets = parse_dashboard_body(body).1;
        let (by_region, capped) = build_dashboard_queries(&widgets, "us-east-1", 300, false);
        assert!(!capped);

        let queries = &by_region["us-east-1"];
        assert_eq!(queries.len(), 1);
        assert!(queries[0].expression().is_some());
        assert_eq!(queries[0].period(), Some(300));
    }

    /// The period floor and the region grouping, on the same walk.
    #[test]
    fn queries_group_by_region_and_never_go_finer_than_the_range() {
        let body = r##"{"widgets": [
            {"type":"metric","x":0,"y":0,"width":12,"height":6,
             "properties":{"period":60,"metrics":[["AWS/EC2","CPUUtilization","InstanceId","i-1"]]}},
            {"type":"metric","x":12,"y":0,"width":12,"height":6,
             "properties":{"region":"eu-west-1",
               "metrics":[["AWS/EC2","CPUUtilization","InstanceId","i-2"]]}}
        ]}"##;
        let widgets = parse_dashboard_body(body).1;
        let (by_region, _) = build_dashboard_queries(&widgets, "us-east-1", 3600, false);

        assert_eq!(by_region.len(), 2);
        assert_eq!(by_region["us-east-1"].len(), 1);
        assert_eq!(by_region["eu-west-1"].len(), 1);
        // The widget asked for 60s over a range whose floor is 3600s.
        let stat = by_region["us-east-1"][0].metric_stat().unwrap();
        assert_eq!(stat.period(), Some(3600));
    }

    /// Identifiers inside a SEARCH()'s quoted argument are data, not references
    /// — rewriting inside the string would corrupt the search and silently
    /// return nothing.
    #[test]
    fn expression_rewrite_leaves_quoted_strings_alone() {
        let ids: HashMap<String, String> = [("Errors".to_string(), "w0_Errors".to_string())]
            .into_iter()
            .collect();
        let expr = r#"SUM(SEARCH('{AWS/Lambda,FunctionName} MetricName="Errors"', 'Sum', 300))"#;
        assert_eq!(rewrite_expression(expr, &ids), expr);
    }

    /// Annotations are either single lines or two-element **bands**. Reading
    /// only the object form drops every band; reading only the first element of
    /// a band halves it.
    #[test]
    fn horizontal_annotations_cover_lines_and_bands() {
        let props = serde_json::json!({
            "annotations": {"horizontal": [
                {"label": "limit", "value": 100},
                [{"label": "target", "value": 20}, {"value": 40}],
                {"label": "no value here"}
            ]}
        });
        let a = parse_horizontal_annotations(Some(&props));
        assert_eq!(a.len(), 3); // both band edges kept, the value-less entry dropped
        assert_eq!(a[0], CwAnnotation { label: "limit".into(), value: 100.0 });
        assert_eq!(a[1].value, 20.0);
        assert_eq!(a[2].value, 40.0);

        assert!(parse_horizontal_annotations(Some(&serde_json::json!({}))).is_empty());
    }

    /// The view / axis / visibility properties the non-timeSeries renderers key
    /// on. A hidden line exists only to feed metric math and must never draw.
    #[test]
    fn widget_view_axis_and_hidden_lines_are_captured() {
        let body = r##"{"widgets": [
            {"type":"metric","x":0,"y":0,"width":6,"height":3,
             "properties":{"view":"gauge","sparkline":true,
               "yAxis":{"left":{"min":0,"max":200}},
               "metrics":[["AWS/EC2","CPUUtilization",{"id":"m1","visible":false}],
                          [{"expression":"m1*2","id":"e1","label":"doubled"}]]}}
        ]}"##;
        let w = &parse_dashboard_body(body).1[0];
        assert_eq!(w.view, "gauge");
        assert!(w.sparkline);
        assert_eq!((w.y_min, w.y_max), (Some(0.0), Some(200.0)));

        assert!(!w.metrics[0].visible);
        assert_eq!(w.metrics[0].id.as_deref(), Some("m1"));
        // The expression line is visible by default and carries the reference.
        assert!(w.metrics[1].visible);
        assert_eq!(w.metrics[1].expression.as_deref(), Some("m1*2"));
    }

    /// A body that omits x/y is laid out by the console left-to-right, wrapping
    /// at column 24. Defaulting them all to (0,0) would stack every widget.
    #[test]
    fn widgets_without_coordinates_auto_flow() {
        let body = r##"{"widgets": [
            {"type": "metric", "width": 12, "height": 6},
            {"type": "metric", "width": 12, "height": 6},
            {"type": "metric", "width": 24, "height": 3},
            {"type": "metric", "x": 0, "y": 40, "width": 6, "height": 6}
        ]}"##;
        let (_, w) = parse_dashboard_body(body);
        assert_eq!((w[0].x, w[0].y), (0, 0));
        assert_eq!((w[1].x, w[1].y), (12, 0)); // fills the first row
        assert_eq!((w[2].x, w[2].y), (0, 6)); // wraps past 24 columns
        assert_eq!((w[3].x, w[3].y), (0, 40)); // explicit coordinates win
    }

    /// The bleed bug: raw log payloads carry ANSI colour codes, tabs and
    /// carriage returns. Ratatui writes them into the cell and counts them as
    /// ~0-wide, so the *terminal* acts on them and the row repaints from the
    /// left edge, over the list pane. Nothing control-ish may survive.
    #[test]
    fn sanitize_strips_everything_a_terminal_would_act_on() {
        assert_eq!(sanitize_log_line("\u{1b}[31mERROR\u{1b}[0m boom"), "ERROR boom");
        assert_eq!(sanitize_log_line("a\u{1b}]0;retitle\u{7}b"), "ab");
        assert_eq!(sanitize_log_line("col\tval"), "col    val");
        assert_eq!(sanitize_log_line("overwrite\rme"), "overwriteme");
        assert_eq!(sanitize_log_line("nul\0byte\u{8}"), "nulbyte");
        assert!(!sanitize_log_line("\u{1b}[1;32m ok \u{1b}[m")
            .chars()
            .any(|c| c.is_control()));
        // Plain text, including leading indentation, is untouched.
        assert_eq!(sanitize_log_line("    at Foo.bar(Foo.java:42)  "), "    at Foo.bar(Foo.java:42)");
    }

    /// A multi-line event becomes one row per line (console behaviour) rather
    /// than one row with embedded newlines the terminal would break on.
    #[test]
    fn multi_line_events_split_into_rows() {
        let mut out = Vec::new();
        push_log_lines(&mut out, 0, "app", "boom\n  at Foo.bar\n  at Baz.qux\n\n");
        assert_eq!(
            out.iter().map(|l| l.message.as_str()).collect::<Vec<_>>(),
            vec!["boom", "  at Foo.bar", "  at Baz.qux"]
        );
        assert_eq!(
            out.iter().map(|l| l.continuation).collect::<Vec<_>>(),
            vec![false, true, true]
        );
        // Continuations keep ts/stream so the `/` filter still matches on them.
        assert!(out.iter().all(|l| l.stream == "app" && l.ts == out[0].ts));
    }

    #[test]
    fn single_line_events_stay_one_row() {
        let mut out = Vec::new();
        push_log_lines(&mut out, 0, "", "just one line");
        assert_eq!(out.len(), 1);
        assert!(!out[0].continuation);
        // An entirely empty event still yields a row rather than vanishing.
        push_log_lines(&mut out, 0, "", "");
        assert_eq!(out.len(), 2);
    }
}
