use crate::aws::client::AwsClients;
use crate::aws::resource::{shell_quote, Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use aws_sdk_xray::Client as XRayClient;
use std::any::Any;
use std::collections::HashMap;
use tokio::sync::mpsc;

/// AWS X-Ray — "why is this slow / erroring, and where?". Three sub-tabs:
/// **Service Map** (`GetServiceGraph` nodes with their edges), **Traces**
/// (`GetTraceSummaries`, drill → `BatchGetTraces`) and **Groups & Sampling**
/// (`GetGroups` + `GetSamplingRules` — the answer to "why are there no
/// traces?" is often a sampling rule).
///
/// Everything is scoped to a **time window** (`[`/`]` in the list pane:
/// 5m / 15m / 1h / 6h). The service struct carries the window and the list is
/// variant-cached per window, the WAF-scope pattern. 6h is the ceiling:
/// `GetServiceGraph` rejects a longer span.
pub struct XRayService {
    client: XRayClient,
    window: XRayWindow,
}

impl XRayService {
    pub fn new(aws_clients: &AwsClients, window: XRayWindow) -> Self {
        Self {
            client: aws_clients.xray_client(),
            window,
        }
    }
}

/// The X-Ray list's look-back window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum XRayWindow {
    FiveMinutes,
    FifteenMinutes,
    #[default]
    OneHour,
    SixHours,
}

impl XRayWindow {
    pub fn secs(self) -> i64 {
        match self {
            XRayWindow::FiveMinutes => 300,
            XRayWindow::FifteenMinutes => 900,
            XRayWindow::OneHour => 3600,
            XRayWindow::SixHours => 6 * 3600,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            XRayWindow::FiveMinutes => "5m",
            XRayWindow::FifteenMinutes => "15m",
            XRayWindow::OneHour => "1h",
            XRayWindow::SixHours => "6h",
        }
    }

    pub const ALL: [XRayWindow; 4] = [
        XRayWindow::FiveMinutes,
        XRayWindow::FifteenMinutes,
        XRayWindow::OneHour,
        XRayWindow::SixHours,
    ];

    /// Wider (`]`), saturating at 6h.
    pub fn wider(self) -> Self {
        match self {
            XRayWindow::FiveMinutes => XRayWindow::FifteenMinutes,
            XRayWindow::FifteenMinutes => XRayWindow::OneHour,
            _ => XRayWindow::SixHours,
        }
    }

    /// Narrower (`[`), saturating at 5m.
    pub fn narrower(self) -> Self {
        match self {
            XRayWindow::SixHours => XRayWindow::OneHour,
            XRayWindow::OneHour => XRayWindow::FifteenMinutes,
            _ => XRayWindow::FiveMinutes,
        }
    }
}

/// Cap on trace summaries per load (the first pages X-Ray returns, sorted
/// newest-first afterwards) — a busy service returns thousands per
/// hour; the list is for finding the broken ones, and the `F` filter
/// narrows to faults/errors.
pub const MAX_TRACES: usize = 500;

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn fmt_dt(dt: &aws_smithy_types::DateTime) -> String {
    crate::aws::services::cloudwatch::fmt_epoch_secs(dt.secs())
}

/// "1.23 s" / "87 ms".
pub fn fmt_secs(s: f64) -> String {
    if s >= 1.0 {
        format!("{s:.2} s")
    } else {
        format!("{:.0} ms", s * 1000.0)
    }
}

#[async_trait]
impl AwsService for XRayService {
    fn service_type(&self) -> ServiceType {
        ServiceType::XRay
    }

    fn name(&self) -> &str {
        "X-Ray"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        self.list_resources_streaming(tx, ServiceType::XRay).await?;
        Ok(vec![])
    }

