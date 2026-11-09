//! `neboto open` and `--bookmark`: start the TUI at a resource, the public
//! way in to what `--at` does for a `|` split (#331).
//!
//! `open`'s arguments are the stable contract (`docs/HEADLESS.md`); the
//! `NavLocation` they build is not. A place is resolved here, before the
//! TUI takes the screen, as far as it can be without a fetch — the service,
//! the chart range — so a typo exits with a message instead of opening a
//! blank pane. The rest (which row, which sub-tab, which section) resolves
//! once the service's list streams in (`App::resolve_pending_jump`).

use crate::app::{JumpView, NavLens, NavLocation};
use crate::aws::service::ServiceType;
use crate::aws::services::ec2::MetricsTimeRange;
use crate::cli::OpenArgs;

/// A chart range as `--charts` spells it: the labels the charts' own range
/// row shows (`1h`, `6h`, `24h`, `7d`, `30d`).
pub fn parse_range(s: &str) -> Result<MetricsTimeRange, String> {
    Ok(match s.trim().to_ascii_lowercase().as_str() {
        "1h" => MetricsTimeRange::OneHour,
        "6h" => MetricsTimeRange::SixHours,
        "24h" | "1d" => MetricsTimeRange::TwentyFourHours,
        "7d" | "1w" => MetricsTimeRange::SevenDays,
        "30d" => MetricsTimeRange::ThirtyDays,
        other => return Err(format!("--charts: unknown range {other:?} — 1h, 6h, 24h, 7d or 30d")),
    })
}

/// What a startup place asks for beyond the place itself: checked once the
/// row resolves, so a wrong section or a type without charts says so.
#[derive(Debug, Clone, Default)]
pub struct OpenRequest {
    /// The id or name as given, for the "not found" message.
    pub query: String,
    /// The section name as given (any case); matched against the
    /// resource's own sections once it's selected.
    pub section: Option<String>,
    /// `--charts` was asked for.
    pub charts: bool,
    /// `--full`: the detail pane (or the charts) takes the whole width.
    pub full: bool,
    /// The row may sit on any sub-tab: find it among the service's rows
    /// and show its tab. Off for a bookmark, which records its tab.
    pub seek_tab: bool,
}

/// `neboto open …` → the place to start at, plus what to check once there.
pub fn plan(args: &OpenArgs) -> Result<(NavLocation, OpenRequest), String> {
    let (service, view, id) = match args.target.as_slice() {
        [svc, id] => {
            let service = crate::headless::resolve_service(svc).map_err(|f| f.message().to_string())?;
            // An ARN given with its service may still name the sub-tab (and
            // reduce to the name the row is keyed by).
            match target_of(id) {
                Some((s, view, id)) if s == service => (service, view, id),
                _ => (service, JumpView::None, id.trim().to_string()),
            }
        }
        [id] => target_of(id).ok_or_else(|| {
            format!(
                "open: can't tell which service {id:?} belongs to — give it first, e.g. `neboto open ec2 {id}`"
            )
        })?,
        _ => return Err("open: expected `<service> <id>` or an ARN".into()),
    };
    if service.is_virtual() {
        return Err(format!("open: {} has no resources of its own to open", service.name()));
    }
    let range = args.charts.as_deref().map(parse_range).transpose()?;
    let lens = (args.charts.is_some() || args.metric.is_some())
        .then(|| NavLens::Metrics { range, metric: args.metric.clone(), only: args.only });
    let loc = NavLocation {
        service,
        view,
        query: String::new(),
        selected_id: Some(id.clone()),
        label: format!("{} · {id}", service.name()),
        s3_object_path: None,
        details_focused: true,
        // Matched (any case) once the resource is selected; the restore
        // path wants the exact label, so it's set there, not here.
        detail_section: None,
        region: None,
        credentials: None,
        lens,
    };
    let req = OpenRequest {
        query: id,
        section: args.section.clone(),
        charts: args.charts.is_some() || args.metric.is_some(),
        full: args.full,
        seek_tab: true,
    };
    Ok((loc, req))
}

/// The service, sub-tab and row id an id or ARN on its own points at — the
/// classifiers `Enter` follows, so `open` knows every reference a detail
/// row can jump to.
fn target_of(value: &str) -> Option<(ServiceType, JumpView, String)> {
    let value = value.trim();
    let t = crate::ui::widgets::details_pane::resource_jump_target("", value, ServiceType::EC2)?;
    Some((t.service, t.view, t.id))
}

