//! Metrics charted from a spec table (`m`): a kind lists its CloudWatch
//! metrics here — name, label, statistic, unit — and one fetch plus one
//! renderer (`ui::widgets::spec_metrics`) does the rest, instead of a
//! hand-built fetcher, state map, event and chart page per kind. Ported from
//! nebaz's `azure::metrics`; CloudWatch needs more per metric than Azure
//! Monitor does (a namespace and a dimension set, and no units come back),
//! so the spec carries them.
//!
//! One `GetMetricData` call per open charts the whole set (billed per metric
//! requested, like `GetMetricStatistics` per request), and each metric that
//! fails says so on its own row. `a` lists what `ListMetrics` knows for the
//! resource's dimensions — every namespace, so a CloudWatch agent's
//! `CWAgent` metrics show up next to the AWS ones.

use crate::aws::services::ec2::MetricsTimeRange;
use aws_sdk_cloudwatch::primitives::DateTime as CwDateTime;
use aws_sdk_cloudwatch::types::{
    Dimension, DimensionFilter, Metric, MetricDataQuery, MetricStat, ScanBy, StatusCode,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stat {
    Average,
    Sum,
    Maximum,
    Minimum,
    P99,
}

impl Stat {
    /// The order `s` cycles through.
    pub const ALL: [Stat; 5] = [Stat::Average, Stat::Maximum, Stat::Sum, Stat::Minimum, Stat::P99];

    fn api(self) -> &'static str {
        match self {
            Stat::Average => "Average",
            Stat::Sum => "Sum",
            Stat::Maximum => "Maximum",
            Stat::Minimum => "Minimum",
            Stat::P99 => "p99",
        }
    }

    pub fn short(self) -> &'static str {
        match self {
            Stat::Average => "avg",
            Stat::Sum => "sum",
            Stat::Maximum => "max",
            Stat::Minimum => "min",
            Stat::P99 => "p99",
        }
    }

    pub fn next(self) -> Stat {
        let i = Self::ALL.iter().position(|s| *s == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }
}

/// How a value reads. CloudWatch's `GetMetricData` returns no units, so the
/// spec states them; a metric picked from `a` is `Raw`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Percent,
    Bytes,
    BytesPerSec,
    Count,
    PerSec,
    Millis,
    Seconds,
    Raw,
}

/// One curated metric of a kind.
#[derive(Debug, Clone, Copy)]
pub struct MetricSpec {
    pub metric: &'static str,
    pub label: &'static str,
    pub stat: Stat,
    pub unit: Unit,
    /// Divide each point by the period: a `Sum` of bytes becomes bytes/s.
    pub rate: bool,
    /// Metrics sharing a group draw on one chart when either is selected
    /// (in beside out), each still its own row.
    pub group: Option<&'static str>,
}

const fn m(metric: &'static str, label: &'static str, stat: Stat, unit: Unit) -> MetricSpec {
    MetricSpec { metric, label, stat, unit, rate: false, group: None }
}

impl MetricSpec {
    const fn rate(self) -> Self {
        MetricSpec { rate: true, ..self }
    }

    const fn group(self, group: &'static str) -> Self {
        MetricSpec { group: Some(group), ..self }
    }
}

use Stat::{Average as Avg, Maximum as Max, Sum};

pub const EC2_INSTANCE: &[MetricSpec] = &[
    m("CPUUtilization", "CPU", Avg, Unit::Percent),
    m("NetworkIn", "Network in", Sum, Unit::BytesPerSec).rate().group("net"),
    m("NetworkOut", "Network out", Sum, Unit::BytesPerSec).rate().group("net"),
    m("StatusCheckFailed", "Status check failed", Max, Unit::Count),
];

pub const LAMBDA_FUNCTION: &[MetricSpec] = &[
    m("Invocations", "Invocations", Sum, Unit::Count).group("calls"),
    m("Errors", "Errors", Sum, Unit::Count).group("calls"),
    m("Throttles", "Throttles", Sum, Unit::Count).group("calls"),
    m("Duration", "Duration", Avg, Unit::Millis),
    m("ConcurrentExecutions", "Concurrency", Max, Unit::Count),
];

