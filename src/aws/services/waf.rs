use crate::aws::client::AwsClients;
use crate::aws::resource::{Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use aws_sdk_cloudwatch::primitives::DateTime as CwDateTime;
use aws_sdk_wafv2::Client as WafClient;
use aws_sdk_wafv2::types::Scope;
use std::any::Any;
use std::collections::HashMap;
use tokio::sync::mpsc;

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum WafScope {
    CloudFront,
    Regional,
}

impl WafScope {
    pub fn sdk_scope(&self) -> Scope {
        match self {
            WafScope::CloudFront => Scope::Cloudfront,
            WafScope::Regional => Scope::Regional,
        }
    }
    pub fn label(&self) -> &'static str {
        match self {
            WafScope::CloudFront => "CLOUDFRONT",
            WafScope::Regional => "REGIONAL",
        }
    }
}

pub struct WafService {
    client: WafClient,
    scope: WafScope,
}

impl WafService {
    pub fn new(aws_clients: &AwsClients, scope: WafScope) -> Self {
        let client = match scope {
            WafScope::CloudFront => aws_clients.wafv2_cloudfront_client(),
            WafScope::Regional => aws_clients.wafv2_client(),
        };
        Self { client, scope }
    }
}

#[async_trait]
impl AwsService for WafService {
    fn service_type(&self) -> ServiceType {
        ServiceType::Waf
    }

    fn name(&self) -> &str {
        "WAF"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        self.list_resources_streaming(tx, ServiceType::Waf).await?;
        Ok(vec![])
    }

    async fn list_resources_streaming(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        let mut total = 0usize;
        let scope = self.scope.sdk_scope();
        let scope_label = self.scope.label().to_string();

        // Web ACLs
        let mut acls = Vec::new();
        let mut marker: Option<String> = None;
        loop {
            let mut req = self.client.list_web_acls().scope(scope.clone());
            if let Some(m) = &marker {
                req = req.next_marker(m);
            }
            match req.send().await {
                Ok(resp) => {
                    for s in resp.web_acls() {
                        acls.push(Box::new(WafWebAcl {
                            id: s.id().unwrap_or_default().to_string(),
                            name: s.name().unwrap_or_default().to_string(),
                            arn: s.arn().unwrap_or_default().to_string(),
                            description: s.description().unwrap_or_default().to_string(),
                            scope: scope_label.clone(),
                        }) as Box<dyn Resource>);
                    }
                    marker = crate::aws::pagination::next_page_token(resp.next_marker(), &marker);
                    if marker.is_none() { break; }
                }
                Err(e) => {
                    let _ = event_tx.send(Event::ResourceLoadError {
                        service: service_type,
                        error: format!("Failed to list Web ACLs: {}", e),
                    });
                    return Ok(());
                }
            }
        }
        if !acls.is_empty() {
            total += acls.len();
            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                service: service_type,
                resources: acls,
                progress: LoadProgress { loaded_count: total, total_count: None, status_message: Some("Loading IP sets…".to_string()) },
            });
        }

        // IP Sets
        let mut ipsets = Vec::new();
        marker = None;
        loop {
            let mut req = self.client.list_ip_sets().scope(scope.clone());
            if let Some(m) = &marker {
                req = req.next_marker(m);
            }
            match req.send().await {
                Ok(resp) => {
                    for s in resp.ip_sets() {
                        ipsets.push(Box::new(WafIpSet {
                            id: s.id().unwrap_or_default().to_string(),
                            name: s.name().unwrap_or_default().to_string(),
                            arn: s.arn().unwrap_or_default().to_string(),
                            description: s.description().unwrap_or_default().to_string(),
                            scope: scope_label.clone(),
                        }) as Box<dyn Resource>);
                    }
                    marker = crate::aws::pagination::next_page_token(resp.next_marker(), &marker);
                    if marker.is_none() { break; }
                }
                Err(_) => break,
            }
        }
        if !ipsets.is_empty() {
            total += ipsets.len();
            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                service: service_type,
                resources: ipsets,
                progress: LoadProgress { loaded_count: total, total_count: None, status_message: Some("Loading rule groups…".to_string()) },
            });
        }

        // Rule Groups
        let mut rulegroups = Vec::new();
        marker = None;
        loop {
            let mut req = self.client.list_rule_groups().scope(scope.clone());
            if let Some(m) = &marker {
                req = req.next_marker(m);
            }
            match req.send().await {
                Ok(resp) => {
                    for s in resp.rule_groups() {
                        rulegroups.push(Box::new(WafRuleGroup {
                            id: s.id().unwrap_or_default().to_string(),
                            name: s.name().unwrap_or_default().to_string(),
                            arn: s.arn().unwrap_or_default().to_string(),
                            description: s.description().unwrap_or_default().to_string(),
                            scope: scope_label.clone(),
                        }) as Box<dyn Resource>);
                    }
                    marker = crate::aws::pagination::next_page_token(resp.next_marker(), &marker);
                    if marker.is_none() { break; }
                }
                Err(_) => break,
            }
        }
        if !rulegroups.is_empty() {
            total += rulegroups.len();
            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                service: service_type,
                resources: rulegroups,
                progress: LoadProgress { loaded_count: total, total_count: None, status_message: None },
            });
        }

        let _ = event_tx.send(Event::ResourcesFullyLoaded { service: service_type, total_count: total });
        Ok(())
    }

    async fn get_resource_details(&self, _id: &str) -> Result<Box<dyn Resource>> {
        Err(crate::error::Error::NotImplemented)
    }
}

// ── WafWebAcl ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct WafWebAcl {
    pub id: String,
    pub name: String,
    pub arn: String,
    pub description: String,
    pub scope: String,
}

