use crate::aws::client::AwsClients;
use crate::aws::resource::{Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use aws_sdk_costexplorer::types::{
    AnomalyDateInterval, CostCategoryValues, DateInterval, Dimension, DimensionValues, Expression, Granularity,
    GroupDefinition, GroupDefinitionType, MatchOption, Metric, TagValues,
};
use aws_sdk_costexplorer::Client as CeClient;
use chrono::{Datelike, Duration as ChronoDuration, Months, NaiveDate, Utc};
use std::any::Any;
use std::collections::HashMap;
use tokio::sync::mpsc;

/// Cost & Billing service, backed by AWS Cost Explorer (`ce`).
///
/// The list groups unblended spend by a selectable dimension (service / linked
/// account / region / usage type), a cost-allocation tag key, or a cost
/// category over a selectable period (MTD / last month /
/// last 3 months), ranked descending. Drilling into a row fetches its usage-type
/// and region breakdowns plus a month-end forecast.
///
/// Cost Explorer is a global (`us-east-1`) endpoint and bills ~$0.01 per request,
/// so the list is one `GetCostAndUsage` call and the result is cached with a long
/// TTL (see `ResourceCache`). Changing the grouping/period rebuilds the service
/// with new query params and refetches.
pub struct CostService {
    client: CeClient,
    query: CostQuery,
}

impl CostService {
    pub fn new(aws_clients: &AwsClients) -> Self {
        Self::with_query(aws_clients, CostQuery::default())
    }

    pub fn with_query(aws_clients: &AwsClients, query: CostQuery) -> Self {
        Self {
            client: aws_clients.costexplorer_client(),
            query,
        }
    }
}

#[async_trait]
impl AwsService for CostService {
    fn service_type(&self) -> ServiceType {
        ServiceType::Cost
    }

    fn name(&self) -> &str {
        "Cost & Billing"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        if self.query.anomalies {
            return Ok(fetch_anomalies(&self.client)
                .await?
                .into_iter()
                .map(|a| Box::new(a) as Box<dyn Resource>)
                .collect());
        }
        let items = fetch_cost(&self.client, &self.query).await?;
        Ok(items
            .into_iter()
            .map(|i| Box::new(i) as Box<dyn Resource>)
            .collect())
    }

    async fn list_resources_streaming(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        if self.query.anomalies {
            return self.stream_anomalies(event_tx, service_type).await;
        }
        match fetch_cost(&self.client, &self.query).await {
            Ok(items) => {
                let total = items.len();
                if total > 0 {
                    let batch: Vec<Box<dyn Resource>> = items
                        .into_iter()
                        .map(|i| Box::new(i) as Box<dyn Resource>)
                        .collect();
                    let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                        service: service_type,
                        resources: batch,
                        progress: LoadProgress {
                            loaded_count: total,
                            total_count: Some(total),
                            status_message: Some(format!(
                                "Loading cost by {}…",
                                self.query.group_by.noun()
                            )),
                        },
                    });
                }
                let _ = event_tx.send(Event::ResourcesFullyLoaded {
                    service: service_type,
                    total_count: total,
                });
                Ok(())
            }
            Err(e) => {
                let _ = event_tx.send(Event::ResourceLoadError {
                    service: service_type,
                    error: friendly_error(&e.to_string(), "ce:GetCostAndUsage"),
                });
                Ok(())
            }
        }
    }

    async fn get_resource_details(&self, _id: &str) -> Result<Box<dyn Resource>> {
        Err(crate::error::Error::NotImplemented)
    }
}

impl CostService {
    /// The Anomalies view (key `8`): one paginated `GetAnomalies` walk over
    /// the last [`ANOMALY_LOOKBACK_DAYS`], streamed as a single batch like the
    /// spend view (each page is a billed CE request, so there is no point
    /// showing a partial list for the second it takes).
    async fn stream_anomalies(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        match fetch_anomalies(&self.client).await {
            Ok(items) => {
                let total = items.len();
                if total > 0 {
                    let batch: Vec<Box<dyn Resource>> = items
                        .into_iter()
                        .map(|a| Box::new(a) as Box<dyn Resource>)
                        .collect();
                    let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                        service: service_type,
                        resources: batch,
                        progress: LoadProgress {
                            loaded_count: total,
                            total_count: Some(total),
                            status_message: Some("Loading cost anomalies…".to_string()),
                        },
                    });
                }
                let _ = event_tx.send(Event::ResourcesFullyLoaded {
                    service: service_type,
                    total_count: total,
                });
                Ok(())
            }
            Err(e) => {
                let _ = event_tx.send(Event::ResourceLoadError {
                    service: service_type,
                    error: friendly_error(&e.to_string(), "ce:GetAnomalies"),
                });
                Ok(())
            }
        }
    }
}

/// Map raw Cost Explorer SDK errors to actionable hints. `action` is the IAM
/// action the failed view needs, named in the access-denied hint.
fn friendly_error(raw: &str, action: &str) -> String {
    let low = raw.to_lowercase();
    if low.contains("not subscribed") || low.contains("not enabled") {
        "Cost Explorer is not enabled for this account. Enable it in the Billing console (takes ~24h to populate).".to_string()
    } else if low.contains("accessdenied") || low.contains("access denied") || low.contains("not authorized") {
        format!(
            "Access denied — need {}. Cost Explorer must also be enabled by the management/payer account.",
            action
        )
    } else {
        format!("Failed to load cost data: {}", raw)
    }
}

// ── Query parameters (grouping + period toggles) ──────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CostGroupBy {
    Service,
    LinkedAccount,
    Region,
    UsageType,
    /// A cost-allocation tag key (key `9`); the key itself rides
    /// `CostQuery.group_key` / `App.cost_tag_key`.
    Tag,
    /// A cost category (key `0`); the category name rides
    /// `CostQuery.group_key` / `App.cost_category`.
    CostCategory,
}

impl CostGroupBy {
    /// Cost Explorer `GroupBy` key for the four dimensions; `TAG` /
    /// `COST_CATEGORY` for the keyed groupings (stored on each row as
    /// `CostLineItem.dimension`, read back by `from_dimension_key`).
    pub fn dimension_key(&self) -> &'static str {
        match self {
            CostGroupBy::Service => "SERVICE",
            CostGroupBy::LinkedAccount => "LINKED_ACCOUNT",
            CostGroupBy::Region => "REGION",
            CostGroupBy::UsageType => "USAGE_TYPE",
            CostGroupBy::Tag => "TAG",
            CostGroupBy::CostCategory => "COST_CATEGORY",
        }
    }

    /// SDK `Dimension` enum (for filter expressions). None for the keyed
    /// groupings, which filter on `Tags` / `CostCategories` instead.
    fn dimension(&self) -> Option<Dimension> {
        match self {
            CostGroupBy::Service => Some(Dimension::Service),
            CostGroupBy::LinkedAccount => Some(Dimension::LinkedAccount),
            CostGroupBy::Region => Some(Dimension::Region),
            CostGroupBy::UsageType => Some(Dimension::UsageType),
            CostGroupBy::Tag | CostGroupBy::CostCategory => None,
        }
    }

    /// Whether this grouping needs a key (tag key / category name).
    pub fn is_keyed(&self) -> bool {
        matches!(self, CostGroupBy::Tag | CostGroupBy::CostCategory)
    }

    pub fn label(&self) -> &'static str {
        match self {
            CostGroupBy::Service => "Service",
            CostGroupBy::LinkedAccount => "Account",
            CostGroupBy::Region => "Region",
            CostGroupBy::UsageType => "Usage Type",
            CostGroupBy::Tag => "Tag",
            CostGroupBy::CostCategory => "Category",
        }
    }

    fn noun(&self) -> &'static str {
        match self {
            CostGroupBy::Service => "service",
            CostGroupBy::LinkedAccount => "account",
            CostGroupBy::Region => "region",
            CostGroupBy::UsageType => "usage type",
            CostGroupBy::Tag => "tag",
            CostGroupBy::CostCategory => "cost category",
        }
    }

    /// Row label for spend with no value for the tag / category.
    fn absent_label(&self) -> &'static str {
        match self {
            CostGroupBy::CostCategory => "(uncategorized)",
            _ => "(untagged)",
        }
    }
}