pub const RDS_DATABASE: &[MetricSpec] = &[
    m("CPUUtilization", "CPU", Avg, Unit::Percent),
    m("DatabaseConnections", "Connections", Avg, Unit::Count),
    m("ReadIOPS", "Read IOPS", Avg, Unit::PerSec).group("iops"),
    m("WriteIOPS", "Write IOPS", Avg, Unit::PerSec).group("iops"),
    m("ReadLatency", "Read latency", Avg, Unit::Seconds).group("latency"),
    m("WriteLatency", "Write latency", Avg, Unit::Seconds).group("latency"),
    m("FreeableMemory", "Freeable memory", Avg, Unit::Bytes),
    m("FreeStorageSpace", "Free storage", Avg, Unit::Bytes),
];

/// What `m` charts for one resource: where its metrics live and its curated set.
#[derive(Debug, Clone)]
pub struct MetricTarget {
    /// `EC2 instance · web-1`, the view's title.
    pub title: String,
    pub namespace: &'static str,
    pub dimensions: Vec<(String, String)>,
    pub specs: &'static [MetricSpec],
}

impl MetricTarget {
    pub fn curated_queries(&self) -> Vec<MetricQuery> {
        self.specs
            .iter()
            .map(|s| MetricQuery {
                namespace: self.namespace.to_string(),
                metric: s.metric.to_string(),
                dimensions: self.dimensions.clone(),
                label: s.label.to_string(),
                stat: s.stat,
                unit: s.unit,
                rate: s.rate,
                group: s.group.map(str::to_string),
            })
            .collect()
    }
}

/// One series to fetch: a curated spec resolved against a resource, or a
/// metric picked from `a`.
#[derive(Debug, Clone, PartialEq)]
pub struct MetricQuery {
    pub namespace: String,
    pub metric: String,
    pub dimensions: Vec<(String, String)>,
    pub label: String,
    pub stat: Stat,
    pub unit: Unit,
    pub rate: bool,
    pub group: Option<String>,
}

/// One metric's answer: (epoch seconds, value) points oldest first, or why not.
#[derive(Debug, Clone, PartialEq)]
pub struct MetricSeries {
    pub query: MetricQuery,
    pub points: Vec<(i64, f64)>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stats {
    pub last: f64,
    pub min: f64,
    pub max: f64,
    pub avg: f64,
}

impl MetricSeries {
    pub fn stats(&self) -> Option<Stats> {
        let last = self.points.last()?.1;
        let (mut min, mut max, mut sum) = (f64::MAX, f64::MIN, 0.0);
        for &(_, v) in &self.points {
            min = min.min(v);
            max = max.max(v);
            sum += v;
        }
        Some(Stats { last, min, max, avg: sum / self.points.len() as f64 })
    }