/// `--bookmark NAME`: the saved bookmark NAME picks. Matched against each
/// bookmark's label (`EC2 · web-3 (i-…)`) and the id it holds, any case:
/// an exact label or id first, then a label containing NAME — which must be
/// the only one, or the candidates are listed.
pub fn pick_bookmark<'a>(bookmarks: &'a [NavLocation], name: &str) -> Result<&'a NavLocation, String> {
    let want = name.trim().to_lowercase();
    if want.is_empty() {
        return Err("--bookmark: give part of a bookmark's name".into());
    }
    let exact = |b: &&NavLocation| {
        b.label.to_lowercase() == want || b.selected_id.as_deref().is_some_and(|id| id.to_lowercase() == want)
    };
    if let Some(b) = bookmarks.iter().find(exact) {
        return Ok(b);
    }
    let hits: Vec<&NavLocation> = bookmarks.iter().filter(|b| b.label.to_lowercase().contains(&want)).collect();
    match hits.as_slice() {
        [one] => Ok(one),
        [] if bookmarks.is_empty() => Err("--bookmark: no bookmarks saved yet (B in the TUI saves one)".into()),
        [] => Err(format!("--bookmark: none match {name:?}:{}", labels(bookmarks.iter()))),
        many => Err(format!("--bookmark: {name:?} matches {}:{}", many.len(), labels(many.iter().copied()))),
    }
}

fn labels<'a>(it: impl Iterator<Item = &'a NavLocation>) -> String {
    let all: Vec<&str> = it.map(|b| b.label.as_str()).collect();
    const SHOW: usize = 8;
    let mut s: String = all.iter().take(SHOW).map(|l| format!("\n  {l}")).collect();
    if all.len() > SHOW {
        s.push_str(&format!("\n  … {} more", all.len() - SHOW));
    }
    s
}

/// The chart row `wanted` names: a row's label or CloudWatch metric name
/// (any case), else the one label it starts — or the message listing them.
pub fn match_metric(series: &[crate::aws::metrics::MetricSeries], wanted: &str) -> Result<usize, String> {
    let w = wanted.trim().to_lowercase();
    // Hidden rows feed a computed one and never take the cursor.
    let shown: Vec<usize> = (0..series.len()).filter(|&i| !series[i].query.hidden).collect();
    let q = |i: usize| &series[i].query;
    if let Some(&i) =
        shown.iter().find(|&&i| q(i).label.to_lowercase() == w || q(i).metric.to_lowercase() == w)
    {
        return Ok(i);
    }
    let starts: Vec<usize> = shown.iter().copied().filter(|&i| q(i).label.to_lowercase().starts_with(&w)).collect();
    if let [i] = starts.as_slice() {
        return Ok(*i);
    }
    let rows: Vec<String> = shown
        .iter()
        .map(|&i| {
            let q = q(i);
            if q.metric.is_empty() || q.metric == q.label {
                q.label.clone()
            } else {
                format!("{} ({})", q.label, q.metric)
            }
        })
        .collect();
    let what = if starts.len() > 1 { "matches more than one" } else { "isn't charted" };
    Err(format!("open: metric {wanted:?} {what} — {}", rows.join(", ")))
}

