use super::*;

// ── Cost line-item split pane ─────────────────────────────────────────────

pub(super) fn render_cost_split(app: &App, item: &CostLineItem, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 4, "");

    let mut block = theme::pane_block("Cost", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let header_lines = build_cost_header_lines(item);
    let header_h = header_lines.len() as u16;

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(header_h),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(inner);

    frame.render_widget(Paragraph::new(header_lines), chunks[0]);
    render_hr(chunks[1], frame);
    render_cost_section_tabs(app, chunks[2], frame);
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub(super) fn build_cost_header_lines(item: &CostLineItem) -> Vec<Line<'static>> {
    use crate::aws::services::cost::fmt_money;
    let mut lines: Vec<Line<'static>> = vec![Line::raw("")];

    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            item.name().to_string(),
            Style::default()
                .fg(crate::ui::theme::text_primary())
                .add_modifier(Modifier::BOLD),
        ),
    ]));

    lines.push(Line::raw(""));
    lines.push(header_kv(
        &format!("Cost ({})", item.period_label),
        &format!("${} {}", fmt_money(item.current), item.currency),
    ));
    if item.has_prior {
        lines.push(header_kv(
            &format!("Prior ({})", item.prior_label),
            &format!("${} {}", fmt_money(item.prior), item.currency),
        ));
        if let Some(p) = item.delta_pct() {
            let arrow = if p >= 0.0 { "▲" } else { "▼" };
            lines.push(header_kv("Change", &format!("{} {:+.1}%", arrow, p)));
        }
    } else if let Some(p) = item.mom_delta_pct {
        // 3-month view: no comparable prior window — the trend is the
        // month-over-month delta (last two complete months).
        let arrow = if p >= 0.0 { "▲" } else { "▼" };
        lines.push(header_kv("Change (MoM)", &format!("{} {:+.1}%", arrow, p)));
    }

    lines.push(Line::raw(""));
    lines
}

pub(super) fn render_cost_section_tabs(app: &App, area: Rect, frame: &mut Frame) {
    // Section 2's label adapts: a region / tag / category row breaks down
    // into services (`fetch_cost_drilldown`'s secondary dimension).
    use crate::aws::services::cost::CostGroupBy as G;
    let secondary = if matches!(app.cost_group_by, G::Region | G::Tag | G::CostCategory) {
        "Services"
    } else {
        "Regions"
    };
    let tabs = [
        ('1', "Breakdown", CostDetailSection::from_index(app.detail_section_index()) == CostDetailSection::Breakdown),
        ('2', secondary, CostDetailSection::from_index(app.detail_section_index()) == CostDetailSection::Regions),
        ('3', "Trend", CostDetailSection::from_index(app.detail_section_index()) == CostDetailSection::Trend),
        ('4', "Forecast", CostDetailSection::from_index(app.detail_section_index()) == CostDetailSection::Forecast),
    ];
    render_section_tab_bar(app, area, frame, &tabs);
}


pub fn cost_section_lines(
    item: &CostLineItem,
    section: CostDetailSection,
    drilldown: Option<&crate::lazy::Lazy<Box<crate::aws::services::cost::CostDrilldown>>>,
) -> Vec<(String, String)> {
    match section {
        CostDetailSection::Breakdown => cost_breakdown_lines(item, drilldown, false),
        CostDetailSection::Regions => cost_breakdown_lines(item, drilldown, true),
        CostDetailSection::Trend => cost_trend_lines(item),
        CostDetailSection::Forecast => cost_forecast_lines(item, drilldown),
    }
}