crate::sections! {
    pub enum WafWebAclDetailSection,
    pub static WAF_WEB_ACL_SECTIONS = [
        Details "Details" => crate::app::App::trigger_waf_web_acl_detail_load,
        Rules "Rules" => crate::app::App::trigger_waf_web_acl_detail_load,
        Associated "Associated" => crate::app::App::trigger_waf_web_acl_detail_load,
        Logging "Logging" => crate::app::App::trigger_waf_web_acl_detail_load,
        Traffic "Traffic" => crate::app::App::hook_waf_web_acl_traffic,
        Insights "Insights" => crate::app::App::hook_waf_web_acl_insights,
    ]
}

impl Resource for WafWebAcl {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&WAF_WEB_ACL_SECTIONS)
    }
    fn id(&self) -> &str { &self.id }
    fn name(&self) -> &str { &self.name }
    fn resource_type(&self) -> &str { "WAF Web ACL" }
    fn state(&self) -> ResourceState { ResourceState::Available }
    fn tags(&self) -> &HashMap<String, String> {
        static EMPTY: std::sync::LazyLock<HashMap<String, String>> = std::sync::LazyLock::new(HashMap::new);
        &EMPTY
    }
    fn search_text(&self) -> String {
        format!("{} {} {} {}", self.name, self.id, self.description, self.scope)
    }
    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Name".to_string(), self.name.clone()),
            ("ID".to_string(), self.id.clone()),
            ("ARN".to_string(), self.arn.clone()),
            ("Scope".to_string(), self.scope.clone()),
            ("Description".to_string(), self.description.clone()),
        ]
    }
    fn console_url(&self, region: &str) -> Option<String> {
        let r = if self.scope == "CLOUDFRONT" { "us-east-1" } else { region };
        Some(format!(
            "https://{}.console.aws.amazon.com/wafv2/homev2/web-acl/{}/{}/overview?region={}",
            r, self.name, self.id, r
        ))
    }
    fn clone_box(&self) -> Box<dyn Resource> { Box::new(self.clone()) }
    fn as_any(&self) -> &dyn Any { self }
}

// ── WafIpSet ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct WafIpSet {
    pub id: String,
    pub name: String,
    pub arn: String,
    pub description: String,
    pub scope: String,
}

crate::sections! {
    pub enum WafIpSetDetailSection,
    pub static WAF_IP_SET_SECTIONS = [
        Details "Details" => crate::app::App::trigger_waf_ip_set_load,
        Addresses "Addresses" => crate::app::App::trigger_waf_ip_set_load,
        Tags "Tags" => crate::app::App::trigger_waf_ip_set_load,
    ]
}

impl Resource for WafIpSet {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&WAF_IP_SET_SECTIONS)
    }
    fn id(&self) -> &str { &self.id }
    fn name(&self) -> &str { &self.name }
    fn resource_type(&self) -> &str { "WAF IP Set" }
    fn state(&self) -> ResourceState { ResourceState::Available }
    fn tags(&self) -> &HashMap<String, String> {
        static EMPTY: std::sync::LazyLock<HashMap<String, String>> = std::sync::LazyLock::new(HashMap::new);
        &EMPTY
    }
    fn search_text(&self) -> String {
        format!("{} {} {}", self.name, self.id, self.description)
    }
    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Name".to_string(), self.name.clone()),
            ("ID".to_string(), self.id.clone()),
            ("ARN".to_string(), self.arn.clone()),
            ("Scope".to_string(), self.scope.clone()),
        ]
    }
    fn console_url(&self, region: &str) -> Option<String> {
        let r = if self.scope == "CLOUDFRONT" { "us-east-1" } else { region };
        Some(format!(
            "https://{}.console.aws.amazon.com/wafv2/homev2/ip-sets/{}/{}/details?region={}",
            r, self.name, self.id, r
        ))
    }
    fn clone_box(&self) -> Box<dyn Resource> { Box::new(self.clone()) }
    fn as_any(&self) -> &dyn Any { self }
}

// ── WafRuleGroup ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct WafRuleGroup {
    pub id: String,
    pub name: String,
    pub arn: String,
    pub description: String,
    pub scope: String,
}

crate::sections! {
    pub enum WafRuleGroupDetailSection,
    pub static WAF_RULE_GROUP_SECTIONS = [
        Details "Details" => crate::app::App::trigger_waf_rule_group_load,
        Rules "Rules" => crate::app::App::trigger_waf_rule_group_load,
        Tags "Tags" => crate::app::App::trigger_waf_rule_group_load,
    ]
}

impl Resource for WafRuleGroup {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&WAF_RULE_GROUP_SECTIONS)
    }
    fn id(&self) -> &str { &self.id }
    fn name(&self) -> &str { &self.name }
    fn resource_type(&self) -> &str { "WAF Rule Group" }
    fn state(&self) -> ResourceState { ResourceState::Available }
    fn tags(&self) -> &HashMap<String, String> {
        static EMPTY: std::sync::LazyLock<HashMap<String, String>> = std::sync::LazyLock::new(HashMap::new);
        &EMPTY
    }
    fn search_text(&self) -> String {
        format!("{} {} {}", self.name, self.id, self.description)
    }
    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Name".to_string(), self.name.clone()),
            ("ID".to_string(), self.id.clone()),
            ("ARN".to_string(), self.arn.clone()),
            ("Scope".to_string(), self.scope.clone()),
        ]
    }
    fn console_url(&self, region: &str) -> Option<String> {
        let r = if self.scope == "CLOUDFRONT" { "us-east-1" } else { region };
        Some(format!(
            "https://{}.console.aws.amazon.com/wafv2/homev2/rule-groups/{}/{}/details?region={}",
            r, self.name, self.id, r
        ))
    }
    fn clone_box(&self) -> Box<dyn Resource> { Box::new(self.clone()) }
    fn as_any(&self) -> &dyn Any { self }
}