    async fn list_resources_streaming(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        let warn = |msg: String| {
            let _ = event_tx.send(Event::ResourceLoadWarning {
                service: service_type,
                warning: msg,
            });
        };
        let mut total = 0usize;
        let mut send = |batch: Vec<Box<dyn Resource>>, status: Option<&str>| {
            total += batch.len();
            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                service: service_type,
                resources: batch,
                progress: LoadProgress {
                    loaded_count: total,
                    total_count: None,
                    status_message: status.map(str::to_string),
                },
            });
        };

        let end = now_secs();
        let start = end - self.window.secs();
        let start_dt = aws_smithy_types::DateTime::from_secs(start);
        let end_dt = aws_smithy_types::DateTime::from_secs(end);
        let window = self.window.label();

        // ── Service map ───────────────────────────────────────────────────────
        let mut graph: Vec<aws_sdk_xray::types::Service> = Vec::new();
        let mut graph_failed = None;
        let mut pages = self
            .client
            .get_service_graph()
            .start_time(start_dt)
            .end_time(end_dt)
            .into_paginator()
            .send();
        while let Some(page) = pages.next().await {
            match page {
                Ok(p) => graph.extend(p.services().iter().cloned()),
                Err(e) => {
                    graph_failed = Some(crate::error::sdk_error_message(&e));
                    break;
                }
            }
        }
        let nodes = XRayNode::from_graph(&graph, self.window);
        send(
            nodes.into_iter().map(|n| Box::new(n) as Box<dyn Resource>).collect(),
            Some("Loading traces…"),
        );

        // ── Trace summaries (capped, newest first) ────────────────────────────
        let mut traces: Vec<XRayTrace> = Vec::new();
        let mut traces_failed = None;
        let mut truncated = false;
        let mut pages = self
            .client
            .get_trace_summaries()
            .start_time(start_dt)
            .end_time(end_dt)
            .into_paginator()
            .send();
        while let Some(page) = pages.next().await {
            match page {
                Ok(p) => {
                    traces.extend(p.trace_summaries().iter().map(XRayTrace::from_sdk));
                    if traces.len() >= MAX_TRACES {
                        traces.truncate(MAX_TRACES);
                        truncated = true;
                        break;
                    }
                }
                Err(e) => {
                    traces_failed = Some(crate::error::sdk_error_message(&e));
                    break;
                }
            }
        }
        traces.sort_by_key(|t| std::cmp::Reverse(t.start_epoch));
        if truncated {
            warn(format!(
                "X-Ray: trace list capped at {MAX_TRACES} for the last {window} — narrow the window with [ to see them all"
            ));
        }
        send(
            traces.into_iter().map(|t| Box::new(t) as Box<dyn Resource>).collect(),
            Some("Loading groups and sampling rules…"),
        );

        // ── Groups + sampling rules ───────────────────────────────────────────
        let mut config: Vec<Box<dyn Resource>> = Vec::new();
        let mut pages = self.client.get_groups().into_paginator().send();
        while let Some(page) = pages.next().await {
            match page {
                Ok(p) => config.extend(
                    p.groups()
                        .iter()
                        .map(|g| Box::new(XRayGroup::from_sdk(g)) as Box<dyn Resource>),
                ),
                Err(e) => {
                    warn(format!("X-Ray groups: {}", crate::error::sdk_error_message(&e)));
                    break;
                }
            }
        }
        let mut rules: Vec<XRaySamplingRule> = Vec::new();
        let mut pages = self.client.get_sampling_rules().into_paginator().send();
        while let Some(page) = pages.next().await {
            match page {
                Ok(p) => rules.extend(
                    p.sampling_rule_records()
                        .iter()
                        .filter_map(XRaySamplingRule::from_sdk),
                ),
                Err(e) => {
                    warn(format!("X-Ray sampling rules: {}", crate::error::sdk_error_message(&e)));
                    break;
                }
            }
        }
        // Evaluation order: lowest priority number wins.
        rules.sort_by_key(|r| r.priority);
        config.extend(rules.into_iter().map(|r| Box::new(r) as Box<dyn Resource>));
        send(config, None);

        match (graph_failed, traces_failed) {
            (Some(g), Some(_)) if total == 0 => {
                let _ = event_tx.send(Event::ResourceLoadError {
                    service: service_type,
                    error: format!("X-Ray unavailable: {g}"),
                });
                return Ok(());
            }
            (g, t) => {
                if let Some(g) = g {
                    warn(format!("X-Ray service map: {g}"));
                }
                if let Some(t) = t {
                    warn(format!("X-Ray traces: {t}"));
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

// ── Service map node ──────────────────────────────────────────────────────────

/// Request counters shared by nodes and edges.
#[derive(Debug, Clone, Copy, Default)]
pub struct XRayStats {
    pub total: i64,
    pub ok: i64,
    pub errors: i64,
    pub throttles: i64,
    pub faults: i64,
    /// Sum of response times (s) — `/ total` is the mean.
    pub total_response_time: f64,
}

impl XRayStats {
    fn from_parts(
        total: Option<i64>,
        ok: Option<i64>,
        err: Option<&aws_sdk_xray::types::ErrorStatistics>,
        fault: Option<&aws_sdk_xray::types::FaultStatistics>,
        time: Option<f64>,
    ) -> Self {
        Self {
            total: total.unwrap_or(0),
            ok: ok.unwrap_or(0),
            errors: err.and_then(|e| e.total_count()).unwrap_or(0),
            throttles: err.and_then(|e| e.throttle_count()).unwrap_or(0),
            faults: fault.and_then(|f| f.total_count()).unwrap_or(0),
            total_response_time: time.unwrap_or(0.0),
        }
    }

    pub fn pct(&self, n: i64) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            n as f64 * 100.0 / self.total as f64
        }
    }

    pub fn mean(&self) -> Option<f64> {
        (self.total > 0).then(|| self.total_response_time / self.total as f64)
    }
}

/// One edge of the map, resolved to the node at the other end.
#[derive(Debug, Clone)]
pub struct XRayEdge {
    pub name: String,
    pub node_type: String,
    pub stats: XRayStats,
}

#[derive(Debug, Clone)]
pub struct XRayNode {
    /// `type::name` — the graph's `ReferenceId` isn't stable across queries.
    pub key: String,
    pub name: String,
    pub node_type: String,
    pub aliases: Vec<String>,
    pub account: Option<String>,
    pub root: bool,
    pub state: Option<String>,
    pub stats: Option<XRayStats>,
    pub p50: Option<f64>,
    pub p90: Option<f64>,
    pub p99: Option<f64>,
    pub downstream: Vec<XRayEdge>,
    pub upstream: Vec<XRayEdge>,
    pub window: &'static str,
    pub window_secs: i64,
    tags: HashMap<String, String>,
}

/// Percentile from a response-time histogram (`value` s, `count`).
fn histogram_percentile(h: &[aws_sdk_xray::types::HistogramEntry], p: f64) -> Option<f64> {
    let mut entries: Vec<(f64, i64)> = h.iter().map(|e| (e.value(), e.count() as i64)).collect();
    let total: i64 = entries.iter().map(|e| e.1).sum();
    if total == 0 {
        return None;
    }
    entries.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let target = (total as f64 * p).ceil() as i64;
    let mut seen = 0;
    for (v, c) in entries {
        seen += c;
        if seen >= target {
            return Some(v);
        }
    }
    None
}

impl XRayNode {
    /// Build every node, resolving each edge's `ReferenceId` against the same
    /// graph (downstream) and inverting the edges (upstream callers).
    pub fn from_graph(graph: &[aws_sdk_xray::types::Service], window: XRayWindow) -> Vec<Self> {
        let by_ref: HashMap<i32, (String, String)> = graph
            .iter()
            .filter_map(|s| {
                Some((
                    s.reference_id()?,
                    (
                        s.name().unwrap_or("?").to_string(),
                        s.r#type().unwrap_or("?").to_string(),
                    ),
                ))
            })
            .collect();
        let mut nodes: Vec<XRayNode> = graph
            .iter()
            .map(|s| {
                let name = s.name().unwrap_or("?").to_string();
                let node_type = s.r#type().unwrap_or("?").to_string();
                let downstream = s
                    .edges()
                    .iter()
                    .filter_map(|e| {
                        let (n, t) = by_ref.get(&e.reference_id()?)?.clone();
                        let st = e.summary_statistics();
                        Some(XRayEdge {
                            name: n,
                            node_type: t,
                            stats: XRayStats::from_parts(
                                st.and_then(|s| s.total_count()),
                                st.and_then(|s| s.ok_count()),
                                st.and_then(|s| s.error_statistics()),
                                st.and_then(|s| s.fault_statistics()),
                                st.and_then(|s| s.total_response_time()),
                            ),
                        })
                    })
                    .collect();
                let st = s.summary_statistics();
                XRayNode {
                    key: format!("{node_type}::{name}"),
                    aliases: s.names().iter().filter(|n| **n != name).cloned().collect(),
                    account: s.account_id().map(str::to_string),
                    root: s.root().unwrap_or(false),
                    state: s.state().map(str::to_string),
                    stats: st.map(|st| {
                        XRayStats::from_parts(
                            st.total_count(),
                            st.ok_count(),
                            st.error_statistics(),
                            st.fault_statistics(),
                            st.total_response_time(),
                        )
                    }),
                    p50: histogram_percentile(s.response_time_histogram(), 0.50),
                    p90: histogram_percentile(s.response_time_histogram(), 0.90),
                    p99: histogram_percentile(s.response_time_histogram(), 0.99),
                    downstream,
                    upstream: Vec::new(),
                    window: window.label(),
                    window_secs: window.secs(),
                    name,
                    node_type,
                    tags: HashMap::new(),
                }
            })
            .collect();
        // Invert: every downstream edge is an upstream edge of its target.
        let callers: Vec<(String, XRayEdge)> = nodes
            .iter()
            .flat_map(|n| {
                n.downstream.iter().map(move |e| {
                    (
                        format!("{}::{}", e.node_type, e.name),
                        XRayEdge {
                            name: n.name.clone(),
                            node_type: n.node_type.clone(),
                            stats: e.stats,
                        },
                    )
                })
            })
            .collect();
        for (target, edge) in callers {
            if let Some(n) = nodes.iter_mut().find(|n| n.key == target) {
                n.upstream.push(edge);
            }
        }
        // Worst first: faults, then errors, then by volume.
        nodes.sort_by(|a, b| {
            let score = |n: &XRayNode| {
                n.stats
                    .map(|s| (s.faults > 0, s.errors > 0, s.total))
                    .unwrap_or((false, false, 0))
            };
            score(b).cmp(&score(a)).then_with(|| a.name.cmp(&b.name))
        });
        nodes
    }

    /// `@prefix id` for nodes that are a resource neboto lists — the value
    /// of the pane's "Jump To" row. Only types whose X-Ray node name *is* the
    /// resource's name/id are mapped.
    pub fn neboto_target(&self) -> Option<String> {
        let prefix = match self.node_type.as_str() {
            "AWS::Lambda::Function" | "AWS::Lambda" => "@lambda",
            "AWS::DynamoDB::Table" => "@ddb",
            "AWS::S3::Bucket" => "@s3",
            "AWS::StepFunctions::StateMachine" => "@sfn",
            "AWS::SNS::Topic" => "@sns",
            "AWS::EKS::Cluster" => "@eks",
            _ => return None,
        };
        Some(format!("{prefix} {}", self.name))
    }

    fn is_client(&self) -> bool {
        self.node_type == "client"
    }
}

crate::sections! {
    pub enum XRayNodeDetailSection,
    pub static XRAY_NODE_SECTIONS = [
        Overview "Overview",
        Downstream "Downstream",
        Upstream "Upstream",
    ]
}

impl Resource for XRayNode {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&XRAY_NODE_SECTIONS)
    }

    fn cli_command(&self) -> Option<String> {
        // Portable epoch arithmetic (BSD `date` has no `-d`).
        Some(format!(
            "aws xray get-service-graph --start-time $(( $(date +%s) - {} )) --end-time $(date +%s)",
            self.window_secs
        ))
    }

    fn id(&self) -> &str {
        &self.key
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn resource_type(&self) -> &str {
        "X-Ray Service"
    }

    fn state(&self) -> ResourceState {
        if self.is_client() {
            return ResourceState::stateless();
        }
        match self.stats {
            None => ResourceState::stateless(),
            Some(s) if s.faults > 0 => ResourceState::Unavailable,
            Some(s) if s.errors > 0 => ResourceState::Pending,
            Some(_) => ResourceState::Available,
        }
    }

    fn state_label(&self) -> String {
        match (self.is_client(), self.stats) {
            (true, _) | (_, None) => String::new(),
            (_, Some(s)) if s.faults > 0 => format!("{:.1}% faults", s.pct(s.faults)),
            (_, Some(s)) if s.errors > 0 => format!("{:.1}% errors", s.pct(s.errors)),
            _ => "ok".to_string(),
        }
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!("{} {} {} xray service node", self.name, self.node_type, self.aliases.join(" "))
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Service".to_string(), self.name.clone()),
            ("Type".to_string(), self.node_type.clone()),
        ]
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{region}.console.aws.amazon.com/cloudwatch/home?region={region}#xray:service-map/map"
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── Trace summary ─────────────────────────────────────────────────────────────

/// One root-cause line: which service, which entity, which exception.
#[derive(Debug, Clone)]
pub struct XRayRootCause {
    /// `fault` / `error` / `latency`.
    pub kind: &'static str,
    pub service: String,
    pub path: String,
    pub exception: Option<String>,
}

#[derive(Debug, Clone)]
pub struct XRayTrace {
    pub trace_id: String,
    pub start: Option<String>,
    pub start_epoch: i64,
    pub duration: Option<f64>,
    pub response_time: Option<f64>,
    pub fault: bool,
    pub error: bool,
    pub throttle: bool,
    pub partial: bool,
    pub method: Option<String>,
    pub url: Option<String>,
    pub status: Option<i32>,
    pub client_ip: Option<String>,
    pub user_agent: Option<String>,
    pub entry_point: Option<String>,
    pub services: Vec<String>,
    pub resource_arns: Vec<String>,
    pub root_causes: Vec<XRayRootCause>,
    /// List label: `GET /orders` or the entry point.
    pub label: String,
    tags: HashMap<String, String>,
}

impl XRayTrace {
    pub fn from_sdk(t: &aws_sdk_xray::types::TraceSummary) -> Self {
        let http = t.http();
        let method = http.and_then(|h| h.http_method()).map(str::to_string);
        let url = http.and_then(|h| h.http_url()).map(str::to_string);
        let entry_point = t.entry_point().and_then(|e| e.name()).map(str::to_string);
        // Path only — the host repeats on every row.
        let path = url.as_deref().map(|u| {
            u.split_once("://")
                .map(|(_, rest)| rest.find('/').map(|i| &rest[i..]).unwrap_or("/"))
                .unwrap_or(u)
                .to_string()
        });
        let label = match (&method, &path) {
            (Some(m), Some(p)) => format!("{m} {p}"),
            (None, Some(p)) => p.clone(),
            _ => entry_point.clone().unwrap_or_else(|| t.id().unwrap_or_default().to_string()),
        };

        let mut root_causes = Vec::new();
        for rc in t.fault_root_causes() {
            for s in rc.services() {
                let path: Vec<&str> = s.entity_path().iter().filter_map(|e| e.name()).collect();
                let exception = s
                    .entity_path()
                    .iter()
                    .flat_map(|e| e.exceptions())
                    .find_map(|x| exception_label(x.name(), x.message()));
                root_causes.push(XRayRootCause {
                    kind: "fault",
                    service: s.name().unwrap_or("?").to_string(),
                    path: path.join(" › "),
                    exception,
                });
            }
        }
        for rc in t.error_root_causes() {
            for s in rc.services() {
                let path: Vec<&str> = s.entity_path().iter().filter_map(|e| e.name()).collect();
                let exception = s
                    .entity_path()
                    .iter()
                    .flat_map(|e| e.exceptions())
                    .find_map(|x| exception_label(x.name(), x.message()));
                root_causes.push(XRayRootCause {
                    kind: "error",
                    service: s.name().unwrap_or("?").to_string(),
                    path: path.join(" › "),
                    exception,
                });
            }
        }
        for rc in t.response_time_root_causes() {
            for s in rc.services() {
                let path: Vec<String> = s
                    .entity_path()
                    .iter()
                    .filter_map(|e| {
                        let name = e.name()?;
                        Some(match e.coverage() {
                            Some(c) => format!("{name} ({:.0}%)", c * 100.0),
                            None => name.to_string(),
                        })
                    })
                    .collect();
                root_causes.push(XRayRootCause {
                    kind: "latency",
                    service: s.name().unwrap_or("?").to_string(),
                    path: path.join(" › "),
                    exception: None,
                });
            }
        }

        Self {
            trace_id: t.id().unwrap_or_default().to_string(),
            start: t.start_time().map(fmt_dt),
            start_epoch: t.start_time().map(|d| d.secs()).unwrap_or(0),
            duration: t.duration(),
            response_time: t.response_time(),
            fault: t.has_fault().unwrap_or(false),
            error: t.has_error().unwrap_or(false),
            throttle: t.has_throttle().unwrap_or(false),
            partial: t.is_partial().unwrap_or(false),
            method,
            url,
            status: http.and_then(|h| h.http_status()),
            client_ip: http.and_then(|h| h.client_ip()).map(str::to_string),
            user_agent: http.and_then(|h| h.user_agent()).map(str::to_string),
            entry_point,
            services: t.service_ids().iter().filter_map(|s| s.name()).map(str::to_string).collect(),
            resource_arns: t.resource_arns().iter().filter_map(|r| r.arn()).map(str::to_string).collect(),
            root_causes,
            label,
            tags: HashMap::new(),
        }
    }
}

fn exception_label(name: Option<&str>, message: Option<&str>) -> Option<String> {
    match (name, message) {
        (Some(n), Some(m)) if !m.is_empty() => Some(format!("{n}: {m}")),
        (Some(n), _) => Some(n.to_string()),
        (None, Some(m)) => Some(m.to_string()),
        _ => None,
    }
}

crate::sections! {
    pub enum XRayTraceDetailSection,
    pub static XRAY_TRACE_SECTIONS = [
        Overview "Overview",
        RootCause "Root Cause",
        Segments "Segments" => crate::app::App::trigger_xray_trace_load,
    ]
}

impl Resource for XRayTrace {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&XRAY_TRACE_SECTIONS)
    }

    fn references(&self) -> Vec<(String, String)> {
        self.resource_arns
            .iter()
            .map(|a| ("Resource".to_string(), a.clone()))
            .collect()
    }

    fn cli_command(&self) -> Option<String> {
        Some(format!("aws xray batch-get-traces --trace-ids {}", shell_quote(&self.trace_id)))
    }

    fn cli_actions(&self) -> Vec<crate::aws::cli_actions::CliAction> {
        use crate::aws::cli_actions::{CliAction, CliTier};
        vec![
            CliAction::batchable(
                CliTier::Inspect,
                "batch-get-traces",
                "aws xray batch-get-traces --trace-ids",
                &self.trace_id,
                "",
            ),
            CliAction::new(
                CliTier::Inspect,
                "get-trace-graph",
                format!("aws xray get-trace-graph --trace-ids {}", shell_quote(&self.trace_id)),
            ),
        ]
    }

    fn id(&self) -> &str {
        &self.trace_id
    }

    fn name(&self) -> &str {
        &self.label
    }

    fn resource_type(&self) -> &str {
        "X-Ray Trace"
    }

    fn state(&self) -> ResourceState {
        if self.fault {
            ResourceState::Unavailable
        } else if self.error || self.throttle {
            ResourceState::Pending
        } else {
            ResourceState::Available
        }
    }

    /// The `F` chips read these words — so `F` is the faults/errors filter.
    fn state_label(&self) -> String {
        if self.fault {
            "fault"
        } else if self.throttle {
            "throttled"
        } else if self.error {
            "error"
        } else {
            "ok"
        }
        .to_string()
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!(
            "{} {} {} {} {} {} xray trace",
            self.trace_id,
            self.label,
            self.entry_point.as_deref().unwrap_or(""),
            self.status.map(|s| s.to_string()).unwrap_or_default(),
            self.services.join(" "),
            self.root_causes
                .iter()
                .filter_map(|r| r.exception.as_deref())
                .collect::<Vec<_>>()
                .join(" "),
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Trace".to_string(), self.trace_id.clone()),
            ("Request".to_string(), self.label.clone()),
            (
                "Duration".to_string(),
                self.duration.map(fmt_secs).unwrap_or_default(),
            ),
        ]
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{region}.console.aws.amazon.com/cloudwatch/home?region={region}#xray:traces/{}",
            self.trace_id
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── Trace detail (BatchGetTraces) ─────────────────────────────────────────────

/// One segment or subsegment, flattened depth-first.
#[derive(Debug, Clone, Default)]
pub struct XRaySegmentRow {
    pub depth: usize,
    pub name: String,
    pub origin: Option<String>,
    /// Seconds after the trace's first segment started.
    pub offset: f64,
    pub duration: Option<f64>,
    pub fault: bool,
    pub error: bool,
    pub throttle: bool,
    pub http_status: Option<i64>,
    pub exception: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct XRayTraceDetail {
    pub rows: Vec<XRaySegmentRow>,
    /// The raw trace as JSON (segment documents parsed) — what `e` opens.
    pub raw: String,
    pub limit_exceeded: bool,
}

/// Flatten one segment document (and its subsegments) into rows.
fn flatten_segment(doc: &serde_json::Value, depth: usize, out: &mut Vec<(f64, XRaySegmentRow)>) {
    let start = doc.get("start_time").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let end = doc.get("end_time").and_then(|v| v.as_f64());
    let exception = doc
        .get("cause")
        .and_then(|c| c.get("exceptions"))
        .and_then(|x| x.as_array())
        .and_then(|a| a.first())
        .and_then(|x| {
            exception_label(
                x.get("type").and_then(|v| v.as_str()),
                x.get("message").and_then(|v| v.as_str()),
            )
        });
    out.push((
        start,
        XRaySegmentRow {
            depth,
            name: doc.get("name").and_then(|v| v.as_str()).unwrap_or("?").to_string(),
            origin: doc.get("origin").and_then(|v| v.as_str()).map(str::to_string),
            offset: start,
            duration: end.map(|e| e - start),
            fault: doc.get("fault").and_then(|v| v.as_bool()).unwrap_or(false),
            error: doc.get("error").and_then(|v| v.as_bool()).unwrap_or(false),
            throttle: doc.get("throttle").and_then(|v| v.as_bool()).unwrap_or(false),
            http_status: doc
                .get("http")
                .and_then(|h| h.get("response"))
                .and_then(|r| r.get("status"))
                .and_then(|s| s.as_i64()),
            exception,
        },
    ));
    if let Some(subs) = doc.get("subsegments").and_then(|s| s.as_array()) {
        let mut subs: Vec<&serde_json::Value> = subs.iter().collect();
        subs.sort_by(|a, b| {
            let s = |v: &serde_json::Value| v.get("start_time").and_then(|t| t.as_f64()).unwrap_or(0.0);
            s(a).partial_cmp(&s(b)).unwrap_or(std::cmp::Ordering::Equal)
        });
        for sub in subs {
            flatten_segment(sub, depth + 1, out);
        }
    }
}

/// Parse `BatchGetTraces` output into the Segments rows + raw JSON.
pub fn trace_detail_from_documents(docs: &[String], limit_exceeded: bool) -> XRayTraceDetail {
    let parsed: Vec<serde_json::Value> = docs
        .iter()
        .filter_map(|d| serde_json::from_str(d).ok())
        .collect();
    // Top-level segments in start order; subsegments stay under their parent.
    let mut tops: Vec<&serde_json::Value> = parsed.iter().collect();
    tops.sort_by(|a, b| {
        let s = |v: &serde_json::Value| v.get("start_time").and_then(|t| t.as_f64()).unwrap_or(0.0);
        s(a).partial_cmp(&s(b)).unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut flat: Vec<(f64, XRaySegmentRow)> = Vec::new();
    for t in tops {
        flatten_segment(t, 0, &mut flat);
    }
    let t0 = flat.iter().map(|(s, _)| *s).fold(f64::INFINITY, f64::min);
    let rows = flat
        .into_iter()
        .map(|(_, mut r)| {
            r.offset = if t0.is_finite() { r.offset - t0 } else { 0.0 };
            r
        })
        .collect();
    let raw = serde_json::to_string_pretty(&serde_json::Value::Array(parsed)).unwrap_or_default();
    XRayTraceDetail {
        rows,
        raw,
        limit_exceeded,
    }
}

pub async fn fetch_trace_detail(client: XRayClient, trace_id: String) -> Result<XRayTraceDetail> {
    let mut docs: Vec<String> = Vec::new();
    let mut limit_exceeded = false;
    let mut pages = client.batch_get_traces().trace_ids(trace_id).into_paginator().send();
    while let Some(page) = pages.next().await {
        let page = page.map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
        for t in page.traces() {
            limit_exceeded |= t.limit_exceeded().unwrap_or(false);
            docs.extend(t.segments().iter().filter_map(|s| s.document()).map(str::to_string));
        }
    }
    Ok(trace_detail_from_documents(&docs, limit_exceeded))
}

// ── Group ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct XRayGroup {
    pub name: String,
    pub arn: String,
    pub filter: Option<String>,
    pub insights: Option<bool>,
    pub notifications: Option<bool>,
    tags: HashMap<String, String>,
}

impl XRayGroup {
    pub fn from_sdk(g: &aws_sdk_xray::types::GroupSummary) -> Self {
        let ic = g.insights_configuration();
        Self {
            name: g.group_name().unwrap_or_default().to_string(),
            arn: g.group_arn().unwrap_or_default().to_string(),
            filter: g.filter_expression().map(str::to_string),
            insights: ic.and_then(|c| c.insights_enabled()),
            notifications: ic.and_then(|c| c.notifications_enabled()),
            tags: HashMap::new(),
        }
    }
}

impl Resource for XRayGroup {
    fn cli_command(&self) -> Option<String> {
        Some(format!("aws xray get-group --group-arn {}", shell_quote(&self.arn)))
    }

    fn id(&self) -> &str {
        &self.arn
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn resource_type(&self) -> &str {
        "X-Ray Group"
    }

    fn state(&self) -> ResourceState {
        ResourceState::stateless()
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!("{} {} xray group", self.name, self.filter.as_deref().unwrap_or(""))
    }

    fn details(&self) -> Vec<(String, String)> {
        let yn = |b: Option<bool>| match b {
            Some(true) => "✓ enabled".to_string(),
            Some(false) => "disabled".to_string(),
            None => "—".to_string(),
        };
        vec![
            ("Group".to_string(), self.name.clone()),
            (
                "Filter Expression".to_string(),
                self.filter.clone().unwrap_or_else(|| "(all traces)".to_string()),
            ),
            ("Insights".to_string(), yn(self.insights)),
            ("Insight Notifications".to_string(), yn(self.notifications)),
            ("ARN".to_string(), self.arn.clone()),
        ]
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── Sampling rule ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct XRaySamplingRule {
    pub name: String,
    pub arn: String,
    pub priority: i32,
    pub fixed_rate: f64,
    pub reservoir: i32,
    pub service_name: String,
    pub service_type: String,
    pub host: String,
    pub method: String,
    pub url_path: String,
    pub resource_arn: String,
    pub attributes: Vec<(String, String)>,
    pub modified: Option<String>,
    tags: HashMap<String, String>,
}

impl XRaySamplingRule {
    pub fn from_sdk(r: &aws_sdk_xray::types::SamplingRuleRecord) -> Option<Self> {
        let rule = r.sampling_rule()?;
        let mut attributes: Vec<(String, String)> = rule
            .attributes()
            .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default();
        attributes.sort();
        Some(Self {
            name: rule.rule_name().unwrap_or_default().to_string(),
            arn: rule.rule_arn().unwrap_or_default().to_string(),
            priority: rule.priority(),
            fixed_rate: rule.fixed_rate(),
            reservoir: rule.reservoir_size(),
            service_name: rule.service_name().to_string(),
            service_type: rule.service_type().to_string(),
            host: rule.host().to_string(),
            method: rule.http_method().to_string(),
            url_path: rule.url_path().to_string(),
            resource_arn: rule.resource_arn().to_string(),
            attributes,
            modified: r.modified_at().map(fmt_dt),
            tags: HashMap::new(),
        })
    }

    /// A rule that samples nothing — the usual "why no traces?" answer.
    pub fn samples_nothing(&self) -> bool {
        self.fixed_rate == 0.0 && self.reservoir == 0
    }
}

impl Resource for XRaySamplingRule {
    fn cli_command(&self) -> Option<String> {
        Some("aws xray get-sampling-rules".to_string())
    }

    fn id(&self) -> &str {
        &self.arn
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn resource_type(&self) -> &str {
        "X-Ray Sampling Rule"
    }

    fn state(&self) -> ResourceState {
        if self.samples_nothing() {
            ResourceState::Stopped
        } else {
            ResourceState::stateless()
        }
    }

    fn state_label(&self) -> String {
        if self.samples_nothing() {
            "samples nothing".to_string()
        } else {
            String::new()
        }
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!(
            "{} {} {} {} xray sampling rule",
            self.name, self.service_name, self.host, self.url_path
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        let mut rows = vec![
            ("Rule".to_string(), self.name.clone()),
            ("Priority".to_string(), format!("{} (lower wins)", self.priority)),
            (
                "Rate".to_string(),
                format!(
                    "{} req/s reservoir, then {:.1}%{}",
                    self.reservoir,
                    self.fixed_rate * 100.0,
                    if self.samples_nothing() { "  ⚠ samples nothing" } else { "" }
                ),
            ),
            ("Service Name".to_string(), self.service_name.clone()),
            ("Service Type".to_string(), self.service_type.clone()),
            ("Host".to_string(), self.host.clone()),
            ("HTTP Method".to_string(), self.method.clone()),
            ("URL Path".to_string(), self.url_path.clone()),
            ("Resource ARN".to_string(), self.resource_arn.clone()),
        ];
        for (k, v) in &self.attributes {
            rows.push((format!("Attribute {k}"), v.clone()));
        }
        if let Some(m) = &self.modified {
            rows.push(("Modified".to_string(), m.clone()));
        }
        rows.push(("ARN".to_string(), self.arn.clone()));
        rows
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_sdk_xray::types::{Edge, EdgeStatistics, FaultStatistics, HistogramEntry, Service, ServiceStatistics};

    #[test]
    fn histogram_percentiles() {
        let h: Vec<HistogramEntry> = [(0.1, 50), (0.2, 40), (2.0, 10)]
            .iter()
            .map(|(v, c)| HistogramEntry::builder().value(*v).count(*c).build())
            .collect();
        assert_eq!(histogram_percentile(&h, 0.5), Some(0.1));
        assert_eq!(histogram_percentile(&h, 0.9), Some(0.2));
        assert_eq!(histogram_percentile(&h, 0.99), Some(2.0));
        assert_eq!(histogram_percentile(&[], 0.5), None);
    }

    #[test]
    fn graph_resolves_edges_both_ways_and_sorts_faulty_first() {
        let api = Service::builder()
            .reference_id(0)
            .name("orders-api")
            .r#type("AWS::ApiGateway::Stage")
            .summary_statistics(ServiceStatistics::builder().total_count(100).ok_count(100).build())
            .edges(
                Edge::builder()
                    .reference_id(1)
                    .summary_statistics(
                        EdgeStatistics::builder()
                            .total_count(100)
                            .fault_statistics(FaultStatistics::builder().total_count(5).build())
                            .build(),
                    )
                    .build(),
            )
            .build();
        let lambda = Service::builder()
            .reference_id(1)
            .name("orders-fn")
            .r#type("AWS::Lambda::Function")
            .summary_statistics(
                ServiceStatistics::builder()
                    .total_count(100)
                    .fault_statistics(FaultStatistics::builder().total_count(5).build())
                    .build(),
            )
            .build();
        let nodes = XRayNode::from_graph(&[api, lambda], XRayWindow::OneHour);
        assert_eq!(nodes[0].name, "orders-fn", "faulty node first");
        assert_eq!(nodes[0].state_label(), "5.0% faults");
        assert_eq!(nodes[0].upstream[0].name, "orders-api");
        assert_eq!(nodes[1].downstream[0].name, "orders-fn");
        assert_eq!(nodes[1].downstream[0].stats.faults, 5);
        assert_eq!(nodes[0].neboto_target().as_deref(), Some("@lambda orders-fn"));
    }

    #[test]
    fn segments_flatten_depth_first_with_offsets() {
        let docs = vec![
            r#"{"id":"a","name":"orders-api","start_time":100.0,"end_time":101.5,
               "subsegments":[{"id":"b","name":"DynamoDB","start_time":100.2,"end_time":100.4,
                 "fault":true,"cause":{"exceptions":[{"type":"ProvisionedThroughputExceededException","message":"slow down"}]}}]}"#
                .to_string(),
        ];
        let d = trace_detail_from_documents(&docs, false);
        assert_eq!(d.rows.len(), 2);
        assert_eq!(d.rows[0].depth, 0);
        assert_eq!(d.rows[1].depth, 1);
        assert!((d.rows[1].offset - 0.2).abs() < 1e-9);
        assert!(d.rows[1].fault);
        assert_eq!(
            d.rows[1].exception.as_deref(),
            Some("ProvisionedThroughputExceededException: slow down")
        );
        assert!(d.raw.contains("orders-api"));
    }

    #[test]
    fn window_steps_saturate() {
        assert_eq!(XRayWindow::SixHours.wider(), XRayWindow::SixHours);
        assert_eq!(XRayWindow::FiveMinutes.narrower(), XRayWindow::FiveMinutes);
        assert_eq!(XRayWindow::OneHour.wider(), XRayWindow::SixHours);
    }
}