/// A Cost Anomaly Detection anomaly's pane. Both sections are eager — the
/// list call (`GetAnomalies`) returns everything shown here.
pub fn cost_anomaly_section_lines(
    a: &crate::aws::services::cost::CostAnomaly,
    section: crate::aws::services::cost::CostAnomalyDetailSection,
) -> Vec<(String, String)> {
    use crate::aws::services::cost::{fmt_money, CostAnomalyDetailSection as S};
    let money = |v: f64| format!("${}", fmt_money(v));
    let kv = |k: &str, v: String| (k.to_string(), v);
    match section {
        S::Overview => {
            let mut rows = vec![
                kv("Anomaly ID", a.anomaly_id.clone()),
                kv(
                    "Status",
                    if a.is_ongoing() { "⚠ ongoing".to_string() } else { "closed".to_string() },
                ),
                kv("Start", a.start_date.clone().unwrap_or_else(|| "—".to_string())),
                kv("End", a.end_date.clone().unwrap_or_else(|| "— (ongoing)".to_string())),
                (String::new(), String::new()),
                kv("Impact", String::new()),
                kv("Total impact", money(a.total_impact)),
            ];
            if let Some(p) = a.impact_pct {
                rows.push(kv("Above expected", format!("+{:.1}%", p)));
            }
            if let Some(v) = a.total_actual {
                rows.push(kv("Actual spend", money(v)));
            }
            if let Some(v) = a.total_expected {
                rows.push(kv("Expected spend", money(v)));
            }
            rows.push(kv("Max daily impact", money(a.max_impact)));
            rows.push((String::new(), String::new()));
            rows.push(kv("Score", String::new()));
            rows.push(kv("Max score", format!("{:.2}", a.max_score)));
            rows.push(kv("Current score", format!("{:.2}", a.current_score)));
            rows.push((String::new(), String::new()));
            rows.push(kv("Detection", String::new()));
            if !a.dimension_value.is_empty() {
                rows.push(kv("Dimension value", a.dimension_value.clone()));
            }
            rows.push(kv("Monitor", a.monitor_arn.clone()));
            rows.push(kv(
                "Feedback",
                a.feedback.clone().unwrap_or_else(|| "none given".to_string()),
            ));
            rows
        }
        S::RootCauses => {
            if a.root_causes.is_empty() {
                return vec![(
                    String::new(),
                    "No root causes reported for this anomaly".to_string(),
                )];
            }
            let mut rows = Vec::new();
            for (i, rc) in a.root_causes.iter().enumerate() {
                if i > 0 {
                    rows.push((String::new(), String::new()));
                }
                let head = match rc.contribution {
                    Some(c) => format!("Cause {} · {}", i + 1, money(c)),
                    None => format!("Cause {}", i + 1),
                };
                rows.push((head, String::new()));
                if let Some(v) = &rc.service {
                    rows.push(kv("Service", v.clone()));
                }
                if let Some(v) = &rc.usage_type {
                    rows.push(kv("Usage type", v.clone()));
                }
                if let Some(v) = &rc.region {
                    rows.push(kv("Region", v.clone()));
                }
                if let Some(acct) = &rc.linked_account {
                    let v = match &rc.linked_account_name {
                        Some(n) if !n.is_empty() => format!("{} ({})", acct, n),
                        _ => acct.clone(),
                    };
                    rows.push(kv("Account", v));
                }
            }
            rows
        }
    }
}

/// Truncate to `n` chars, appending an ellipsis when clipped.
pub(crate) fn cost_trunc(s: &str, n: usize) -> String {
    if s.chars().count() > n {
        let t: String = s.chars().take(n.saturating_sub(1)).collect();
        format!("{}…", t)
    } else {
        s.to_string()
    }
}

/// A ranked `name … $amount  pct%` table emitted as plain content lines.
pub(super) fn cost_table(entries: &[(String, f64)]) -> Vec<(String, String)> {
    use crate::aws::services::cost::fmt_money;
    let mut rows = vec![];
    if entries.is_empty() {
        rows.push(("  No data for this period".to_string(), String::new()));
        return rows;
    }
    let total: f64 = entries.iter().map(|(_, a)| *a).sum();
    rows.push((
        format!(" {:<40} {:>14}  {:>5}", "Name", "Cost", "Share"),
        String::new(),
    ));
    rows.push((format!(" {}", "─".repeat(63)), String::new()));
    for (name, amt) in entries.iter().take(40) {
        let pct = if total > 0.0 { amt / total * 100.0 } else { 0.0 };
        rows.push((
            format!(
                " {:<40} {:>14}  {:>4.0}%",
                cost_trunc(name, 40),
                format!("${}", fmt_money(*amt)),
                pct
            ),
            String::new(),
        ));
    }
    rows
}