// ── WAF Metrics ───────────────────────────────────────────────────────────────

use crate::aws::services::ec2::MetricsTimeRange;

/// Range totals for one Rule-dimension series (a top-level rule, a managed
/// rule-group rollup, or `Default_Action`).
#[derive(Debug, Clone, Default)]
pub struct WafRuleTraffic {
    pub name: String,
    pub allowed: f64,
    pub blocked: f64,
    pub counted: f64,
    pub captcha: f64,
    pub challenge: f64,
}

impl WafRuleTraffic {
    pub fn total(&self) -> f64 {
        self.allowed + self.blocked + self.counted + self.captcha + self.challenge
    }
}

#[derive(Debug, Clone)]
pub struct WafMetricsData {
    pub time_range: MetricsTimeRange,
    pub allowed: Vec<(f64, f64)>,
    pub blocked: Vec<(f64, f64)>,
    pub counted: Vec<(f64, f64)>,
    pub captcha: Vec<(f64, f64)>,
    pub challenge: Vec<(f64, f64)>,
    /// Per-rule range totals, sorted by total desc (Rule=ALL excluded).
    pub rule_traffic: Vec<WafRuleTraffic>,
    /// Per-label range totals (managed rule groups emit these — bot
    /// categories, attack signals), sorted by total desc.
    pub label_traffic: Vec<WafRuleTraffic>,
    /// Set when the per-rule/label breakdown couldn't be fetched (e.g. the
    /// policy lacks `cloudwatch:GetMetricData`) — the charts still render.
    pub rule_note: Option<String>,
    pub x_max: f64,
}

#[derive(Debug, Clone)]
pub enum WafMetricsState {
    Loading,
    Loaded(Box<WafMetricsData>),
    Error(String),
}

const WAF_METRICS: [&str; 5] = [
    "AllowedRequests",
    "BlockedRequests",
    "CountedRequests",
    "CaptchaRequests",
    "ChallengeRequests",
];

/// One `GetMetricStatistics` Sum series for a `WebACL` + `Rule=ALL` (+
/// `Region` for REGIONAL scope) dimension set. `region_value` is `None` for
/// CLOUDFRONT-scope ACLs — they publish with no `Region` dimension at all,
/// and GetMetricStatistics only matches the exact dimension set.
async fn fetch_stat_series(
    cw_client: &aws_sdk_cloudwatch::Client,
    metric: &str,
    web_acl_dim: &str,
    region_value: Option<&str>,
    start_secs: i64,
    now_secs: i64,
    period: i32,
) -> std::result::Result<Vec<(f64, f64)>, String> {
    let mut req = cw_client
        .get_metric_statistics()
        .namespace("AWS/WAFV2")
        .metric_name(metric)
        .dimensions(
            aws_sdk_cloudwatch::types::Dimension::builder()
                .name("WebACL")
                .value(web_acl_dim)
                .build(),
        )
        .dimensions(
            aws_sdk_cloudwatch::types::Dimension::builder()
                .name("Rule")
                .value("ALL")
                .build(),
        );
    if let Some(rv) = region_value {
        req = req.dimensions(
            aws_sdk_cloudwatch::types::Dimension::builder()
                .name("Region")
                .value(rv)
                .build(),
        );
    }
    let resp = req
        .start_time(CwDateTime::from_secs(start_secs))
        .end_time(CwDateTime::from_secs(now_secs))
        .period(period)
        .set_statistics(Some(vec![aws_sdk_cloudwatch::types::Statistic::Sum]))
        .send()
        .await
        .map_err(|e| crate::error::sdk_error_message(&e))?;
    let mut pts: Vec<(f64, f64)> = resp
        .datapoints()
        .iter()
        .filter_map(|dp| {
            let t = dp.timestamp()?.secs() as f64 - start_secs as f64;
            let v = dp.sum()?;
            Some((t, v))
        })
        .collect();
    pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    Ok(pts)
}