/// The resource's section label `wanted` names (any case), or the message
/// listing the real ones.
pub fn match_section(labels: &[String], wanted: &str) -> Result<String, String> {
    labels
        .iter()
        .find(|l| l.eq_ignore_ascii_case(wanted.trim()))
        .cloned()
        .ok_or_else(|| format!("open: no section {wanted:?} — sections: {}", labels.join(", ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(target: &[&str]) -> OpenArgs {
        OpenArgs {
            target: target.iter().map(|s| s.to_string()).collect(),
            section: None,
            charts: None,
            metric: None,
            only: false,
            full: false,
        }
    }

    #[test]
    fn service_and_id() {
        let (loc, req) = plan(&args(&["ec2", "i-0abc"])).unwrap();
        assert_eq!(loc.service, ServiceType::EC2);
        assert_eq!(loc.selected_id.as_deref(), Some("i-0abc"));
        assert!(loc.details_focused);
        assert!(req.seek_tab);
        assert!(loc.lens.is_none());
    }

    #[test]
    fn an_arn_alone_finds_its_service_and_tab() {
        let (loc, _) = plan(&args(&["arn:aws:iam::123456789012:role/app/deployer"])).unwrap();
        assert_eq!(loc.service, ServiceType::IAM);
        assert!(matches!(loc.view, JumpView::Iam(crate::app::IamView::Roles)));
        assert_eq!(loc.selected_id.as_deref(), Some("deployer"));
    }

    #[test]
    fn an_unrecognised_lone_id_asks_for_the_service() {
        let err = plan(&args(&["something-plain"])).unwrap_err();
        assert!(err.contains("neboto open ec2 something-plain"), "{err}");
    }

    #[test]
    fn unknown_service_and_range_are_errors() {
        assert!(plan(&args(&["nope", "x"])).is_err());
        let mut a = args(&["ec2", "i-1"]);
        a.charts = Some("2h".into());
        assert!(plan(&a).unwrap_err().contains("1h, 6h"));
        a.charts = Some("7D".into());
        let (loc, req) = plan(&a).unwrap();
        assert_eq!(loc.lens, Some(NavLens::Metrics { range: Some(MetricsTimeRange::SevenDays), metric: None, only: false }));
        assert!(req.charts);
    }

    #[test]
    fn metric_implies_the_charts_at_their_default_range() {
        let mut a = args(&["ec2", "i-1"]);
        a.metric = Some("cpu".into());
        let (loc, req) = plan(&a).unwrap();
        assert_eq!(loc.lens, Some(NavLens::Metrics { range: None, metric: Some("cpu".into()), only: false }));
        assert!(req.charts);
    }

    fn series(rows: &[(&str, &str)]) -> Vec<crate::aws::metrics::MetricSeries> {
        rows.iter()
            .map(|(label, metric)| crate::aws::metrics::MetricSeries {
                query: crate::aws::metrics::MetricQuery {
                    label: label.to_string(),
                    ..crate::aws::metrics::MetricDefinition {
                        namespace: "AWS/EC2".into(),
                        metric: metric.to_string(),
                        dimensions: vec![],
                    }
                    .query()
                },
                points: vec![],
                error: None,
            })
            .collect()
    }

    #[test]
    fn metric_matching() {
        let s = series(&[("CPU", "CPUUtilization"), ("Network in", "NetworkIn"), ("Network out", "NetworkOut")]);
        assert_eq!(match_metric(&s, "cpu"), Ok(0));
        assert_eq!(match_metric(&s, "networkout"), Ok(2));
        assert_eq!(match_metric(&s, "network i"), Ok(1));
        let err = match_metric(&s, "net").unwrap_err();
        assert!(err.contains("more than one") && err.contains("Network in (NetworkIn)"), "{err}");
        assert!(match_metric(&s, "disk").unwrap_err().contains("isn't charted"));
        let mut h = s.clone();
        h[0].query.hidden = true;
        assert!(match_metric(&h, "cpu").is_err(), "a hidden row never takes the cursor");
    }

    #[test]
    fn virtual_services_have_nothing_to_open() {
        assert!(plan(&args(&["audit", "x"])).is_err());
    }

    fn bm(label: &str, id: &str) -> NavLocation {
        let (mut loc, _) = plan(&args(&["ec2", id])).unwrap();
        loc.label = label.into();
        loc
    }

    #[test]
    fn bookmark_matching() {
        let bms = vec![
            bm("EC2 · web-3 (i-1)", "i-1"),
            bm("EC2 · web-30 (i-2)", "i-2"),
            bm("EC2 · db-9 (i-3)", "i-3"),
        ];
        assert_eq!(pick_bookmark(&bms, "db").unwrap().selected_id.as_deref(), Some("i-3"));
        assert_eq!(pick_bookmark(&bms, "I-2").unwrap().selected_id.as_deref(), Some("i-2"));
        assert_eq!(pick_bookmark(&bms, "ec2 · web-3 (i-1)").unwrap().selected_id.as_deref(), Some("i-1"));
        assert!(pick_bookmark(&bms, "web").unwrap_err().contains("matches 2"));
        assert!(pick_bookmark(&bms, "zzz").unwrap_err().contains("none match"));
        assert!(pick_bookmark(&[], "x").unwrap_err().contains("no bookmarks"));
    }

    #[test]
    fn section_names_match_any_case() {
        let labels = vec!["Overview".to_string(), "Security".to_string()];
        assert_eq!(match_section(&labels, "security").unwrap(), "Security");
        assert!(match_section(&labels, "nope").unwrap_err().contains("Overview, Security"));
    }
}
