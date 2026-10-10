use super::*;

// ── Budget split pane ──────────────────────────────────────────────────────────

pub(super) fn render_budget_split(app: &App, b: &BudgetItem, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 4, "");
    let mut block = theme::pane_block("Budget", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let state_color = match b.state() {
        crate::aws::resource::ResourceState::Unavailable => theme::error(),
        crate::aws::resource::ResourceState::Pending => theme::warning(),
        crate::aws::resource::ResourceState::Available => theme::success(),
        _ => theme::text_dim(),
    };
    let subtitle = match b.pct_used() {
        Some(pct) => format!("{}  ·  {:.0}% used", b.budget_type, pct),
        None => b.budget_type.clone(),
    };
    let header = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                b.name.clone(),
                Style::default().fg(theme::text_primary()).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(subtitle, Style::default().fg(state_color)),
        ]),
        Line::raw(""),
    ];
    let header_h = header.len() as u16;
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
    frame.render_widget(Paragraph::new(header), chunks[0]);
    render_hr(chunks[1], frame);
    render_section_tab_bar(
        app,
        chunks[2],
        frame,
        &descriptor_tabs(app, &crate::aws::services::budgets::BUDGET_SECTIONS),
    );
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub fn budget_section_lines(
    b: &BudgetItem,
    section: BudgetDetailSection,
    notifications: Option<&Lazy<Vec<crate::aws::services::budgets::BudgetNotification>>>,
    tags: Option<&Lazy<Vec<(String, String)>>>,
) -> Vec<(String, String)> {
    use crate::aws::services::cost::fmt_money;
    match section {
        BudgetDetailSection::Overview => {
            let mut rows = vec![
                ("Name".to_string(), b.name.clone()),
                ("Type".to_string(), b.budget_type.clone()),
                ("Time Unit".to_string(), b.time_unit.clone()),
            ];
            if let (Some(s), Some(e)) = (&b.period_start, &b.period_end) {
                rows.push(("Period".to_string(), format!("{} – {}", s, e)));
            }
            rows.push((String::new(), String::new()));
            rows.push(("Spend".to_string(), String::new())); // group header
            if let Some(l) = b.limit_amount {
                rows.push(("  Limit".to_string(), format!("{} {}", fmt_money(l), b.limit_unit)));
            }
            if let Some(a) = b.actual_amount {
                rows.push(("  Actual".to_string(), format!("{} {}", fmt_money(a), b.actual_unit)));
            }
            if let Some(pct) = b.pct_used() {
                rows.push(("  % Used".to_string(), format!("{:.0}%", pct)));
            }
            if let Some(f) = b.forecasted_amount {
                rows.push(("  Forecasted".to_string(), format!("{} {}", fmt_money(f), b.actual_unit)));
            }
            if let Some(pct) = b.pct_forecasted() {
                rows.push(("  % Forecasted".to_string(), format!("{:.0}%", pct)));
            }
            if let Some(a) = &b.auto_adjust {
                rows.push((String::new(), String::new()));
                rows.push(("Auto-Adjust".to_string(), a.clone()));
            }
            if let Some(t) = &b.last_updated {
                rows.push((String::new(), String::new()));
                rows.push(("Last Updated".to_string(), t.clone()));
            }
            rows
        }
        BudgetDetailSection::Filters => {
            if b.filter_rows.is_empty() {
                vec![("".to_string(), "No filters — applies to all cost/usage".to_string())]
            } else {
                b.filter_rows.clone()
            }
        }
        BudgetDetailSection::Notifications => match notifications {
            None | Some(Lazy::Loading) => {
                vec![("".to_string(), "Loading notifications…".to_string())]
            }
            Some(Lazy::Error(e)) => error_rows(e),
            Some(Lazy::Loaded(list)) if list.is_empty() => {
                vec![("".to_string(), "No notifications configured".to_string())]
            }
            Some(Lazy::Loaded(list)) => {
                let mut rows = Vec::new();
                for (i, n) in list.iter().enumerate() {
                    if i > 0 {
                        rows.push((String::new(), String::new()));
                    }
                    let threshold = match n.threshold_type.as_deref() {
                        Some("ABSOLUTE_VALUE") => format!("{}", n.threshold),
                        _ => format!("{}%", n.threshold),
                    };
                    rows.push((
                        format!("{} {} {}", n.notification_type, n.comparison_operator, threshold),
                        String::new(),
                    )); // group header
                    rows.push((
                        "  State".to_string(),
                        if n.alarm { "⚠ ALARM".to_string() } else { "✓ OK".to_string() },
                    ));
                    if n.subscribers.is_empty() {
                        rows.push(("  Subscribers".to_string(), "—".to_string()));
                    } else {
                        for (kind, addr) in &n.subscribers {
                            rows.push((format!("  {}", kind), addr.clone()));
                        }
                    }
                }
                rows
            }
        },
        BudgetDetailSection::Tags => match tags {
            None | Some(Lazy::Loading) => vec![("".to_string(), "Loading tags…".to_string())],
            Some(Lazy::Error(e)) => error_rows(e),
            Some(Lazy::Loaded(list)) if list.is_empty() => {
                vec![("".to_string(), "No tags".to_string())]
            }
            Some(Lazy::Loaded(list)) => list.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        },
    }
}