/// Fold `(query id, series label) → sum` entries with a given id prefix into
/// sorted per-key traffic rows. The digit after the prefix indexes
/// [`WAF_METRICS`].
fn fold_breakdown(
    sums: &HashMap<(String, String), f64>,
    prefix: char,
) -> Vec<WafRuleTraffic> {
    let mut by_key: HashMap<String, WafRuleTraffic> = HashMap::new();
    for ((id, label), sum) in sums {
        if !id.starts_with(prefix) || label == "ALL" || *sum <= 0.0 {
            continue;
        }
        let e = by_key.entry(label.clone()).or_insert_with(|| WafRuleTraffic {
            name: label.clone(),
            ..Default::default()
        });
        match &id[1..] {
            "0" => e.allowed += sum,
            "1" => e.blocked += sum,
            "2" => e.counted += sum,
            "3" => e.captcha += sum,
            "4" => e.challenge += sum,
            _ => {}
        }
    }
    let mut rows: Vec<WafRuleTraffic> = by_key.into_values().collect();
    rows.sort_by(|a, b| {
        b.total()
            .partial_cmp(&a.total())
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    rows
}

/// Per-rule and per-label range totals via one paginated `GetMetricData` —
/// a SEARCH per metric over the `Rule` schema (one series per rule: top-level
/// rules, managed rule-group rollups, `Default_Action`) and one over the
/// label schema (bot categories, attack signals), without needing the metric
/// names up front. Best-effort: callers turn a failure into a note, never a
/// fetch error.
async fn fetch_rule_breakdown(
    cw_client: &aws_sdk_cloudwatch::Client,
    web_acl_dim: &str,
    region_value: Option<&str>,
    start_secs: i64,
    now_secs: i64,
    period: i32,
) -> std::result::Result<(Vec<WafRuleTraffic>, Vec<WafRuleTraffic>), String> {
    // The SEARCH schema names the exact dimension set — CLOUDFRONT-scope ACLs
    // publish without a Region dimension, so the schema must omit it too.
    let (rule_schema, label_schema, region_filter) = match region_value {
        Some(rv) => (
            "{AWS/WAFV2,Region,Rule,WebACL}",
            "{AWS/WAFV2,LabelName,LabelNamespace,Region,WebACL}",
            format!(" Region=\"{}\"", rv),
        ),
        None => (
            "{AWS/WAFV2,Rule,WebACL}",
            "{AWS/WAFV2,LabelName,LabelNamespace,WebACL}",
            String::new(),
        ),
    };
    let mut queries = Vec::new();
    for (i, metric) in WAF_METRICS.iter().enumerate() {
        // The dynamic labels carry the Rule / LabelName value back so results
        // group.
        let expr = format!(
            "SEARCH('{} MetricName=\"{}\" WebACL=\"{}\"{}', 'Sum', {})",
            rule_schema, metric, web_acl_dim, region_filter, period
        );
        queries.push(
            aws_sdk_cloudwatch::types::MetricDataQuery::builder()
                .id(format!("e{}", i))
                .expression(expr)
                .label("${PROP('Dim.Rule')}")
                .build(),
        );
        let label_expr = format!(
            "SEARCH('{} MetricName=\"{}\" WebACL=\"{}\"{}', 'Sum', {})",
            label_schema, metric, web_acl_dim, region_filter, period
        );
        queries.push(
            aws_sdk_cloudwatch::types::MetricDataQuery::builder()
                .id(format!("l{}", i))
                .expression(label_expr)
                .label("${PROP('Dim.LabelName')}")
                .build(),
        );
    }

    // A busy ACL returns many series (and a series can span result pages), so
    // accumulate across the paginated responses.
    let mut rule_sums: HashMap<(String, String), f64> = HashMap::new();
    let mut next: Option<String> = None;
    loop {
        let mut req = cw_client
            .get_metric_data()
            .set_metric_data_queries(Some(queries.clone()))
            .start_time(CwDateTime::from_secs(start_secs))
            .end_time(CwDateTime::from_secs(now_secs));
        if let Some(t) = &next {
            req = req.next_token(t);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| crate::error::sdk_error_message(&e))?;
        for r in resp.metric_data_results() {
            let id = r.id().unwrap_or_default().to_string();
            let label = r.label().unwrap_or_default().to_string();
            let sum: f64 = r.values().iter().sum();
            *rule_sums.entry((id, label)).or_insert(0.0) += sum;
        }
        next = crate::aws::pagination::next_page_token(resp.next_token(), &next);
        if next.is_none() {
            break;
        }
    }

    Ok((
        fold_breakdown(&rule_sums, 'e'),
        fold_breakdown(&rule_sums, 'l'),
    ))
}

pub async fn fetch_waf_metrics(
    waf_client: WafClient,
    cw_client: aws_sdk_cloudwatch::Client,
    web_acl_name: String,
    web_acl_id: String,
    scope: WafScope,
    region: String,
    time_range: MetricsTimeRange,
) -> Result<WafMetricsData> {
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let start_secs = now_secs - time_range.duration_secs();
    let period = time_range.period_secs();

    // WAFv2 metrics use dimensions WebACL + Rule, plus Region for REGIONAL
    // scope only — the docs' dimension table says Region is "required for all
    // protected resource types except for Amazon CloudFront distributions",
    // so a CLOUDFRONT query must omit it entirely (Region="Global" matches
    // nothing; that shipped once as permanently-empty CloudFront charts).
    let region_value = match scope {
        WafScope::CloudFront => None,
        WafScope::Regional => Some(region),
    };

    // Candidate WebACL dimension values: the ACL name first, then (when it
    // differs) the VisibilityConfig metric name — accounts publish under one
    // or the other, so try both instead of guessing. GetWebACL is best-effort;
    // without it the name alone still matches the common case.
    let mut candidates = vec![web_acl_name.clone()];
    if let Ok(resp) = waf_client
        .get_web_acl()
        .name(&web_acl_name)
        .id(&web_acl_id)
        .scope(scope.sdk_scope())
        .send()
        .await
    {
        if let Some(mn) = resp
            .web_acl()
            .and_then(|a| a.visibility_config())
            .map(|vc| vc.metric_name().to_string())
        {
            if mn != web_acl_name {
                candidates.push(mn);
            }
        }
    }

    // Charts ride GetMetricStatistics (the call every metrics pane uses, so
    // no extra IAM requirement), trying each candidate until one has data.
    let mut charts: [Vec<(f64, f64)>; 5] = Default::default();
    let mut first_err: Option<String> = None;
    let mut web_acl_dim = candidates[0].clone();
    for cand in &candidates {
        let (a, b, c, d, e) = tokio::join!(
            fetch_stat_series(&cw_client, WAF_METRICS[0], cand, region_value.as_deref(), start_secs, now_secs, period),
            fetch_stat_series(&cw_client, WAF_METRICS[1], cand, region_value.as_deref(), start_secs, now_secs, period),
            fetch_stat_series(&cw_client, WAF_METRICS[2], cand, region_value.as_deref(), start_secs, now_secs, period),
            fetch_stat_series(&cw_client, WAF_METRICS[3], cand, region_value.as_deref(), start_secs, now_secs, period),
            fetch_stat_series(&cw_client, WAF_METRICS[4], cand, region_value.as_deref(), start_secs, now_secs, period),
        );
        let mut any = false;
        for (i, r) in [a, b, c, d, e].into_iter().enumerate() {
            match r {
                Ok(pts) => {
                    if !pts.is_empty() {
                        any = true;
                    }
                    charts[i] = pts;
                }
                Err(e) => {
                    if first_err.is_none() {
                        first_err = Some(e);
                    }
                }
            }
        }
        if any {
            web_acl_dim = cand.clone();
            break;
        }
    }

    // Per-rule/label breakdown is best-effort — GetMetricData is a newer IAM
    // requirement than the charts, so a denial degrades to a note.
    let (rule_traffic, label_traffic, rule_note) = match fetch_rule_breakdown(
        &cw_client,
        &web_acl_dim,
        region_value.as_deref(),
        start_secs,
        now_secs,
        period,
    )
    .await
    {
        Ok((rt, lt)) => (rt, lt, None),
        Err(e) => (
            Vec::new(),
            Vec::new(),
            Some(format!(
                "Per-rule breakdown unavailable (needs cloudwatch:GetMetricData): {}",
                e
            )),
        ),
    };

    // Only fail the whole fetch when nothing at all came back and a call
    // errored — otherwise partial data (with the note) beats an error page.
    if charts.iter().all(|c| c.is_empty()) && rule_traffic.is_empty() && label_traffic.is_empty() {
        if let Some(e) = first_err {
            return Err(crate::error::Error::AwsSdk(e));
        }
    }

    let [allowed, blocked, counted, captcha, challenge] = charts;
    Ok(WafMetricsData {
        time_range,
        allowed,
        blocked,
        counted,
        captcha,
        challenge,
        rule_traffic,
        label_traffic,
        rule_note,
        x_max: time_range.duration_secs() as f64,
    })
}

// ── Sampled Requests ──────────────────────────────────────────────────────────

pub async fn fetch_sampled_requests(
    client: WafClient,
    web_acl_arn: String,
    web_acl_name: String,
    web_acl_id: String,
    scope: WafScope,
) -> Result<String> {
    use aws_sdk_wafv2::primitives::DateTime as WafDateTime;

    // First, get the Web ACL's metric name via GetWebACL
    let acl_resp = client
        .get_web_acl()
        .name(&web_acl_name)
        .id(&web_acl_id)
        .scope(scope.sdk_scope())
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;

    let metric_name = acl_resp
        .web_acl()
        .and_then(|acl| acl.visibility_config())
        .map(|vc| vc.metric_name().to_string())
        .unwrap_or_else(|| web_acl_name.clone());

    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let start_secs = now_secs - 3 * 3600; // last 3 hours

    let time_window = aws_sdk_wafv2::types::TimeWindow::builder()
        .start_time(WafDateTime::from_secs(start_secs))
        .end_time(WafDateTime::from_secs(now_secs))
        .build()
        .map_err(|e| crate::error::Error::AwsSdk(e.to_string()))?;

    let resp = client
        .get_sampled_requests()
        .web_acl_arn(&web_acl_arn)
        .rule_metric_name(&metric_name)
        .scope(scope.sdk_scope())
        .time_window(time_window)
        .max_items(500)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;

    let mut lines = Vec::new();
    let total = resp.population_size();
    let samples = resp.sampled_requests();

    lines.push(format!("WAF Sampled Requests (last 3 hours)"));
    lines.push(format!("Total population: {}  |  Samples shown: {}", total, samples.len()));
    lines.push(String::new());
    lines.push(format!(
        "{:<20} {:<6} {:<16} {:<3} {:<40} {:<10} {}",
        "Timestamp", "Action", "Client IP", "CC", "URI", "Method", "Rule/Labels"
    ));
    lines.push("─".repeat(120));

    for sample in samples {
        let ts = sample
            .timestamp()
            .map(|t| {
                let s = t.secs();
                let days = s / 86400;
                let rem = s % 86400;
                let h = rem / 3600;
                let m = (rem % 3600) / 60;
                let sec = rem % 60;
                // inline ymd
                let mut yr = 1970i32;
                let mut d_rem = days;
                loop {
                    let dy = if (yr % 4 == 0 && yr % 100 != 0) || yr % 400 == 0 { 366 } else { 365 };
                    if d_rem < dy { break; }
                    d_rem -= dy;
                    yr += 1;
                }
                let leap = (yr % 4 == 0 && yr % 100 != 0) || yr % 400 == 0;
                let dm = [31i64, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
                let mut mo = 1u8;
                for &md in &dm {
                    if d_rem < md { break; }
                    d_rem -= md;
                    mo += 1;
                }
                format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", yr, mo, d_rem + 1, h, m, sec)
            })
            .unwrap_or_else(|| "—".to_string());

        let action = sample.action().unwrap_or("—");
        let req = sample.request();
        let ip = req.and_then(|r| r.client_ip()).unwrap_or("—");
        let country = req.and_then(|r| r.country()).unwrap_or("—");
        let uri = req.and_then(|r| r.uri()).unwrap_or("/");
        let method = req.and_then(|r| r.method()).unwrap_or("—");

        let labels: String = sample
            .labels()
            .iter()
            .map(|l| l.name())
            .collect::<Vec<_>>()
            .join(", ");

        let rule_info = if let Some(rule) = sample.rule_name_within_rule_group() {
            if labels.is_empty() {
                rule.to_string()
            } else {
                format!("{} [{}]", rule, labels)
            }
        } else if !labels.is_empty() {
            format!("[{}]", labels)
        } else {
            String::new()
        };

        lines.push(format!(
            "{:<20} {:<6} {:<16} {:<3} {:<40} {:<10} {}",
            ts,
            action,
            ip,
            country,
            if uri.len() > 40 { format!("{}…", &uri[..39]) } else { uri.to_string() },
            method,
            rule_info,
        ));
    }

    if samples.is_empty() {
        lines.push("  No sampled requests in the last 3 hours.".to_string());
    }

    Ok(lines.join("\n"))
}

// ── Log-tail resolution (`t` on a web ACL) ────────────────────────────────────

/// Resolve the web ACL's CloudWatch Logs destination (`aws-waf-logs-…`) for
/// the live tail. Errs with a human explanation when logging is off or goes
/// to S3/Firehose instead.
pub async fn resolve_waf_log_group(
    client: WafClient,
    web_acl_arn: String,
) -> Result<(String, Vec<String>)> {
    let resp = client
        .get_logging_configuration()
        .resource_arn(&web_acl_arn)
        .send()
        .await
        .map_err(|e| {
            let msg = crate::error::sdk_error_message(&e);
            crate::error::Error::AwsSdk(if msg.contains("onexistent") {
                "Logging is not enabled for this web ACL".to_string()
            } else {
                msg
            })
        })?;
    let dests: Vec<String> = resp
        .logging_configuration()
        .map(|l| l.log_destination_configs().to_vec())
        .unwrap_or_default();

    for d in &dests {
        // arn:aws:logs:<region>:<acct>:log-group:<name>[:*]
        if d.contains(":logs:") {
            if let Some(idx) = d.find(":log-group:") {
                let name = &d[idx + ":log-group:".len()..];
                let name = name.strip_suffix(":*").unwrap_or(name);
                if !name.is_empty() {
                    return Ok((name.to_string(), Vec::new()));
                }
            }
        }
    }
    let kind = if dests.iter().any(|d| d.starts_with("arn:aws:s3")) {
        "S3"
    } else if dests.iter().any(|d| d.contains(":firehose:")) {
        "Kinesis Data Firehose"
    } else if dests.is_empty() {
        return Err(crate::error::Error::AwsSdk(
            "Logging is not enabled for this web ACL".to_string(),
        ));
    } else {
        "a non-CloudWatch destination"
    };
    Err(crate::error::Error::AwsSdk(format!(
        "WAF logs go to {} — no CloudWatch Logs group to tail",
        kind
    )))
}

// ── Sampled-request insights (lazy Insights section) ─────────────────────────

/// Cap on `GetSampledRequests` calls per web ACL (default action + the
/// highest-priority rules) — rules beyond it are reported, not silently
/// dropped.
pub const MAX_INSIGHT_SAMPLERS: usize = 10;
const INSIGHT_TOP_N: usize = 10;

#[derive(Clone, Debug)]
pub struct WafInsightRow {
    pub key: String,
    pub count: u64,
    pub blocked: u64,
}

#[derive(Clone, Debug, Default)]
pub struct WafInsights {
    /// Sum of the samplers' population sizes (requests the samples represent).
    pub population: i64,
    /// Raw samples aggregated.
    pub samples: usize,
    /// Samplers queried (default action + rules).
    pub samplers: usize,
    /// Rules beyond [`MAX_INSIGHT_SAMPLERS`].
    pub skipped_rules: usize,
    /// Rules with sampled requests disabled in their visibility config.
    pub sampling_disabled: usize,
    pub failed_samplers: usize,
    pub actions: Vec<(String, u64)>,
    pub top_ips: Vec<WafInsightRow>,
    pub top_countries: Vec<WafInsightRow>,
    pub top_uris: Vec<WafInsightRow>,
    pub top_agents: Vec<WafInsightRow>,
    pub top_rules: Vec<WafInsightRow>,
    pub top_labels: Vec<WafInsightRow>,
}


#[derive(Default)]
struct InsightAgg(HashMap<String, (u64, u64)>);

impl InsightAgg {
    fn bump(&mut self, key: &str, weight: u64, blocked: bool) {
        let e = self.0.entry(key.to_string()).or_insert((0, 0));
        e.0 += weight;
        if blocked {
            e.1 += weight;
        }
    }
    fn top(self) -> Vec<WafInsightRow> {
        let mut rows: Vec<WafInsightRow> = self
            .0
            .into_iter()
            .map(|(key, (count, blocked))| WafInsightRow { key, count, blocked })
            .collect();
        rows.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.key.cmp(&b.key)));
        rows.truncate(INSIGHT_TOP_N);
        rows
    }
}

/// Aggregate `GetSampledRequests` across the web ACL's samplers (default
/// action + per-rule metric names) into top-N breakdowns — the API-reachable
/// slice of the console's dashboard "top" tables. The sample window is capped
/// at 3 hours by the API itself.
pub async fn fetch_waf_insights(
    client: WafClient,
    web_acl_arn: String,
    web_acl_name: String,
    web_acl_id: String,
    scope: WafScope,
) -> Result<WafInsights> {
    use aws_sdk_wafv2::primitives::DateTime as WafDateTime;

    let acl_resp = client
        .get_web_acl()
        .name(&web_acl_name)
        .id(&web_acl_id)
        .scope(scope.sdk_scope())
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
    let acl = acl_resp.web_acl();

    // Samplers: the ACL's own metric name (default-action samples) first,
    // then rules by priority. Rules with sampling disabled can't return data.
    let mut samplers: Vec<(String, String)> = Vec::new(); // (display label, metric name)
    let mut sampling_disabled = 0usize;
    if let Some(a) = acl {
        if let Some(vc) = a.visibility_config() {
            if vc.sampled_requests_enabled() {
                samplers.push(("Default action".to_string(), vc.metric_name().to_string()));
            } else {
                sampling_disabled += 1;
            }
        }
        let mut rules: Vec<_> = a.rules().to_vec();
        rules.sort_by_key(|r| r.priority());
        for r in &rules {
            match r.visibility_config() {
                Some(vc) if vc.sampled_requests_enabled() => {
                    samplers.push((r.name().to_string(), vc.metric_name().to_string()));
                }
                _ => sampling_disabled += 1,
            }
        }
    }
    let skipped_rules = samplers.len().saturating_sub(MAX_INSIGHT_SAMPLERS);
    samplers.truncate(MAX_INSIGHT_SAMPLERS);
    if samplers.is_empty() {
        return Err(crate::error::Error::AwsSdk(
            "Sampled requests are disabled for this web ACL (visibility config)".to_string(),
        ));
    }

    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let start_secs = now_secs - 3 * 3600; // API maximum lookback

    let mut population = 0i64;
    let mut samples = 0usize;
    let mut failed_samplers = 0usize;
    let mut first_err: Option<String> = None;
    let mut actions: HashMap<String, u64> = HashMap::new();
    let (mut ips, mut countries, mut uris, mut agents, mut rules_agg, mut labels) = (
        InsightAgg::default(),
        InsightAgg::default(),
        InsightAgg::default(),
        InsightAgg::default(),
        InsightAgg::default(),
        InsightAgg::default(),
    );

    for (label, metric_name) in &samplers {
        let time_window = aws_sdk_wafv2::types::TimeWindow::builder()
            .start_time(WafDateTime::from_secs(start_secs))
            .end_time(WafDateTime::from_secs(now_secs))
            .build()
            .map_err(|e| crate::error::Error::AwsSdk(e.to_string()))?;
        let resp = match client
            .get_sampled_requests()
            .web_acl_arn(&web_acl_arn)
            .rule_metric_name(metric_name)
            .scope(scope.sdk_scope())
            .time_window(time_window)
            .max_items(500)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                failed_samplers += 1;
                if first_err.is_none() {
                    first_err = Some(crate::error::sdk_error_message(&e));
                }
                continue;
            }
        };
        population += resp.population_size();
        for sample in resp.sampled_requests() {
            samples += 1;
            let weight = (sample.weight().max(1)) as u64;
            let action = sample.action().unwrap_or("—").to_string();
            let blocked = action == "BLOCK";
            *actions.entry(action).or_insert(0) += weight;

            let rule_key = match sample.rule_name_within_rule_group() {
                Some(inner) => format!("{} / {}", label, inner),
                None => label.clone(),
            };
            rules_agg.bump(&rule_key, weight, blocked);
            for l in sample.labels() {
                labels.bump(l.name(), weight, blocked);
            }
            if let Some(req) = sample.request() {
                if let Some(ip) = req.client_ip() {
                    ips.bump(ip, weight, blocked);
                }
                if let Some(cc) = req.country() {
                    countries.bump(cc, weight, blocked);
                }
                if let Some(uri) = req.uri() {
                    uris.bump(uri, weight, blocked);
                }
                if let Some(ua) = req
                    .headers()
                    .iter()
                    .find(|h| {
                        h.name()
                            .is_some_and(|n| n.eq_ignore_ascii_case("user-agent"))
                    })
                    .and_then(|h| h.value())
                {
                    agents.bump(ua, weight, blocked);
                }
            }
        }
    }

    if failed_samplers == samplers.len() {
        return Err(crate::error::Error::AwsSdk(
            first_err.unwrap_or_else(|| "all samplers failed".to_string()),
        ));
    }

    let mut actions: Vec<(String, u64)> = actions.into_iter().collect();
    actions.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    Ok(WafInsights {
        population,
        samples,
        samplers: samplers.len(),
        skipped_rules,
        sampling_disabled,
        failed_samplers,
        actions,
        top_ips: ips.top(),
        top_countries: countries.top(),
        top_uris: uris.top(),
        top_agents: agents.top(),
        top_rules: rules_agg.top(),
        top_labels: labels.top(),
    })
}