    /// `CPU (avg CPUUtilization)  last 41%  min 3%  max 88%  avg 27%` — the
    /// `y` copy and the `e` editor view.
    pub fn summary(&self) -> String {
        let q = &self.query;
        let head = format!("{} ({} {})", q.label, q.stat.short(), q.metric);
        match (&self.error, self.stats()) {
            (Some(e), _) => format!("{head}  ! {e}"),
            (None, None) => format!("{head}  no data in the window"),
            (None, Some(s)) => format!(
                "{head}  last {}  min {}  max {}  avg {}",
                fmt_value(s.last, q.unit),
                fmt_value(s.min, q.unit),
                fmt_value(s.max, q.unit),
                fmt_value(s.avg, q.unit)
            ),
        }
    }
}

/// A short, row-sized form of an AWS error.
pub fn short_error(e: &str) -> String {
    if e.contains("not authorized") || e.contains("AccessDenied") {
        return "no access (cloudwatch:GetMetricData)".to_string();
    }
    let first = e.split(". ").next().unwrap_or(e);
    if first.chars().count() > 80 {
        first.chars().take(79).chain(['…']).collect()
    } else {
        first.to_string()
    }
}

fn dims(pairs: &[(String, String)]) -> Vec<Dimension> {
    pairs.iter().map(|(n, v)| Dimension::builder().name(n).value(v).build()).collect()
}

/// Every query's series over `[end − range, end]`, in the order asked, from
/// one `GetMetricData` call (paged). A failed call fails every row with its
/// message; a query the response doesn't answer is a row with an error.
pub async fn fetch_series(
    cw: &aws_sdk_cloudwatch::Client,
    queries: &[MetricQuery],
    range: MetricsTimeRange,
    end: i64,
) -> Vec<MetricSeries> {
    let period = range.period_secs();
    let start = end - range.duration_secs();
    let data_queries: Vec<MetricDataQuery> = queries
        .iter()
        .enumerate()
        .map(|(i, q)| {
            let metric = Metric::builder()
                .namespace(&q.namespace)
                .metric_name(&q.metric)
                .set_dimensions(Some(dims(&q.dimensions)))
                .build();
            let stat = MetricStat::builder().metric(metric).period(period).stat(q.stat.api()).build();
            MetricDataQuery::builder().id(format!("q{i}")).metric_stat(stat).build()
        })
        .collect();

    let mut points: Vec<Vec<(i64, f64)>> = vec![Vec::new(); queries.len()];
    let mut errors: Vec<Option<String>> = vec![Some("not in the response".to_string()); queries.len()];
    let mut token: Option<String> = None;
    loop {
        let resp = cw
            .get_metric_data()
            .set_metric_data_queries(Some(data_queries.clone()))
            .start_time(CwDateTime::from_secs(start))
            .end_time(CwDateTime::from_secs(end))
            .scan_by(ScanBy::TimestampAscending)
            .set_next_token(token.clone())
            .send()
            .await;
        let resp = match resp {
            Ok(r) => r,
            Err(e) => {
                let msg = short_error(&crate::error::sdk_error_message(&e));
                return queries
                    .iter()
                    .map(|q| MetricSeries { query: q.clone(), points: Vec::new(), error: Some(msg.clone()) })
                    .collect();
            }
        };
        for r in resp.metric_data_results() {
            let Some(i) = r.id().and_then(|id| id.strip_prefix('q')).and_then(|n| n.parse::<usize>().ok()) else {
                continue;
            };
            let Some(q) = queries.get(i) else { continue };
            errors[i] = match r.status_code() {
                Some(StatusCode::InternalError) | Some(StatusCode::Forbidden) => Some(
                    r.messages()
                        .first()
                        .and_then(|m| m.value())
                        .map(short_error)
                        .unwrap_or_else(|| "CloudWatch could not read it".to_string()),
                ),
                _ => None,
            };
            let divisor = if q.rate { period as f64 } else { 1.0 };
            points[i].extend(r.timestamps().iter().zip(r.values()).map(|(t, v)| (t.secs(), v / divisor)));
        }
        token = crate::aws::pagination::next_page_token(resp.next_token(), &token);
        if token.is_none() {
            break;
        }
    }
    queries
        .iter()
        .zip(points.into_iter().zip(errors))
        .map(|(q, (mut pts, error))| {
            pts.sort_by_key(|(t, _)| *t);
            MetricSeries { query: q.clone(), points: pts, error }
        })
        .collect()
}

/// One entry of the `a` picker: a metric `ListMetrics` knows for the resource.
#[derive(Debug, Clone, PartialEq)]
pub struct MetricDefinition {
    pub namespace: String,
    pub metric: String,
    pub dimensions: Vec<(String, String)>,
}

impl MetricDefinition {
    /// The definition as a query: `Average`, raw values (`s` changes the stat).
    pub fn query(&self) -> MetricQuery {
        MetricQuery {
            namespace: self.namespace.clone(),
            metric: self.metric.clone(),
            dimensions: self.dimensions.clone(),
            label: self.metric.clone(),
            stat: Stat::Average,
            unit: Unit::Raw,
            rate: false,
            group: None,
        }
    }
}

/// The picker holds at most this many metrics (five `ListMetrics` pages).
pub const MAX_DEFINITIONS: usize = 2_500;

/// Every metric carrying the resource's dimensions, in any namespace, sorted
/// namespace → fewest extra dimensions → name.
pub async fn list_definitions(
    cw: &aws_sdk_cloudwatch::Client,
    dimensions: &[(String, String)],
) -> Result<Vec<MetricDefinition>, String> {
    let filters: Vec<DimensionFilter> =
        dimensions.iter().map(|(n, v)| DimensionFilter::builder().name(n).value(v).build()).collect();
    let mut out: Vec<MetricDefinition> = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..5 {
        let resp = cw
            .list_metrics()
            .set_dimensions(Some(filters.clone()))
            .set_next_token(token.clone())
            .send()
            .await
            .map_err(|e| crate::error::sdk_error_message(&e))?;
        for metric in resp.metrics() {
            out.push(MetricDefinition {
                namespace: metric.namespace().unwrap_or_default().to_string(),
                metric: metric.metric_name().unwrap_or_default().to_string(),
                dimensions: metric
                    .dimensions()
                    .iter()
                    .map(|d| (d.name().unwrap_or_default().to_string(), d.value().unwrap_or_default().to_string()))
                    .collect(),
            });
        }
        token = crate::aws::pagination::next_page_token(resp.next_token(), &token);
        if token.is_none() || out.len() >= MAX_DEFINITIONS {
            break;
        }
    }
    out.sort_by(|a, b| {
        (&a.namespace, a.dimensions.len(), &a.metric, &a.dimensions)
            .cmp(&(&b.namespace, b.dimensions.len(), &b.metric, &b.dimensions))
    });
    out.dedup();
    out.truncate(MAX_DEFINITIONS);
    Ok(out)
}

/// `41%`, `2.1 MB/s`, `340 ms`, `1.2k`.
pub fn fmt_value(v: f64, unit: Unit) -> String {
    fn si(v: f64) -> String {
        let a = v.abs();
        if a >= 1e9 {
            format!("{:.1}G", v / 1e9)
        } else if a >= 1e6 {
            format!("{:.1}M", v / 1e6)
        } else if a >= 1e4 {
            format!("{:.1}k", v / 1e3)
        } else if a >= 100.0 || v.fract() == 0.0 {
            format!("{v:.0}")
        } else if a >= 10.0 {
            format!("{v:.1}")
        } else {
            format!("{v:.2}")
        }
    }
    fn bytes(v: f64) -> String {
        const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
        let mut v = v;
        let mut i = 0;
        while v.abs() >= 1024.0 && i < U.len() - 1 {
            v /= 1024.0;
            i += 1;
        }
        if i == 0 { format!("{v:.0} B") } else { format!("{v:.1} {}", U[i]) }
    }
    match unit {
        Unit::Percent => format!("{v:.1}%"),
        Unit::Bytes => bytes(v),
        Unit::BytesPerSec => format!("{}/s", bytes(v)),
        Unit::Count | Unit::Raw => si(v),
        Unit::PerSec => format!("{}/s", si(v)),
        Unit::Millis if v >= 1000.0 => format!("{:.2} s", v / 1000.0),
        Unit::Millis => format!("{v:.0} ms"),
        Unit::Seconds if v < 1.0 => format!("{:.1} ms", v * 1000.0),
        Unit::Seconds => format!("{v:.2} s"),
    }
}

/// `1`–`4` pick a range in the view.
pub fn range_from_digit(c: char) -> Option<MetricsTimeRange> {
    Some(match c {
        '1' => MetricsTimeRange::OneHour,
        '2' => MetricsTimeRange::SixHours,
        '3' => MetricsTimeRange::TwentyFourHours,
        '4' => MetricsTimeRange::SevenDays,
        _ => return None,
    })
}

pub const RANGES: [MetricsTimeRange; 4] = [
    MetricsTimeRange::OneHour,
    MetricsTimeRange::SixHours,
    MetricsTimeRange::TwentyFourHours,
    MetricsTimeRange::SevenDays,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_read_in_their_unit() {
        assert_eq!(fmt_value(41.234, Unit::Percent), "41.2%");
        assert_eq!(fmt_value(2.0 * 1024.0 * 1024.0, Unit::BytesPerSec), "2.0 MB/s");
        assert_eq!(fmt_value(340.4, Unit::Millis), "340 ms");
        assert_eq!(fmt_value(0.0042, Unit::Seconds), "4.2 ms");
        assert_eq!(fmt_value(12_345.0, Unit::Count), "12.3k");
        assert_eq!(fmt_value(3.0, Unit::Count), "3");
    }

    #[test]
    fn a_target_resolves_its_specs_against_the_resource() {
        let t = MetricTarget {
            title: "EC2 instance · web".into(),
            namespace: "AWS/EC2",
            dimensions: vec![("InstanceId".into(), "i-1".into())],
            specs: EC2_INSTANCE,
        };
        let q = t.curated_queries();
        assert_eq!(q.len(), EC2_INSTANCE.len());
        let net_in = q.iter().find(|q| q.metric == "NetworkIn").unwrap();
        assert!(net_in.rate && net_in.group.as_deref() == Some("net"));
        assert_eq!(net_in.dimensions, vec![("InstanceId".to_string(), "i-1".to_string())]);
    }

    #[test]
    fn a_denied_call_reads_as_no_access() {
        let e = "User: arn:aws:sts::1:assumed-role/r is not authorized to perform: cloudwatch:GetMetricData";
        assert_eq!(short_error(e), "no access (cloudwatch:GetMetricData)");
        assert_eq!(Stat::P99.next(), Stat::Average, "s wraps around");
    }
}