pub(super) fn cost_breakdown_lines(
    item: &CostLineItem,
    drilldown: Option<&crate::lazy::Lazy<Box<crate::aws::services::cost::CostDrilldown>>>,
    secondary: bool,
) -> Vec<(String, String)> {
    let mut rows = vec![(String::new(), String::new())];
    match drilldown {
        None | Some(crate::lazy::Lazy::Loading) => {
            rows.push(("  Loading breakdown…".to_string(), String::new()));
        }
        Some(crate::lazy::Lazy::Error(e)) => {
            rows.extend(error_rows(e));
        }
        Some(crate::lazy::Lazy::Loaded(d)) => {
            let (title, entries) = if secondary {
                (d.secondary_label, &d.secondary)
            } else {
                ("By usage type", &d.usage_types)
            };
            rows.push((format!("{} ({})", title, d.window_label), String::new()));
            rows.push((String::new(), String::new()));
            rows.extend(cost_table(entries));
        }
    }
    rows.push((String::new(), String::new()));
    rows.push((
        "Note".to_string(),
        format!(
            "Breakdown of {} over {} — the same window as the list totals.",
            item.name(),
            item.period_label
        ),
    ));
    rows
}

pub(super) fn cost_trend_lines(item: &CostLineItem) -> Vec<(String, String)> {
    use crate::aws::services::cost::{fmt_money, short_date, sparkline};
    let cur = &item.currency;
    let mut rows = vec![
        (
            format!("Current ({})", item.period_label),
            format!("${} {}", fmt_money(item.current), cur),
        ),
    ];
    if item.has_prior {
        rows.push((
            format!("Prior ({})", item.prior_label),
            format!("${} {}", fmt_money(item.prior), cur),
        ));
        if let Some(p) = item.delta_pct() {
            rows.push(("Change".to_string(), format!("{:+.1}%", p)));
        }
    } else if let Some(p) = item.mom_delta_pct {
        rows.push(("Change (MoM)".to_string(), format!("{:+.1}%", p)));
    }
    rows.push((
        "Daily avg".to_string(),
        format!("${} {}", fmt_money(item.daily_avg()), cur),
    ));

    // Multi-month window (3-month view): a per-month table is the natural
    // read, and the sparkline is bucketed weekly so ~90 daily points don't
    // render as noise.
    let months = item.monthly_rows();
    let multi_month = months.len() >= 2;
    if multi_month {
        rows.push((String::new(), String::new()));
        rows.push(("By month".to_string(), String::new()));
        for (label, total, is_current) in &months {
            let key = if *is_current {
                format!("{} (to date)", label)
            } else {
                label.clone()
            };
            rows.push((key, format!("${} {}", fmt_money(*total), cur)));
        }
    }

    let values: Vec<f64> = item.daily.iter().map(|(_, a)| *a).collect();
    rows.push((String::new(), String::new()));
    if values.is_empty() {
        rows.push(("Daily spend".to_string(), String::new()));
        rows.push(("  No daily data".to_string(), String::new()));
    } else if multi_month {
        rows.push(("Weekly spend".to_string(), String::new()));
        let weekly: Vec<f64> = values.chunks(7).map(|c| c.iter().sum()).collect();
        rows.push((format!(" {}", sparkline(&weekly)), String::new()));
        rows.push((
            format!(
                " {} → now, weekly buckets    (press m for the daily chart)",
                item.first_date_label()
            ),
            String::new(),
        ));
    } else {
        rows.push(("Daily spend".to_string(), String::new()));
        // Sparkline spanning the whole window, with the date range beneath.
        rows.push((format!(" {}", sparkline(&values)), String::new()));
        rows.push((
            format!(
                " {} → now    (press m for full chart)",
                item.first_date_label()
            ),
            String::new(),
        ));
        rows.push((String::new(), String::new()));
        // The last ~12 days as a small table.
        let recent: Vec<(String, f64)> = item.daily.iter().rev().take(12).rev().cloned().collect();
        for (date, amt) in recent {
            rows.push((short_date(&date), format!("${} {}", fmt_money(amt), cur)));
        }
    }

    rows.push((String::new(), String::new()));
    rows
}