// ── WAF IP Set / Rule Group lazy detail ─────────────────────────────────────

async fn fetch_waf_tags(client: &WafClient, arn: &str) -> Vec<(String, String)> {
    let mut tags = Vec::new();
    if let Ok(resp) = client.list_tags_for_resource().resource_arn(arn).send().await {
        if let Some(info) = resp.tag_info_for_resource() {
            for t in info.tag_list() {
                tags.push((t.key().to_string(), t.value().to_string()));
            }
        }
    }
    tags
}

#[derive(Clone, Debug)]
pub struct WafIpSetDetail {
    pub ip_address_version: String,
    pub addresses: Vec<String>,
    pub tags: Vec<(String, String)>,
}

/// Fetch an IP set's addresses (`GetIPSet`) + tags, for the Addresses/Tags
/// sections.
pub async fn fetch_ip_set(
    client: WafClient,
    scope: Scope,
    name: String,
    id: String,
    arn: String,
) -> Result<WafIpSetDetail> {
    let resp = client
        .get_ip_set()
        .name(&name)
        .scope(scope)
        .id(&id)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
    let (ip_address_version, addresses) = resp
        .ip_set()
        .map(|s| {
            (
                s.ip_address_version().as_str().to_string(),
                s.addresses().to_vec(),
            )
        })
        .unwrap_or_default();
    let tags = fetch_waf_tags(&client, &arn).await;
    Ok(WafIpSetDetail {
        ip_address_version,
        addresses,
        tags,
    })
}