/// Split a keyed `GetCostAndUsage` group key (`"team$payments"`,
/// `"Team$"` for spend without a value) into its value; a key without a
/// `$` is taken whole. An empty value means "absent".
pub(crate) fn keyed_group_value(raw: &str) -> &str {
    raw.split_once('$').map(|(_, v)| v).unwrap_or(raw)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CostPeriod {
    Mtd,
    LastMonth,
    Last3Months,
}

impl CostPeriod {
    pub fn label(&self) -> &'static str {
        match self {
            CostPeriod::Mtd => "MTD",
            CostPeriod::LastMonth => "Last Mo",
            CostPeriod::Last3Months => "3 Mo",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CostQuery {
    pub group_by: CostGroupBy,
    /// The tag key / cost category name for a keyed `group_by`
    /// (`CostGroupBy::is_keyed`); ignored otherwise.
    pub group_key: Option<String>,
    pub period: CostPeriod,
    /// The Anomalies view (key `8`): list Cost Anomaly Detection anomalies
    /// instead of grouped spend. `group_by`/`period` are kept (not used) so
    /// leaving the view returns to the grouping the user had.
    pub anomalies: bool,
}

impl Default for CostQuery {
    fn default() -> Self {
        Self {
            group_by: CostGroupBy::Service,
            group_key: None,
            period: CostPeriod::Mtd,
            anomalies: false,
        }
    }
}

/// Resolved date windows for a period: the current window `[cur_start, cur_end)`
/// and a comparable prior window `[prior_start, prior_end)` (empty for 3-month).
struct Windows {
    cur_start: NaiveDate,
    cur_end: NaiveDate,
    prior_start: NaiveDate,
    prior_end: NaiveDate,
    cur_label: String,
    prior_label: String,
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

fn fmt_date(d: NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}

fn month_label(d: NaiveDate) -> String {
    d.format("%b").to_string()
}

fn resolve_windows(period: CostPeriod) -> Windows {
    resolve_windows_on(period, Utc::now().date_naive())
}

/// Pure core of `resolve_windows`, parameterised on `today` so the date math is
/// deterministically testable (the public wrapper passes `Utc::now()`).
fn resolve_windows_on(period: CostPeriod, today: NaiveDate) -> Windows {
    let month_start = today.with_day(1).expect("day 1 valid");
    let prev_month_start = (month_start - ChronoDuration::days(1))
        .with_day(1)
        .expect("day 1 valid");
    let prev2_month_start = (prev_month_start - ChronoDuration::days(1))
        .with_day(1)
        .expect("day 1 valid");

    match period {
        CostPeriod::Mtd => {
            let cur_end = today + ChronoDuration::days(1); // exclusive, include today
            let days_elapsed = (today - month_start).num_days() + 1;
            let mut prior_end = prev_month_start + ChronoDuration::days(days_elapsed);
            if prior_end > month_start {
                prior_end = month_start;
            }
            let prior_last = prior_end - ChronoDuration::days(1);
            Windows {
                cur_start: month_start,
                cur_end,
                prior_start: prev_month_start,
                prior_end,
                cur_label: format!("{} {}–{}", month_label(month_start), month_start.day(), today.day()),
                prior_label: format!(
                    "{} {}–{}",
                    month_label(prev_month_start),
                    prev_month_start.day(),
                    prior_last.day().max(prev_month_start.day())
                ),
            }
        }
        CostPeriod::LastMonth => Windows {
            cur_start: prev_month_start,
            cur_end: month_start,
            prior_start: prev2_month_start,
            prior_end: prev_month_start,
            cur_label: format!("{} (full)", month_label(prev_month_start)),
            prior_label: format!("{} (full)", month_label(prev2_month_start)),
        },
        CostPeriod::Last3Months => {
            let cur_start = month_start
                .checked_sub_months(Months::new(2))
                .unwrap_or(prev2_month_start);
            let cur_end = today + ChronoDuration::days(1);
            Windows {
                cur_start,
                cur_end,
                // No comparable prior window for the rolling 3-month view.
                prior_start: cur_start,
                prior_end: cur_start,
                cur_label: format!("{}–{}", month_label(cur_start), month_label(today)),
                prior_label: "—".to_string(),
            }
        }
    }
}

// ── CostLineItem ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct CostLineItem {
    /// Cost Explorer dimension key this row was grouped by (e.g. `"SERVICE"`,
    /// or `"TAG"` / `"COST_CATEGORY"` for the keyed groupings).
    pub dimension: String,
    /// The tag key / cost category name for a keyed grouping; empty for the
    /// four dimensions.
    pub group_key: String,
    /// The grouped value as Cost Explorer filters on it — equal to `key` for
    /// the dimensions; for keyed groupings the bare value, empty when the
    /// spend has no value for the tag / category.
    pub filter_value: String,
    /// The grouped value as shown — service name, account id, region, usage
    /// type, tag / category value, or `(untagged)` / `(uncategorized)`.
    pub key: String,
    /// Current-window unblended cost.
    pub current: f64,
    /// Comparable prior-window cost (0.0 when no prior, e.g. 3-month view).
    pub prior: f64,
    pub currency: String,
    /// (date `YYYY-MM-DD`, amount) over the current window, oldest → newest.
    pub daily: Vec<(String, f64)>,
    pub period_label: String,
    pub prior_label: String,
    /// Whether the active period has a comparable prior window.
    pub has_prior: bool,
    /// Month-over-month delta for multi-month windows (the 3-month view):
    /// last complete month vs the one before, derived from the daily series.
    /// None for single-month periods or without two complete months of data.
    pub mom_delta_pct: Option<f64>,
    /// Precomputed `"$1,234.56  ↑12%"` shown dimmed in the list row.
    summary: String,
    tags: HashMap<String, String>,
}

impl CostLineItem {
    /// Layer-2 harness mock — the only constructor outside the fetch path
    /// (`summary`/`tags` are private, so the harness can't use a struct
    /// literal).
    #[cfg(test)]
    pub(crate) fn mock() -> Self {
        Self {
            dimension: "SERVICE".to_string(),
            group_key: String::new(),
            filter_value: "Mock Service".to_string(),
            key: "Mock Service".to_string(),
            current: 12.34,
            prior: 10.0,
            currency: "USD".to_string(),
            daily: vec![("2026-07-01".to_string(), 1.0)],
            period_label: "MTD".to_string(),
            prior_label: "prior MTD".to_string(),
            has_prior: true,
            mom_delta_pct: None,
            summary: "$12.34".to_string(),
            tags: HashMap::new(),
        }
    }

    /// Percent change vs the comparable prior period (None if no prior spend).
    pub fn delta_pct(&self) -> Option<f64> {
        if self.has_prior && self.prior > 0.0 {
            Some((self.current - self.prior) / self.prior * 100.0)
        } else {
            None
        }
    }

    /// Mean daily spend across the days with spend in the current window.
    pub fn daily_avg(&self) -> f64 {
        let days = self.daily.iter().filter(|(_, a)| *a > 0.0).count();
        if days == 0 {
            0.0
        } else {
            self.current / days as f64
        }
    }

    /// Daily series as chart points (x = day index, y = amount).
    pub fn daily_points(&self) -> Vec<(f64, f64)> {
        self.daily
            .iter()
            .enumerate()
            .map(|(i, (_, a))| (i as f64, *a))
            .collect()
    }

    /// Oldest date label for the chart x-axis (e.g. `"May 1"`).
    pub fn first_date_label(&self) -> String {
        self.daily
            .first()
            .map(|(d, _)| short_date(d))
            .unwrap_or_default()
    }

    /// Month rows for multi-month windows: (label, total, is_current_month),
    /// oldest → newest. The current month is flagged so renderers can mark it
    /// as partial ("to date").
    pub fn monthly_rows(&self) -> Vec<(String, f64, bool)> {
        let cur_ym = Utc::now().format("%Y-%m").to_string();
        monthly_buckets(&self.daily)
            .into_iter()
            .map(|(ym, total)| (ym_label(&ym), total, ym == cur_ym))
            .collect()
    }

    /// The trend driving the arrows/dots: prior-window delta when the period
    /// has one, else the month-over-month delta (3-month view).
    fn effective_delta(&self) -> Option<f64> {
        self.delta_pct().or(self.mom_delta_pct)
    }

    fn trend_label(&self) -> String {
        match self.effective_delta() {
            Some(p) if p >= 0.5 => format!("↑{:.0}%", p),
            Some(p) if p <= -0.5 => format!("↓{:.0}%", p.abs()),
            Some(_) => "≈".to_string(),
            None if self.current > 0.0 && self.has_prior => "new".to_string(),
            None => String::new(),
        }
    }
}

crate::sections! {
    pub enum CostDetailSection,
    pub static COST_SECTIONS = [
        Breakdown "Breakdown" => crate::app::App::trigger_cost_drilldown_load,
        Regions "Regions" => crate::app::App::trigger_cost_drilldown_load,
        Trend "Trend" => crate::app::App::trigger_cost_drilldown_load,
        Forecast "Forecast" => crate::app::App::trigger_cost_drilldown_load,
    ]
}

impl Resource for CostLineItem {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&COST_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.summary
    }

    fn name(&self) -> &str {
        &self.key
    }

    fn resource_type(&self) -> &str {
        "Cost"
    }

    fn state(&self) -> ResourceState {
        // Encode trend as the row's status dot: spend up = red (attention),
        // down = green, flat/no-prior = neutral. Multi-month periods use the
        // month-over-month delta so the signal doesn't vanish.
        match self.effective_delta() {
            Some(p) if p >= 5.0 => ResourceState::Unavailable,
            Some(p) if p <= -5.0 => ResourceState::Available,
            _ => ResourceState::Unknown(String::new()),
        }
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        self.key.clone()
    }

    fn details(&self) -> Vec<(String, String)> {
        // Flat summary used by the `X` export; the on-screen pane is the split
        // renderer (cost_section_lines).
        let cur = &self.currency;
        let mut rows = vec![
            ("Key".to_string(), self.key.clone()),
            (
                format!("Current ({})", self.period_label),
                format!("${} {}", fmt_money(self.current), cur),
            ),
        ];
        if self.has_prior {
            rows.push((
                format!("Prior ({})", self.prior_label),
                format!("${} {}", fmt_money(self.prior), cur),
            ));
            if let Some(p) = self.delta_pct() {
                rows.push(("Change".to_string(), format!("{:+.1}%", p)));
            }
        }
        rows.push((
            "Daily avg".to_string(),
            format!("${} {}", fmt_money(self.daily_avg()), cur),
        ));
        rows
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn estimated_monthly_cost(&self) -> Option<f64> {
        Some(self.current)
    }

    fn console_url(&self, _region: &str) -> Option<String> {
        Some("https://console.aws.amazon.com/cost-management/home#/cost-explorer".to_string())
    }
}

// ── Fetch (list) ──────────────────────────────────────────────────────────────

/// Per-group accumulator: (current, prior, daily points, currency).
type CostAccum = (f64, f64, Vec<(String, f64)>, String);

/// One `GetCostAndUsage` call (DAILY, grouped by the query dimension) spanning
/// both the current and prior windows. From the daily buckets we derive, per
/// group: current-window total, prior-window total, and the daily series.
pub async fn fetch_cost(client: &CeClient, query: &CostQuery) -> Result<Vec<CostLineItem>> {
    let w = resolve_windows(query.period);
    let has_prior = w.prior_end > w.prior_start;

    // Fetch window spans from the earliest of the two windows to the current end.
    let fetch_start = if has_prior && w.prior_start < w.cur_start {
        w.prior_start
    } else {
        w.cur_start
    };

    let cur_start_s = fmt_date(w.cur_start);
    let cur_end_s = fmt_date(w.cur_end);
    let prior_start_s = fmt_date(w.prior_start);
    let prior_end_s = fmt_date(w.prior_end);

    let interval = DateInterval::builder()
        .start(fmt_date(fetch_start))
        .end(cur_end_s.clone())
        .build()
        .map_err(|e| crate::error::Error::AwsSdk(e.to_string()))?;

    let group_key = query.group_key.clone().unwrap_or_default();
    let group_by = match query.group_by {
        CostGroupBy::Tag | CostGroupBy::CostCategory if group_key.is_empty() => {
            return Err(crate::error::Error::AwsSdk(format!(
                "no {} chosen to group by",
                query.group_by.noun()
            )));
        }
        CostGroupBy::Tag => GroupDefinition::builder()
            .r#type(GroupDefinitionType::Tag)
            .key(&group_key)
            .build(),
        CostGroupBy::CostCategory => GroupDefinition::builder()
            .r#type(GroupDefinitionType::CostCategory)
            .key(&group_key)
            .build(),
        _ => GroupDefinition::builder()
            .r#type(GroupDefinitionType::Dimension)
            .key(query.group_by.dimension_key())
            .build(),
    };

    let mut accum: HashMap<String, CostAccum> = HashMap::new();
    let mut next_token: Option<String> = None;

    loop {
        let mut req = client
            .get_cost_and_usage()
            .time_period(interval.clone())
            .granularity(Granularity::Daily)
            .metrics("UnblendedCost")
            .group_by(group_by.clone());
        if let Some(t) = &next_token {
            req = req.next_page_token(t);
        }

        let resp = req
            .send()
            .await
            .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;

        for rbt in resp.results_by_time() {
            let bucket_start = rbt.time_period().map(|p| p.start()).unwrap_or("").to_string();
            for g in rbt.groups() {
                let key = g.keys().first().cloned().unwrap_or_default();
                if key.is_empty() {
                    continue;
                }
                let (amt, unit) = g
                    .metrics()
                    .and_then(|m| m.get("UnblendedCost"))
                    .map(|v| {
                        (
                            v.amount().unwrap_or("0").parse::<f64>().unwrap_or(0.0),
                            v.unit().unwrap_or("USD").to_string(),
                        )
                    })
                    .unwrap_or((0.0, "USD".to_string()));

                let entry = accum
                    .entry(key)
                    .or_insert((0.0, 0.0, Vec::new(), unit.clone()));
                entry.3 = unit;
                let in_current =
                    bucket_start.as_str() >= cur_start_s.as_str() && bucket_start.as_str() < cur_end_s.as_str();
                let in_prior = has_prior
                    && bucket_start.as_str() >= prior_start_s.as_str()
                    && bucket_start.as_str() < prior_end_s.as_str();
                if in_current {
                    entry.0 += amt;
                    entry.2.push((bucket_start.clone(), amt));
                } else if in_prior {
                    entry.1 += amt;
                }
            }
        }

        next_token = crate::aws::pagination::next_page_token(resp.next_page_token(), &next_token);
        if next_token.is_none() {
            break;
        }
    }

    let dimension = query.group_by.dimension_key().to_string();
    // Current month as "YYYY-MM" (cur_end is exclusive → last covered day).
    let cur_ym = fmt_date(w.cur_end - ChronoDuration::days(1))[..7].to_string();
    let mut items: Vec<CostLineItem> = accum
        .into_iter()
        .map(|(raw_key, (current, prior, mut daily, currency))| {
            daily.sort_by(|a, b| a.0.cmp(&b.0));
            // Keyed groupings come back as `key$value`; the dimensions are
            // the bare value.
            let (filter_value, key) = if query.group_by.is_keyed() {
                let v = keyed_group_value(&raw_key).to_string();
                let shown = if v.is_empty() {
                    query.group_by.absent_label().to_string()
                } else {
                    v.clone()
                };
                (v, shown)
            } else {
                (raw_key.clone(), raw_key)
            };
            let mut item = CostLineItem {
                dimension: dimension.clone(),
                group_key: if query.group_by.is_keyed() { group_key.clone() } else { String::new() },
                filter_value,
                key,
                current,
                prior,
                currency,
                daily,
                period_label: w.cur_label.clone(),
                prior_label: w.prior_label.clone(),
                has_prior,
                mom_delta_pct: None,
                summary: String::new(),
                tags: HashMap::new(),
            };
            // The 3-month view has no comparable prior window; derive a
            // month-over-month trend from the daily data already fetched
            // (last complete month vs the one before) so the arrows and
            // status dots survive the period switch.
            if query.period == CostPeriod::Last3Months {
                item.mom_delta_pct = mom_delta(&monthly_buckets(&item.daily), &cur_ym);
            }
            let trend = item.trend_label();
            item.summary = if trend.is_empty() {
                format!("${}", fmt_money(item.current))
            } else {
                format!("${}  {}", fmt_money(item.current), trend)
            };
            item
        })
        .filter(|i| i.current.abs() > 0.0001 || i.prior.abs() > 0.0001)
        .collect();

    items.sort_by(|a, b| {
        b.current
            .partial_cmp(&a.current)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    Ok(items)
}

// ── Group-key pickers (keys 9 / 0) ────────────────────────────────────────────

/// Cost-allocation tag keys with spend in the last three months (the
/// longest period the list offers) — `GetTags` without a `TagKey` lists
/// keys. Only *activated* cost-allocation tags appear, so an empty list is
/// the normal answer for an account that never activated any. Every page is
/// a billed CE request; fetched only when the `9` picker opens.
pub async fn fetch_cost_tag_keys(client: CeClient) -> Result<Vec<String>> {
    let today = Utc::now().date_naive();
    let start = resolve_windows_on(CostPeriod::Last3Months, today).cur_start;
    let interval = DateInterval::builder()
        .start(fmt_date(start))
        .end(fmt_date(today + ChronoDuration::days(1)))
        .build()
        .map_err(|e| crate::error::Error::AwsSdk(e.to_string()))?;
    let mut keys: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut req = client.get_tags().time_period(interval.clone());
        if let Some(t) = &token {
            req = req.next_page_token(t);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
        keys.extend(resp.tags().iter().filter(|k| !k.is_empty()).cloned());
        token = crate::aws::pagination::next_page_token(resp.next_page_token(), &token);
        if token.is_none() {
            break;
        }
    }
    keys.sort_by_key(|k| k.to_lowercase());
    keys.dedup();
    Ok(keys)
}

/// Cost category names (`ListCostCategoryDefinitions`, the definitions in
/// effect today). Fetched only when the `0` picker opens.
pub async fn fetch_cost_category_names(client: CeClient) -> Result<Vec<String>> {
    let mut names: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut req = client.list_cost_category_definitions();
        if let Some(t) = &token {
            req = req.next_token(t);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
        names.extend(
            resp.cost_category_references()
                .iter()
                .filter_map(|r| r.name())
                .map(str::to_string),
        );
        token = crate::aws::pagination::next_page_token(resp.next_token(), &token);
        if token.is_none() {
            break;
        }
    }
    names.sort_by_key(|k| k.to_lowercase());
    names.dedup();
    Ok(names)
}

/// The picker's error text for a failed key-list fetch.
pub fn group_keys_error(raw: &str, action: &str) -> String {
    friendly_error(raw, action)
}

// ── Anomalies (key 8: Cost Anomaly Detection) ─────────────────────────────────

/// How far back the Anomalies view looks. Cost Anomaly Detection keeps 90
/// days of history in the console's default view; older anomalies are rarely
/// actionable and every extra page is a billed request.
pub const ANOMALY_LOOKBACK_DAYS: i64 = 90;

/// One root cause AWS attributes an anomaly to — any subset of the four
/// dimensions can be set, plus this cause's share of the impact.
#[derive(Debug, Clone, Default)]
pub struct CostRootCause {
    pub service: Option<String>,
    pub region: Option<String>,
    pub linked_account: Option<String>,
    pub linked_account_name: Option<String>,
    pub usage_type: Option<String>,
    /// Dollar contribution of this cause to the anomaly's impact.
    pub contribution: Option<f64>,
}

/// A Cost Anomaly Detection anomaly (`ce:GetAnomalies`). Everything the pane
/// shows comes back on the list call, so both sections are eager.
#[derive(Debug, Clone)]
pub struct CostAnomaly {
    pub anomaly_id: String,
    /// The monitored dimension value (a service name for an AWS-services
    /// monitor, an account / cost category / tag value otherwise). Empty
    /// when AWS didn't report one.
    pub dimension_value: String,
    /// `YYYY-MM-DD` (the API's date string, time part dropped).
    pub start_date: Option<String>,
    /// None while the anomaly is still ongoing.
    pub end_date: Option<String>,
    pub max_score: f64,
    pub current_score: f64,
    /// Spend above expected, summed over the anomaly's duration.
    pub total_impact: f64,
    /// The largest single-day impact.
    pub max_impact: f64,
    pub total_actual: Option<f64>,
    pub total_expected: Option<f64>,
    pub impact_pct: Option<f64>,
    pub monitor_arn: String,
    /// The user's feedback (`YES` / `NO` / `PLANNED_ACTIVITY`), if given.
    pub feedback: Option<String>,
    /// Sorted by contribution, largest first.
    pub root_causes: Vec<CostRootCause>,
    label: String,
    /// Precomputed `"$96.40 over (+82%) · Sep 28 – ongoing"` list cell.
    summary: String,
    search_blob: String,
    tags: HashMap<String, String>,
}

/// `"2026-09-28T00:00:00Z"` / `"2026-09-28"` → `"2026-09-28"`.
fn anomaly_day(s: &str) -> String {
    s.get(..10).unwrap_or(s).to_string()
}

impl CostAnomaly {
    pub fn from_sdk(a: &aws_sdk_costexplorer::types::Anomaly) -> Self {
        let mut root_causes: Vec<CostRootCause> = a
            .root_causes()
            .iter()
            .map(|rc| CostRootCause {
                service: rc.service().map(str::to_string),
                region: rc.region().map(str::to_string),
                linked_account: rc.linked_account().map(str::to_string),
                linked_account_name: rc.linked_account_name().map(str::to_string),
                usage_type: rc.usage_type().map(str::to_string),
                contribution: rc.impact().map(|i| i.contribution()),
            })
            .collect();
        root_causes.sort_by(|x, y| {
            y.contribution
                .unwrap_or(0.0)
                .partial_cmp(&x.contribution.unwrap_or(0.0))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let impact = a.impact();
        let score = a.anomaly_score();
        let mut item = Self {
            anomaly_id: a.anomaly_id().to_string(),
            dimension_value: a.dimension_value().unwrap_or_default().to_string(),
            start_date: a.anomaly_start_date().map(anomaly_day),
            end_date: a.anomaly_end_date().filter(|s| !s.is_empty()).map(anomaly_day),
            max_score: score.map(|s| s.max_score()).unwrap_or(0.0),
            current_score: score.map(|s| s.current_score()).unwrap_or(0.0),
            total_impact: impact.map(|i| i.total_impact()).unwrap_or(0.0),
            max_impact: impact.map(|i| i.max_impact()).unwrap_or(0.0),
            total_actual: impact.and_then(|i| i.total_actual_spend()),
            total_expected: impact.and_then(|i| i.total_expected_spend()),
            impact_pct: impact.and_then(|i| i.total_impact_percentage()),
            monitor_arn: a.monitor_arn().to_string(),
            feedback: a.feedback().map(|f| f.as_str().to_string()),
            root_causes,
            label: String::new(),
            summary: String::new(),
            search_blob: String::new(),
            tags: HashMap::new(),
        };
        item.label = item.compute_label();
        item.summary = item.compute_summary();
        item.search_blob = item.compute_search_blob();
        item
    }

    /// Layer-2 harness / renderer-test mock: one ongoing anomaly with two
    /// root causes.
    #[cfg(test)]
    pub(crate) fn mock() -> Self {
        use aws_sdk_costexplorer::types::{Anomaly, AnomalyScore, Impact, RootCause, RootCauseImpact};
        Self::from_sdk(
            &Anomaly::builder()
                .anomaly_id("11111111-2222-3333-4444-555555555555")
                .monitor_arn("arn:aws:ce::123456789012:anomalymonitor/mock")
                .dimension_value("Amazon CloudWatch")
                .anomaly_start_date("2026-09-28T00:00:00Z")
                .anomaly_score(AnomalyScore::builder().max_score(0.91).current_score(0.4).build())
                .impact(
                    Impact::builder()
                        .max_impact(31.2)
                        .total_impact(96.4)
                        .total_actual_spend(213.9)
                        .total_expected_spend(117.5)
                        .total_impact_percentage(82.0)
                        .build(),
                )
                .root_causes(
                    RootCause::builder()
                        .service("AmazonCloudWatch")
                        .region("us-east-1")
                        .linked_account("123456789012")
                        .linked_account_name("acme-prod")
                        .usage_type("USE1-DataProcessing-Bytes")
                        .impact(RootCauseImpact::builder().contribution(88.1).build())
                        .build(),
                )
                .root_causes(
                    RootCause::builder()
                        .service("AmazonCloudWatch")
                        .usage_type("USE1-TimedStorage-ByteHrs")
                        .impact(RootCauseImpact::builder().contribution(8.3).build())
                        .build(),
                )
                .build()
                .unwrap(),
        )
    }

    pub fn is_ongoing(&self) -> bool {
        self.end_date.is_none()
    }

    /// The row name: the monitored dimension value, else the top root
    /// cause's service (a dimension-less monitor), else a generic label.
    fn compute_label(&self) -> String {
        if !self.dimension_value.is_empty() {
            return self.dimension_value.clone();
        }
        self.root_causes
            .iter()
            .find_map(|rc| rc.service.clone())
            .unwrap_or_else(|| "Cost anomaly".to_string())
    }

    /// `Sep 28 – ongoing` / `Sep 28 – Oct 2` / `Sep 28`.
    pub fn date_range(&self) -> String {
        let start = self.start_date.as_deref().map(short_date).unwrap_or_default();
        match self.end_date.as_deref() {
            None => format!("{} – ongoing", start),
            Some(e) if Some(e) == self.start_date.as_deref() => start,
            Some(e) => format!("{} – {}", start, short_date(e)),
        }
    }

    fn compute_summary(&self) -> String {
        let pct = self
            .impact_pct
            .map(|p| format!(" (+{:.0}%)", p))
            .unwrap_or_default();
        format!(
            "${} over{} · {}",
            fmt_money(self.total_impact),
            pct,
            self.date_range()
        )
    }

    fn compute_search_blob(&self) -> String {
        let mut parts = vec![self.label.clone(), self.anomaly_id.clone()];
        for rc in &self.root_causes {
            parts.extend(
                [&rc.service, &rc.region, &rc.linked_account, &rc.linked_account_name, &rc.usage_type]
                    .into_iter()
                    .flatten()
                    .cloned(),
            );
        }
        parts.push(if self.is_ongoing() { "ongoing" } else { "closed" }.to_string());
        parts.join(" ")
    }

    /// The list's dim second cell (`resource_list::id_cell`) — the id is a
    /// UUID nobody wants to read.
    pub fn summary(&self) -> &str {
        &self.summary
    }
}

crate::sections! {
    pub enum CostAnomalyDetailSection,
    pub static COST_ANOMALY_SECTIONS = [
        Overview "Overview",
        RootCauses "Root causes",
    ]
}

impl Resource for CostAnomaly {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&COST_ANOMALY_SECTIONS)
    }

    fn id(&self) -> &str {
        &self.anomaly_id
    }

    fn name(&self) -> &str {
        &self.label
    }

    fn resource_type(&self) -> &str {
        "Cost Anomaly"
    }

    fn state(&self) -> ResourceState {
        // The dot is the impact, the Cost spend view's "red = attention" rule:
        // ≥ $100 over expected red, ≥ $10 yellow, smaller ones neutral.
        if self.total_impact >= 100.0 {
            ResourceState::Unavailable
        } else if self.total_impact >= 10.0 {
            ResourceState::Pending
        } else {
            ResourceState::Unknown(String::new())
        }
    }

    fn state_label(&self) -> String {
        let word = if self.is_ongoing() { "ongoing" } else { "closed" };
        word.to_string()
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        self.search_blob.clone()
    }

    fn details(&self) -> Vec<(String, String)> {
        let mut rows = vec![
            ("Anomaly ID".to_string(), self.anomaly_id.clone()),
            ("Dimension".to_string(), self.label.clone()),
            ("Dates".to_string(), self.date_range()),
            ("Total impact".to_string(), format!("${}", fmt_money(self.total_impact))),
            ("Max score".to_string(), format!("{:.2}", self.max_score)),
            ("Monitor".to_string(), self.monitor_arn.clone()),
        ];
        if let Some(f) = &self.feedback {
            rows.push(("Feedback".to_string(), f.clone()));
        }
        rows
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn cli_command(&self) -> Option<String> {
        // GetAnomalies can't filter by id; scope it to this anomaly's monitor
        // and start date, which is as narrow as the API goes.
        let start = self.start_date.clone().unwrap_or_default();
        Some(format!(
            "aws ce get-anomalies --monitor-arn {} --date-interval StartDate={}",
            crate::aws::resource::shell_quote(&self.monitor_arn),
            crate::aws::resource::shell_quote(&start)
        ))
    }

    fn console_url(&self, _region: &str) -> Option<String> {
        Some("https://console.aws.amazon.com/cost-management/home#/anomaly-detection/overview".to_string())
    }
}

/// Every anomaly whose window overlaps the last [`ANOMALY_LOOKBACK_DAYS`],
/// newest first (ties: larger impact first). Walks every page — each one is a
/// billed CE request, but a cut list would hide exactly the old-but-large
/// anomaly a cleanup is looking for.
pub async fn fetch_anomalies(client: &CeClient) -> Result<Vec<CostAnomaly>> {
    let start = Utc::now().date_naive() - ChronoDuration::days(ANOMALY_LOOKBACK_DAYS);
    let interval = AnomalyDateInterval::builder()
        .start_date(fmt_date(start))
        .build()
        .map_err(|e| crate::error::Error::AwsSdk(e.to_string()))?;

    let mut out: Vec<CostAnomaly> = Vec::new();
    let mut next_token: Option<String> = None;
    loop {
        let mut req = client.get_anomalies().date_interval(interval.clone());
        if let Some(t) = &next_token {
            req = req.next_page_token(t);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
        out.extend(resp.anomalies().iter().map(CostAnomaly::from_sdk));
        next_token = crate::aws::pagination::next_page_token(resp.next_page_token(), &next_token);
        if next_token.is_none() {
            break;
        }
    }
    sort_anomalies(&mut out);
    Ok(out)
}

/// Newest start first; same-day anomalies by impact, largest first.
fn sort_anomalies(items: &mut [CostAnomaly]) {
    items.sort_by(|a, b| {
        b.start_date.cmp(&a.start_date).then(
            b.total_impact
                .partial_cmp(&a.total_impact)
                .unwrap_or(std::cmp::Ordering::Equal),
        )
    });
}

// ── Drill-down (lazy: usage-type + region breakdowns + month-end forecast) ─────

#[derive(Debug, Clone)]
pub struct CostForecast {
    /// Forecast for the remainder of the current month.
    pub remaining: f64,
    /// Lower/upper bound of the prediction interval (80% by default).
    pub lower: f64,
    pub upper: f64,
    pub currency: String,
}

#[derive(Debug, Clone)]
pub struct CostDrilldown {
    /// (usage type, cost) for the selected row, descending.
    pub usage_types: Vec<(String, f64)>,
    /// Secondary breakdown — region (or service if the row is itself a region).
    pub secondary_label: &'static str,
    pub secondary: Vec<(String, f64)>,
    /// Month-end forecast for the selected row (None if CE couldn't forecast).
    pub forecast: Option<CostForecast>,
    /// Human label of the window the breakdowns cover (e.g. `"Jun (full)"`)
    /// — the same window the list totals were computed over.
    pub window_label: String,
    /// This row's current month-to-date spend — the forecast's baseline.
    /// Equals the breakdown sum for MTD; fetched separately for past-window
    /// periods (None if that best-effort call failed).
    pub mtd_spent: Option<f64>,
}

/// Build the filter expression scoping a drill-down to one row: `dimension =
/// value` for the four dimensions, `tag key = value` / `category = value`
/// for the keyed groupings — where an empty value (the `(untagged)` row)
/// becomes an `ABSENT` match, the only way CE filters "no value".
fn row_filter(group_by: CostGroupBy, group_key: &str, value: &str) -> Expression {
    if let Some(dimension) = group_by.dimension() {
        let dv = DimensionValues::builder()
            .key(dimension)
            .values(value.to_string())
            .build();
        return Expression::builder().dimensions(dv).build();
    }
    if group_by == CostGroupBy::CostCategory {
        let mut b = CostCategoryValues::builder().key(group_key);
        b = if value.is_empty() {
            b.match_options(MatchOption::Absent)
        } else {
            b.values(value)
        };
        return Expression::builder().cost_categories(b.build()).build();
    }
    let mut b = TagValues::builder().key(group_key);
    b = if value.is_empty() {
        b.match_options(MatchOption::Absent)
    } else {
        b.values(value)
    };
    Expression::builder().tags(b.build()).build()
}

/// Fetch the usage-type + secondary breakdowns (over the active period's
/// window, matching the list totals) and the current-month-end forecast for
/// one row, all filtered to `dimension = key`. Three concurrent CE calls,
/// plus a fourth (this row's MTD spend, the forecast baseline) when the
/// period's window isn't the current month.
pub async fn fetch_cost_drilldown(
    client: CeClient,
    dimension_key: String,
    group_key: String,
    key: String,
    period: CostPeriod,
) -> Result<CostDrilldown> {
    let group_by = CostGroupBy::from_dimension_key(&dimension_key);

    let w = resolve_windows(period);
    let today = Utc::now().date_naive();
    let month_start = today.with_day(1).expect("day 1 valid");
    let cur_end = today + ChronoDuration::days(1);
    let next_month = month_start
        .checked_add_months(Months::new(1))
        .unwrap_or(cur_end);

    // Breakdowns cover the period's own window so they reconcile with the
    // header/list numbers (for MTD this is month start → today, as before).
    let window_interval = DateInterval::builder()
        .start(fmt_date(w.cur_start))
        .end(fmt_date(w.cur_end))
        .build()
        .map_err(|e| crate::error::Error::AwsSdk(e.to_string()))?;

    // Secondary dimension: region, unless this row IS a region → services.
    // A tag / category row reads best by service (which services make up
    // `team=payments`), the console's default for those groupings.
    let (sec_key, sec_label): (&str, &'static str) = match group_by {
        CostGroupBy::Region | CostGroupBy::Tag | CostGroupBy::CostCategory => ("SERVICE", "Services"),
        _ => ("REGION", "Regions"),
    };

    let filter = row_filter(group_by, &group_key, &key);

    let usage_fut = client
        .get_cost_and_usage()
        .time_period(window_interval.clone())
        .granularity(Granularity::Monthly)
        .metrics("UnblendedCost")
        .group_by(
            GroupDefinition::builder()
                .r#type(GroupDefinitionType::Dimension)
                .key("USAGE_TYPE")
                .build(),
        )
        .set_filter(Some(filter.clone()))
        .send();

    let sec_fut = client
        .get_cost_and_usage()
        .time_period(window_interval.clone())
        .granularity(Granularity::Monthly)
        .metrics("UnblendedCost")
        .group_by(
            GroupDefinition::builder()
                .r#type(GroupDefinitionType::Dimension)
                .key(sec_key)
                .build(),
        )
        .set_filter(Some(filter.clone()))
        .send();

    // For MTD the breakdown sum IS the row's month-to-date spend; for past or
    // multi-month windows fetch it separately (best-effort — the forecast
    // section degrades to remaining-only without it).
    let mtd_fut = async {
        if period == CostPeriod::Mtd {
            return None;
        }
        let interval = DateInterval::builder()
            .start(fmt_date(month_start))
            .end(fmt_date(cur_end))
            .build()
            .ok()?;
        let resp = client
            .get_cost_and_usage()
            .time_period(interval)
            .granularity(Granularity::Monthly)
            .metrics("UnblendedCost")
            .set_filter(Some(filter.clone()))
            .send()
            .await
            .ok()?;
        let total: f64 = resp
            .results_by_time()
            .iter()
            .filter_map(|r| {
                r.total()
                    .and_then(|m| m.get("UnblendedCost"))
                    .and_then(|v| v.amount())
                    .and_then(|a| a.parse::<f64>().ok())
            })
            .sum();
        Some(total)
    };

    // Forecast covers from today to the start of next month.
    let forecast_fut = async {
        if today >= next_month {
            return None;
        }
        let fc_interval = DateInterval::builder()
            .start(fmt_date(today))
            .end(fmt_date(next_month))
            .build()
            .ok()?;
        client
            .get_cost_forecast()
            .time_period(fc_interval)
            .metric(Metric::UnblendedCost)
            .granularity(Granularity::Monthly)
            .set_filter(Some(filter.clone()))
            .send()
            .await
            .ok()
    };

    let (usage_res, sec_res, forecast_res, mtd_res) =
        tokio::join!(usage_fut, sec_fut, forecast_fut, mtd_fut);

    // Multi-month windows return one time bucket per month, so the same key
    // appears once per bucket — aggregate before ranking.
    let parse_groups = |resp: std::result::Result<
        aws_sdk_costexplorer::operation::get_cost_and_usage::GetCostAndUsageOutput,
        _,
    >| {
        let mut totals: HashMap<String, f64> = HashMap::new();
        if let Ok(o) = resp {
            for rbt in o.results_by_time() {
                for g in rbt.groups() {
                    let k = g.keys().first().cloned().unwrap_or_default();
                    let amt = g
                        .metrics()
                        .and_then(|m| m.get("UnblendedCost"))
                        .and_then(|v| v.amount())
                        .and_then(|a| a.parse::<f64>().ok())
                        .unwrap_or(0.0);
                    if !k.is_empty() {
                        *totals.entry(k).or_insert(0.0) += amt;
                    }
                }
            }
        }
        let mut out: Vec<(String, f64)> = totals
            .into_iter()
            .filter(|(_, a)| a.abs() > 0.0001)
            .collect();
        out.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        out
    };

    let usage_types = parse_groups(usage_res);
    let secondary = parse_groups(sec_res);

    let mtd_spent = if period == CostPeriod::Mtd {
        Some(usage_types.iter().map(|(_, a)| *a).sum())
    } else {
        mtd_res
    };

    let forecast = forecast_res.and_then(|o| {
        let remaining = o
            .total()
            .and_then(|t| t.amount())
            .and_then(|a| a.parse::<f64>().ok())?;
        let (lower, upper) = o
            .forecast_results_by_time()
            .first()
            .map(|f| {
                (
                    f.prediction_interval_lower_bound()
                        .and_then(|s| s.parse::<f64>().ok())
                        .unwrap_or(remaining),
                    f.prediction_interval_upper_bound()
                        .and_then(|s| s.parse::<f64>().ok())
                        .unwrap_or(remaining),
                )
            })
            .unwrap_or((remaining, remaining));
        Some(CostForecast {
            remaining,
            lower,
            upper,
            currency: o
                .total()
                .and_then(|t| t.unit())
                .unwrap_or("USD")
                .to_string(),
        })
    });

    Ok(CostDrilldown {
        usage_types,
        secondary_label: sec_label,
        secondary,
        forecast,
        window_label: w.cur_label,
        mtd_spent,
    })
}

impl CostGroupBy {
    fn from_dimension_key(key: &str) -> CostGroupBy {
        match key {
            "LINKED_ACCOUNT" => CostGroupBy::LinkedAccount,
            "REGION" => CostGroupBy::Region,
            "USAGE_TYPE" => CostGroupBy::UsageType,
            "TAG" => CostGroupBy::Tag,
            "COST_CATEGORY" => CostGroupBy::CostCategory,
            _ => CostGroupBy::Service,
        }
    }
}

// ── Month bucketing (3-month trend) ───────────────────────────────────────────

/// `(YYYY-MM, total)` buckets from a date-sorted daily series, oldest → newest.
/// Months with no daily entries simply don't appear.
fn monthly_buckets(daily: &[(String, f64)]) -> Vec<(String, f64)> {
    let mut out: Vec<(String, f64)> = Vec::new();
    for (date, amt) in daily {
        let ym = date.get(..7).unwrap_or(date.as_str());
        match out.last_mut() {
            Some(last) if last.0 == ym => last.1 += amt,
            _ => out.push((ym.to_string(), *amt)),
        }
    }
    out
}

/// `"2026-04"` → `"Apr"`.
fn ym_label(ym: &str) -> String {
    ym.get(5..7)
        .and_then(|m| m.parse::<usize>().ok())
        .filter(|m| (1..=12).contains(m))
        .map(|m| MONTHS[m - 1].to_string())
        .unwrap_or_else(|| ym.to_string())
}

/// Month-over-month delta over `buckets`: the last two months excluding the
/// (partial) current month `current_ym`. None without two complete months or
/// when the older one had no spend.
fn mom_delta(buckets: &[(String, f64)], current_ym: &str) -> Option<f64> {
    let full: Vec<&(String, f64)> = buckets.iter().filter(|(ym, _)| ym != current_ym).collect();
    if full.len() < 2 {
        return None;
    }
    let older = full[full.len() - 2].1;
    let newer = full[full.len() - 1].1;
    if older > 0.0 {
        Some((newer - older) / older * 100.0)
    } else {
        None
    }
}

// ── Formatting helpers ────────────────────────────────────────────────────────

/// `1234.5` → `"1,234.50"`. Reuses the `thousands` crate for grouping.
pub fn fmt_money(v: f64) -> String {
    use thousands::Separable;
    let neg = v < 0.0;
    let cents = (v.abs() * 100.0).round() as i64;
    let dollars = cents / 100;
    let frac = cents % 100;
    format!(
        "{}{}.{:02}",
        if neg { "-" } else { "" },
        dollars.separate_with_commas(),
        frac
    )
}

/// `"2026-06-17"` → `"Jun 17"`.
pub fn short_date(iso: &str) -> String {
    let parts: Vec<&str> = iso.split('-').collect();
    if parts.len() == 3 {
        if let Ok(m) = parts[1].parse::<usize>() {
            if (1..=12).contains(&m) {
                let day = parts[2].trim_start_matches('0');
                return format!("{} {}", MONTHS[m - 1], day);
            }
        }
    }
    iso.to_string()
}

/// A compact unicode block sparkline scaled to the series max.
pub fn sparkline(values: &[f64]) -> String {
    const BLOCKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let max = values.iter().cloned().fold(0.0_f64, f64::max);
    if max <= 0.0 {
        return BLOCKS[0].to_string().repeat(values.len());
    }
    values
        .iter()
        .map(|&v| {
            let idx = ((v / max) * (BLOCKS.len() - 1) as f64).round() as usize;
            BLOCKS[idx.min(BLOCKS.len() - 1)]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    // ── resolve_windows_on (the risky billing-period date math) ──────────────

    #[test]
    fn mtd_window_aligns_prior_to_same_elapsed_days() {
        // 18 days elapsed in June → prior window is the first 18 days of May.
        let w = resolve_windows_on(CostPeriod::Mtd, d("2026-06-18"));
        assert_eq!(w.cur_start, d("2026-06-01"));
        assert_eq!(w.cur_end, d("2026-06-19")); // exclusive, includes today
        assert_eq!(w.prior_start, d("2026-05-01"));
        assert_eq!(w.prior_end, d("2026-05-19")); // exclusive → May 1–18
        // current and prior cover the same number of days.
        let cur_days = (w.cur_end - w.cur_start).num_days();
        let prior_days = (w.prior_end - w.prior_start).num_days();
        assert_eq!(cur_days, prior_days);
        assert_eq!(cur_days, 18);
    }

    #[test]
    fn mtd_window_crosses_year_boundary() {
        // Mid-January → prior window reaches back into the previous December.
        let w = resolve_windows_on(CostPeriod::Mtd, d("2026-01-15"));
        assert_eq!(w.cur_start, d("2026-01-01"));
        assert_eq!(w.cur_end, d("2026-01-16"));
        assert_eq!(w.prior_start, d("2025-12-01"));
        assert_eq!(w.prior_end, d("2025-12-16"));
    }

    #[test]
    fn mtd_prior_end_clamped_to_current_month_start() {
        // Day 31 of March vs a 28-day February: the naive prior_end would spill
        // into March, so it must clamp to the current month start (full Feb).
        let w = resolve_windows_on(CostPeriod::Mtd, d("2026-03-31"));
        assert_eq!(w.prior_start, d("2026-02-01"));
        assert_eq!(w.prior_end, d("2026-03-01")); // clamped, not 2026-03-04
        assert!(w.prior_end <= w.cur_start);
    }

    #[test]
    fn last_month_uses_full_prior_two_months() {
        let w = resolve_windows_on(CostPeriod::LastMonth, d("2026-06-18"));
        assert_eq!(w.cur_start, d("2026-05-01"));
        assert_eq!(w.cur_end, d("2026-06-01")); // full May
        assert_eq!(w.prior_start, d("2026-04-01"));
        assert_eq!(w.prior_end, d("2026-05-01")); // full April
        assert!(w.prior_end > w.prior_start); // has a comparable prior
    }

    #[test]
    fn three_months_is_rolling_with_no_prior() {
        let w = resolve_windows_on(CostPeriod::Last3Months, d("2026-06-18"));
        assert_eq!(w.cur_start, d("2026-04-01")); // month_start - 2 months
        assert_eq!(w.cur_end, d("2026-06-19"));
        // No comparable prior window → prior_start == prior_end (has_prior false).
        assert_eq!(w.prior_start, w.prior_end);
        assert_eq!(w.prior_label, "—");
    }

    // ── Keyed groupings (tags / cost categories) ─────────────────────────────

    #[test]
    fn keyed_group_value_strips_the_key_and_keeps_absent_empty() {
        assert_eq!(keyed_group_value("team$payments"), "payments");
        assert_eq!(keyed_group_value("team$"), "");
        assert_eq!(keyed_group_value("Team$a$b"), "a$b", "only the first $ splits");
        assert_eq!(keyed_group_value("bare"), "bare");
    }

    #[test]
    fn row_filter_uses_tags_categories_and_absent() {
        let f = row_filter(CostGroupBy::Tag, "team", "payments");
        let t = f.tags().expect("tag filter");
        assert_eq!(t.key(), Some("team"));
        assert_eq!(t.values(), ["payments".to_string()]);
        assert!(f.dimensions().is_none());

        let f = row_filter(CostGroupBy::Tag, "team", "");
        let t = f.tags().unwrap();
        assert!(t.values().is_empty());
        assert_eq!(t.match_options(), [MatchOption::Absent]);

        let f = row_filter(CostGroupBy::CostCategory, "Team", "");
        let c = f.cost_categories().expect("category filter");
        assert_eq!(c.key(), Some("Team"));
        assert_eq!(c.match_options(), [MatchOption::Absent]);

        let f = row_filter(CostGroupBy::Region, "", "us-east-1");
        assert_eq!(f.dimensions().unwrap().key(), Some(&Dimension::Region));
    }

    #[test]
    fn keyed_dimension_keys_round_trip() {
        for g in [CostGroupBy::Tag, CostGroupBy::CostCategory, CostGroupBy::UsageType] {
            assert_eq!(CostGroupBy::from_dimension_key(g.dimension_key()), g);
        }
    }

    // ── CostLineItem derived metrics ─────────────────────────────────────────

    fn item(current: f64, prior: f64, has_prior: bool, daily: Vec<(String, f64)>) -> CostLineItem {
        CostLineItem {
            dimension: "SERVICE".into(),
            group_key: String::new(),
            filter_value: "Amazon EC2".into(),
            key: "Amazon EC2".into(),
            current,
            prior,
            currency: "USD".into(),
            daily,
            period_label: "Jun 1–18".into(),
            prior_label: "May 1–18".into(),
            has_prior,
            mom_delta_pct: None,
            summary: String::new(),
            tags: HashMap::new(),
        }
    }

    #[test]
    fn delta_pct_handles_prior_zero_and_absent() {
        assert_eq!(item(150.0, 100.0, true, vec![]).delta_pct(), Some(50.0));
        assert_eq!(item(50.0, 100.0, true, vec![]).delta_pct(), Some(-50.0));
        // No comparable prior, or prior was zero → undefined (None), no div-by-zero.
        assert_eq!(item(150.0, 100.0, false, vec![]).delta_pct(), None);
        assert_eq!(item(150.0, 0.0, true, vec![]).delta_pct(), None);
    }

    #[test]
    fn daily_avg_divides_by_days_with_spend_only() {
        let daily = vec![
            ("2026-06-01".into(), 10.0),
            ("2026-06-02".into(), 0.0), // zero-spend day excluded from the divisor
            ("2026-06-03".into(), 20.0),
        ];
        // 30 total over 2 active days = 15.0 (not 10.0 across all 3).
        assert_eq!(item(30.0, 0.0, false, daily).daily_avg(), 15.0);
        assert_eq!(item(0.0, 0.0, false, vec![]).daily_avg(), 0.0);
    }

    #[test]
    fn trend_label_buckets_by_threshold() {
        assert_eq!(item(150.0, 100.0, true, vec![]).trend_label(), "↑50%");
        assert_eq!(item(50.0, 100.0, true, vec![]).trend_label(), "↓50%");
        assert_eq!(item(100.0, 100.0, true, vec![]).trend_label(), "≈"); // within ±0.5%
        assert_eq!(item(150.0, 0.0, true, vec![]).trend_label(), "new"); // prior zero
        assert_eq!(item(150.0, 100.0, false, vec![]).trend_label(), ""); // no prior at all
    }

    #[test]
    fn trend_falls_back_to_mom_delta_when_no_prior_window() {
        // 3-month view: no prior window, but a MoM delta keeps the signal.
        let mut i = item(150.0, 0.0, false, vec![]);
        i.mom_delta_pct = Some(25.0);
        assert_eq!(i.trend_label(), "↑25%");
        assert!(matches!(i.state(), ResourceState::Unavailable)); // up ≥5% = red
        i.mom_delta_pct = Some(-25.0);
        assert_eq!(i.trend_label(), "↓25%");
        // A real prior-window delta wins over MoM.
        let mut j = item(150.0, 100.0, true, vec![]);
        j.mom_delta_pct = Some(-99.0);
        assert_eq!(j.trend_label(), "↑50%");
    }

    // ── Month bucketing (3-month trend) ──────────────────────────────────────

    #[test]
    fn monthly_buckets_groups_a_sorted_daily_series() {
        let daily = vec![
            ("2026-04-01".to_string(), 1.0),
            ("2026-04-15".to_string(), 2.0),
            ("2026-05-01".to_string(), 4.0),
            ("2026-06-01".to_string(), 8.0),
        ];
        assert_eq!(
            monthly_buckets(&daily),
            vec![
                ("2026-04".to_string(), 3.0),
                ("2026-05".to_string(), 4.0),
                ("2026-06".to_string(), 8.0),
            ]
        );
        assert!(monthly_buckets(&[]).is_empty());
    }

    #[test]
    fn mom_delta_compares_last_two_complete_months() {
        let buckets = vec![
            ("2026-04".to_string(), 100.0),
            ("2026-05".to_string(), 150.0),
            ("2026-06".to_string(), 10.0), // partial current month — excluded
        ];
        assert_eq!(mom_delta(&buckets, "2026-06"), Some(50.0));
        // Group with no spend this month: the newest bucket IS complete.
        let dead = vec![
            ("2026-04".to_string(), 100.0),
            ("2026-05".to_string(), 50.0),
        ];
        assert_eq!(mom_delta(&dead, "2026-06"), Some(-50.0));
        // Fewer than two complete months, or zero older spend → None.
        assert_eq!(mom_delta(&buckets[1..], "2026-06"), None);
        let zero_older = vec![
            ("2026-04".to_string(), 0.0),
            ("2026-05".to_string(), 50.0),
        ];
        assert_eq!(mom_delta(&zero_older, "2026-06"), None);
    }

    #[test]
    fn ym_label_names_months_and_passes_through_garbage() {
        assert_eq!(ym_label("2026-04"), "Apr");
        assert_eq!(ym_label("2026-12"), "Dec");
        assert_eq!(ym_label("garbage"), "garbage");
    }

    // ── Formatting helpers ───────────────────────────────────────────────────

    #[test]
    fn fmt_money_groups_thousands_and_rounds_cents() {
        assert_eq!(fmt_money(0.0), "0.00");
        assert_eq!(fmt_money(1234.5), "1,234.50");
        assert_eq!(fmt_money(1234567.899), "1,234,567.90"); // rounds up
        assert_eq!(fmt_money(-42.005), "-42.01");
    }

    #[test]
    fn short_date_formats_iso_and_passes_through_garbage() {
        assert_eq!(short_date("2026-06-17"), "Jun 17");
        assert_eq!(short_date("2026-01-01"), "Jan 1"); // strips leading zero
        assert_eq!(short_date("not-a-date"), "not-a-date");
    }

    #[test]
    fn sparkline_scales_to_max_and_handles_empty_or_flat() {
        assert_eq!(sparkline(&[0.0, 5.0, 10.0]).chars().count(), 3);
        assert_eq!(sparkline(&[0.0, 5.0, 10.0]).chars().last(), Some('█')); // max → full block
        assert_eq!(sparkline(&[0.0, 0.0]), "▁▁"); // all-zero → floor, no div-by-zero
        assert_eq!(sparkline(&[]), ""); // empty input
    }

    // ── Anomalies ─────────────────────────────────────────────────────────────

    fn anomaly(id: &str, start: &str, end: Option<&str>, impact: f64) -> CostAnomaly {
        use aws_sdk_costexplorer::types::{
            Anomaly, AnomalyScore, Impact, RootCause, RootCauseImpact,
        };
        let mut b = Anomaly::builder()
            .anomaly_id(id)
            .monitor_arn("arn:aws:ce::123456789012:anomalymonitor/m-1")
            .anomaly_start_date(format!("{}T00:00:00Z", start))
            .anomaly_score(AnomalyScore::builder().max_score(0.8).current_score(0.2).build())
            .impact(
                Impact::builder()
                    .max_impact(impact / 2.0)
                    .total_impact(impact)
                    .total_impact_percentage(50.0)
                    .build(),
            )
            .root_causes(
                RootCause::builder()
                    .service("Amazon CloudWatch")
                    .impact(RootCauseImpact::builder().contribution(1.0).build())
                    .build(),
            )
            .root_causes(
                RootCause::builder()
                    .service("Amazon Elastic Container Service")
                    .usage_type("USE1-Fargate-vCPU-Hours:perCPU")
                    .impact(RootCauseImpact::builder().contribution(9.0).build())
                    .build(),
            );
        if let Some(e) = end {
            b = b.anomaly_end_date(e);
        }
        CostAnomaly::from_sdk(&b.build().unwrap())
    }

    #[test]
    fn anomaly_without_dimension_is_named_by_its_top_root_cause() {
        let a = anomaly("a-1", "2026-09-28", None, 96.4);
        // Root causes sort by contribution, so ECS (9.0) leads CloudWatch (1.0).
        assert_eq!(a.root_causes[0].service.as_deref(), Some("Amazon Elastic Container Service"));
        assert_eq!(a.name(), "Amazon Elastic Container Service");
        assert_eq!(a.start_date.as_deref(), Some("2026-09-28"), "time part dropped");
        assert!(a.is_ongoing());
        assert_eq!(a.state_label(), "ongoing");
        assert_eq!(a.summary(), "$96.40 over (+50%) · Sep 28 – ongoing");
        assert!(a.search_text().contains("Fargate"), "root-cause usage types are searchable");
    }

    #[test]
    fn anomaly_state_colours_by_impact() {
        assert_eq!(anomaly("a", "2026-09-01", Some("2026-09-02"), 250.0).state(), ResourceState::Unavailable);
        assert_eq!(anomaly("b", "2026-09-01", Some("2026-09-02"), 25.0).state(), ResourceState::Pending);
        assert!(matches!(
            anomaly("c", "2026-09-01", Some("2026-09-02"), 2.0).state(),
            ResourceState::Unknown(_)
        ));
        let closed = anomaly("d", "2026-09-01", Some("2026-09-01"), 2.0);
        assert_eq!(closed.state_label(), "closed");
        assert_eq!(closed.date_range(), "Sep 1", "a one-day anomaly shows one date");
    }

    #[test]
    fn anomalies_sort_newest_first_then_by_impact() {
        let mut v = vec![
            anomaly("old", "2026-08-01", Some("2026-08-03"), 500.0),
            anomaly("new-small", "2026-09-20", None, 5.0),
            anomaly("new-big", "2026-09-20", None, 50.0),
        ];
        sort_anomalies(&mut v);
        let ids: Vec<&str> = v.iter().map(|a| a.anomaly_id.as_str()).collect();
        assert_eq!(ids, ["new-big", "new-small", "old"]);
    }
}
