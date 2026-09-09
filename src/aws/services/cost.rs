use crate::aws::client::AwsClients;
use crate::aws::resource::{Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use aws_sdk_costexplorer::types::{
    DateInterval, Dimension, DimensionValues, Expression, Granularity, GroupDefinition,
    GroupDefinitionType, Metric,
};
use aws_sdk_costexplorer::Client as CeClient;
use chrono::{Datelike, Duration as ChronoDuration, Months, NaiveDate, Utc};
use std::any::Any;
use std::collections::HashMap;
use tokio::sync::mpsc;

/// Cost & Billing service, backed by AWS Cost Explorer (`ce`).
///
/// The list groups unblended spend by a selectable dimension (service / linked
/// account / region / usage type) over a selectable period (MTD / last month /
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
        let items = fetch_cost(&self.client, self.query).await?;
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
        match fetch_cost(&self.client, self.query).await {
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
                    error: friendly_error(&e.to_string()),
                });
                Ok(())
            }
        }
    }

    async fn get_resource_details(&self, _id: &str) -> Result<Box<dyn Resource>> {
        Err(crate::error::Error::NotImplemented)
    }
}

/// Map raw Cost Explorer SDK errors to actionable hints.
fn friendly_error(raw: &str) -> String {
    let low = raw.to_lowercase();
    if low.contains("not subscribed") || low.contains("not enabled") {
        "Cost Explorer is not enabled for this account. Enable it in the Billing console (takes ~24h to populate).".to_string()
    } else if low.contains("accessdenied") || low.contains("access denied") || low.contains("not authorized") {
        "Access denied — need ce:GetCostAndUsage. Cost Explorer must also be enabled by the management/payer account.".to_string()
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
}

impl CostGroupBy {
    /// Cost Explorer `GroupBy` dimension key.
    pub fn dimension_key(&self) -> &'static str {
        match self {
            CostGroupBy::Service => "SERVICE",
            CostGroupBy::LinkedAccount => "LINKED_ACCOUNT",
            CostGroupBy::Region => "REGION",
            CostGroupBy::UsageType => "USAGE_TYPE",
        }
    }

    /// SDK `Dimension` enum (for filter expressions).
    fn dimension(&self) -> Dimension {
        match self {
            CostGroupBy::Service => Dimension::Service,
            CostGroupBy::LinkedAccount => Dimension::LinkedAccount,
            CostGroupBy::Region => Dimension::Region,
            CostGroupBy::UsageType => Dimension::UsageType,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            CostGroupBy::Service => "Service",
            CostGroupBy::LinkedAccount => "Account",
            CostGroupBy::Region => "Region",
            CostGroupBy::UsageType => "Usage Type",
        }
    }

    fn noun(&self) -> &'static str {
        match self {
            CostGroupBy::Service => "service",
            CostGroupBy::LinkedAccount => "account",
            CostGroupBy::Region => "region",
            CostGroupBy::UsageType => "usage type",
        }
    }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CostQuery {
    pub group_by: CostGroupBy,
    pub period: CostPeriod,
}

impl Default for CostQuery {
    fn default() -> Self {
        Self {
            group_by: CostGroupBy::Service,
            period: CostPeriod::Mtd,
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
    /// Cost Explorer dimension key this row was grouped by (e.g. `"SERVICE"`).
    pub dimension: String,
    /// The grouped value — service name, account id, region, or usage type.
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
pub async fn fetch_cost(client: &CeClient, query: CostQuery) -> Result<Vec<CostLineItem>> {
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

    let group_by = GroupDefinition::builder()
        .r#type(GroupDefinitionType::Dimension)
        .key(query.group_by.dimension_key())
        .build();

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
        .map(|(key, (current, prior, mut daily, currency))| {
            daily.sort_by(|a, b| a.0.cmp(&b.0));
            let mut item = CostLineItem {
                dimension: dimension.clone(),
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

/// Build a single-dimension filter expression (`dimension = key`).
fn dim_filter(dimension: Dimension, value: &str) -> Result<Expression> {
    let dv = DimensionValues::builder()
        .key(dimension)
        .values(value.to_string())
        .build();
    Ok(Expression::builder().dimensions(dv).build())
}

/// Fetch the usage-type + secondary breakdowns (over the active period's
/// window, matching the list totals) and the current-month-end forecast for
/// one row, all filtered to `dimension = key`. Three concurrent CE calls,
/// plus a fourth (this row's MTD spend, the forecast baseline) when the
/// period's window isn't the current month.
pub async fn fetch_cost_drilldown(
    client: CeClient,
    dimension_key: String,
    key: String,
    period: CostPeriod,
) -> Result<CostDrilldown> {
    let group_by = CostGroupBy::from_dimension_key(&dimension_key);
    let dim = group_by.dimension();

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
    let (sec_key, sec_label): (&str, &'static str) = match group_by {
        CostGroupBy::Region => ("SERVICE", "Services"),
        _ => ("REGION", "Regions"),
    };

    let filter = dim_filter(dim.clone(), &key)?;

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

    // ── CostLineItem derived metrics ─────────────────────────────────────────

    fn item(current: f64, prior: f64, has_prior: bool, daily: Vec<(String, f64)>) -> CostLineItem {
        CostLineItem {
            dimension: "SERVICE".into(),
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
}