#[derive(Clone, Debug)]
pub struct WafRgRule {
    pub name: String,
    pub priority: i32,
    pub action: String,
}

#[derive(Clone, Debug)]
pub struct WafRuleGroupDetail {
    pub capacity: i64,
    pub rules: Vec<WafRgRule>,
    pub tags: Vec<(String, String)>,
}

fn rule_action_label(a: Option<&aws_sdk_wafv2::types::RuleAction>) -> String {
    match a {
        Some(a) if a.allow().is_some() => "Allow",
        Some(a) if a.block().is_some() => "Block",
        Some(a) if a.count().is_some() => "Count",
        Some(a) if a.captcha().is_some() => "CAPTCHA",
        Some(a) if a.challenge().is_some() => "Challenge",
        _ => "—",
    }
    .to_string()
}

/// Fetch a rule group's capacity + rules (`GetRuleGroup`) + tags.
pub async fn fetch_rule_group(
    client: WafClient,
    scope: Scope,
    name: String,
    id: String,
    arn: String,
) -> Result<WafRuleGroupDetail> {
    let resp = client
        .get_rule_group()
        .name(&name)
        .scope(scope)
        .id(&id)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
    let rg = resp.rule_group();
    let capacity = rg.map(|g| g.capacity()).unwrap_or(0);
    let mut rules: Vec<WafRgRule> = rg
        .map(|g| {
            g.rules()
                .iter()
                .map(|r| WafRgRule {
                    name: r.name().to_string(),
                    priority: r.priority(),
                    action: rule_action_label(r.action()),
                })
                .collect()
        })
        .unwrap_or_default();
    rules.sort_by_key(|r| r.priority);
    let tags = fetch_waf_tags(&client, &arn).await;
    Ok(WafRuleGroupDetail {
        capacity,
        rules,
        tags,
    })
}