pub(super) fn cost_forecast_lines(
    _item: &CostLineItem,
    drilldown: Option<&crate::lazy::Lazy<Box<crate::aws::services::cost::CostDrilldown>>>,
) -> Vec<(String, String)> {
    use crate::aws::services::cost::fmt_money;
    let mut rows = vec![(String::new(), String::new())];
    match drilldown {
        None | Some(crate::lazy::Lazy::Loading) => {
            rows.push(("  Loading forecast…".to_string(), String::new()));
        }
        Some(crate::lazy::Lazy::Error(e)) => {
            rows.extend(error_rows(e));
        }
        Some(crate::lazy::Lazy::Loaded(d)) => match &d.forecast {
            None => {
                rows.push((
                    "  Forecast unavailable".to_string(),
                    String::new(),
                ));
                rows.push((String::new(), String::new()));
                rows.push((
                    " Cost Explorer needs more history to forecast this row.".to_string(),
                    String::new(),
                ));
            }
            Some(f) => {
                // The forecast always concerns the CURRENT month, whatever
                // period the list is showing — say so explicitly, and use the
                // row's real MTD spend as the baseline (the breakdown sum only
                // equals it for the MTD period; past windows fetch it apart).
                rows.push(("Month-end forecast (current month)".to_string(), String::new()));
                rows.push((String::new(), String::new()));
                match d.mtd_spent {
                    Some(mtd) => {
                        rows.push((
                            "Spent so far (MTD)".to_string(),
                            format!("${} {}", fmt_money(mtd), f.currency),
                        ));
                        rows.push((
                            "Forecast (rest)".to_string(),
                            format!("${} {}", fmt_money(f.remaining), f.currency),
                        ));
                        rows.push((
                            "Projected total".to_string(),
                            format!("${} {}", fmt_money(mtd + f.remaining), f.currency),
                        ));
                        rows.push((String::new(), String::new()));
                        rows.push((
                            "Range (80% CI)".to_string(),
                            format!(
                                "${} – ${} {}",
                                fmt_money(mtd + f.lower),
                                fmt_money(mtd + f.upper),
                                f.currency
                            ),
                        ));
                    }
                    // MTD lookup failed (best-effort) — show what CE returned.
                    None => {
                        rows.push((
                            "Forecast (rest of month)".to_string(),
                            format!("${} {}", fmt_money(f.remaining), f.currency),
                        ));
                        rows.push((String::new(), String::new()));
                        rows.push((
                            "Range (80% CI)".to_string(),
                            format!(
                                "${} – ${} {}",
                                fmt_money(f.lower),
                                fmt_money(f.upper),
                                f.currency
                            ),
                        ));
                    }
                }
            }
        },
    }
    rows.push((String::new(), String::new()));
    rows
}

#[cfg(test)]
mod cost_anomaly_tests {
    use super::cost_anomaly_section_lines;
    use crate::aws::services::cost::{CostAnomaly, CostAnomalyDetailSection as S};

    #[test]
    fn overview_reads_the_impact_and_flags_an_open_anomaly() {
        let a = CostAnomaly::mock();
        let rows = cost_anomaly_section_lines(&a, S::Overview);
        let get = |k: &str| rows.iter().find(|(key, _)| key == k).map(|(_, v)| v.clone());
        assert_eq!(get("Status").as_deref(), Some("⚠ ongoing"));
        assert_eq!(get("Total impact").as_deref(), Some("$96.40"));
        assert_eq!(get("Above expected").as_deref(), Some("+82.0%"));
        assert_eq!(get("Expected spend").as_deref(), Some("$117.50"));
        assert_eq!(get("Feedback").as_deref(), Some("none given"));
        // Group headers are key-only rows (no value).
        assert_eq!(get("Impact").as_deref(), Some(""));
    }

    #[test]
    fn root_causes_list_largest_first_with_account_names() {
        let a = CostAnomaly::mock();
        let rows = cost_anomaly_section_lines(&a, S::RootCauses);
        assert_eq!(rows[0].0, "Cause 1 · $88.10");
        assert!(rows.contains(&("Account".to_string(), "123456789012 (acme-prod)".to_string())));
        assert!(rows.iter().any(|(k, _)| k == "Cause 2 · $8.30"));
    }
}