// ── WAF Web ACL lazy detail (rules / associated / logging) ──────────────────

#[derive(Clone, Debug)]
pub struct WafAclRule {
    pub name: String,
    pub priority: i32,
    pub action: String,
    pub kind: String, // statement kind: Managed rule group / Rule group / IP set / Rate-based / Custom
}

#[derive(Clone, Debug)]
pub struct WafLogging {
    pub destinations: Vec<String>,
    pub redacted_fields: usize,
    pub managed_by_fm: bool,
}

#[derive(Clone, Debug)]
pub struct WafWebAclDetail {
    pub default_action: String,
    pub capacity: i64,
    pub rules: Vec<WafAclRule>,
    pub associated: Vec<String>,
    pub associated_note: Option<String>,
    pub logging: Option<WafLogging>,
}

fn acl_statement_kind(s: Option<&aws_sdk_wafv2::types::Statement>) -> String {
    match s {
        Some(s) if s.managed_rule_group_statement().is_some() => "Managed rule group",
        Some(s) if s.rule_group_reference_statement().is_some() => "Rule group",
        Some(s) if s.ip_set_reference_statement().is_some() => "IP set",
        Some(s) if s.rate_based_statement().is_some() => "Rate-based",
        Some(_) => "Custom",
        None => "—",
    }
    .to_string()
}

fn acl_rule_action(r: &aws_sdk_wafv2::types::Rule) -> String {
    if let Some(a) = r.action() {
        if a.allow().is_some() {
            return "Allow".to_string();
        }
        if a.block().is_some() {
            return "Block".to_string();
        }
        if a.count().is_some() {
            return "Count".to_string();
        }
        if a.captcha().is_some() {
            return "CAPTCHA".to_string();
        }
        if a.challenge().is_some() {
            return "Challenge".to_string();
        }
    }
    if let Some(o) = r.override_action() {
        if o.count().is_some() {
            return "override: Count".to_string();
        }
        if o.none().is_some() {
            return "override: None".to_string();
        }
    }
    "—".to_string()
}

/// Fetch a Web ACL's rules + default action (`GetWebACL`), the resources it
/// protects (`ListResourcesForWebACL`, REGIONAL only), and its logging config
/// (`GetLoggingConfiguration`).
pub async fn fetch_web_acl_detail(
    client: WafClient,
    scope: Scope,
    name: String,
    id: String,
    arn: String,
) -> Result<WafWebAclDetail> {
    let resp = client
        .get_web_acl()
        .name(&name)
        .scope(scope.clone())
        .id(&id)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
    let acl = resp.web_acl();
    let default_action = acl
        .and_then(|a| a.default_action())
        .map(|d| {
            if d.allow().is_some() {
                "Allow"
            } else if d.block().is_some() {
                "Block"
            } else {
                "—"
            }
        })
        .unwrap_or("—")
        .to_string();
    let capacity = acl.map(|a| a.capacity()).unwrap_or(0);
    let mut rules: Vec<WafAclRule> = acl
        .map(|a| {
            a.rules()
                .iter()
                .map(|r| WafAclRule {
                    name: r.name().to_string(),
                    priority: r.priority(),
                    action: acl_rule_action(r),
                    kind: acl_statement_kind(r.statement()),
                })
                .collect()
        })
        .unwrap_or_default();
    rules.sort_by_key(|r| r.priority);

    let mut associated = Vec::new();
    let mut associated_note = None;
    if scope == Scope::Regional {
        use aws_sdk_wafv2::types::ResourceType;
        for rt in [
            ResourceType::ApplicationLoadBalancer,
            ResourceType::ApiGateway,
            ResourceType::Appsync,
            ResourceType::CognitioUserPool,
            ResourceType::AppRunnerService,
            ResourceType::VerifiedAccessInstance,
            ResourceType::Amplify,
        ] {
            if let Ok(r) = client
                .list_resources_for_web_acl()
                .web_acl_arn(&arn)
                .resource_type(rt)
                .send()
                .await
            {
                for a in r.resource_arns() {
                    associated.push(a.clone());
                }
            }
        }
    } else {
        associated_note = Some("CloudFront distributions — see @cloudfront".to_string());
    }

    let logging = client
        .get_logging_configuration()
        .resource_arn(&arn)
        .send()
        .await
        .ok()
        .and_then(|r| r.logging_configuration().cloned())
        .map(|lc| WafLogging {
            destinations: lc.log_destination_configs().to_vec(),
            redacted_fields: lc.redacted_fields().len(),
            managed_by_fm: lc.managed_by_firewall_manager(),
        });

    Ok(WafWebAclDetail {
        default_action,
        capacity,
        rules,
        associated,
        associated_note,
        logging,
    })
}
