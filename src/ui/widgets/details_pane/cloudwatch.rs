use super::*;

// ── CloudWatch Alarm split pane ───────────────────────────────────────────────

pub(super) fn render_cw_alarm_split(app: &App, alarm: &CwAlarm, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;

    let footer = detail_footer(app, focused, 3, "");

    let mut block = theme::pane_block("CloudWatch Alarm", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let header_lines = build_cw_alarm_header_lines(alarm);
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
    render_cw_section_tabs(app, chunks[2], frame);
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub(super) fn build_cw_alarm_header_lines(alarm: &CwAlarm) -> Vec<Line<'static>> {
    let (state_color, state_dot) = match alarm.state.as_str() {
        "OK" => (theme::success(), "● "),
        "ALARM" => (theme::error(), "● "),
        _ => (theme::warning(), "◌ "),
    };

    vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                alarm.alarm_name.clone(),
                Style::default().fg(crate::ui::theme::text_primary()).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(state_dot.to_string(), Style::default().fg(state_color)),
            Span::styled(
                alarm.state_display().to_string(),
                Style::default().fg(state_color).add_modifier(Modifier::BOLD),
            ),
            Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
            Span::styled(alarm.namespace.clone(), Style::default().fg(theme::text_dim())),
            Span::styled("  /  ", Style::default().fg(theme::text_dim())),
            Span::styled(
                alarm.metric_name.clone(),
                Style::default().fg(theme::aws_orange()),
            ),
        ]),
        Line::raw(""),
    ]
}

pub(super) fn render_cw_section_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let tabs = descriptor_tabs(app, &crate::aws::services::cloudwatch::CW_ALARM_SECTIONS);
    render_section_tab_bar(app, area, frame, &tabs);
}

pub fn cw_alarm_section_lines(
    alarm: &CwAlarm,
    section: CwAlarmDetailSection,
    history_state: Option<&crate::lazy::Lazy<Vec<crate::aws::services::cloudwatch::CwAlarmHistoryItem>>>,
) -> Vec<(String, String)> {
    match section {
        CwAlarmDetailSection::Config => cw_config_lines(alarm),
        CwAlarmDetailSection::Actions => cw_actions_lines(alarm),
        CwAlarmDetailSection::History => cw_history_lines(history_state),
    }
}

pub(super) fn cw_config_lines(alarm: &CwAlarm) -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = Vec::new();

    rows.push(("Metric".to_string(), "".to_string()));
    rows.push(("".to_string(), "".to_string()));
    rows.push(("  Namespace".to_string(), alarm.namespace.clone()));
    rows.push(("  Metric".to_string(), alarm.metric_name.clone()));
    rows.push(("  Statistic".to_string(), alarm.statistic.clone()));
    rows.push(("  Period".to_string(), format!("{} s", alarm.period)));

    if !alarm.dimensions.is_empty() {
        rows.push(("".to_string(), "".to_string()));
        rows.push(("Dimensions".to_string(), "".to_string()));
        rows.push(("".to_string(), "".to_string()));
        for (name, value) in &alarm.dimensions {
            rows.push((format!("  {}", name), value.clone()));
        }
    }

    rows.push(("".to_string(), "".to_string()));
    rows.push(("Threshold".to_string(), "".to_string()));
    rows.push(("".to_string(), "".to_string()));
    rows.push((
        "  Condition".to_string(),
        format!(
            "{} {} {}",
            alarm.metric_name,
            alarm.comparison_display(),
            alarm.threshold
        ),
    ));
    if let Some(periods) = alarm.evaluation_periods {
        // "M of N datapoints" breaching, each datapoint covering `period` secs.
        let dp = alarm.datapoints_to_alarm.unwrap_or(periods);
        rows.push((
            "  Evaluation".to_string(),
            format!("{} of {} datapoints ({}s each)", dp, periods, alarm.period),
        ));
    }
    if !alarm.treat_missing_data.is_empty() {
        rows.push(("  Missing data".to_string(), alarm.treat_missing_data.clone()));
    }

    rows.push(("".to_string(), "".to_string()));
    rows.push(("  ARN".to_string(), alarm.alarm_arn.clone()));
    if !alarm.alarm_description.is_empty() {
        rows.push(("  Description".to_string(), alarm.alarm_description.clone()));
    }

    rows
}

pub(super) fn cw_actions_lines(alarm: &CwAlarm) -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = Vec::new();

    let push_actions = |rows: &mut Vec<(String, String)>, label: &str, actions: &[String]| {
        rows.push((label.to_string(), "".to_string()));
        rows.push(("".to_string(), "".to_string()));
        if actions.is_empty() {
            rows.push(("  (none)".to_string(), "".to_string()));
        } else {
            for arn in actions {
                let short = arn.rsplit(':').next().unwrap_or(arn.as_str());
                rows.push((format!("  {}", short), arn.clone()));
            }
        }
        rows.push(("".to_string(), "".to_string()));
    };

    push_actions(&mut rows, "Alarm Actions", &alarm.alarm_actions);
    push_actions(&mut rows, "OK Actions", &alarm.ok_actions);
    push_actions(&mut rows, "Insufficient Data Actions", &alarm.insufficient_data_actions);

    rows
}

pub(super) fn cw_history_lines(history_state: Option<&crate::lazy::Lazy<Vec<crate::aws::services::cloudwatch::CwAlarmHistoryItem>>>) -> Vec<(String, String)> {
    match history_state {
        None | Some(crate::lazy::Lazy::Loading) => {
            vec![("".to_string(), "Loading history…".to_string())]
        }
        Some(crate::lazy::Lazy::Loaded(items)) => {
            let mut rows = vec![];
            if items.is_empty() {
                rows.push(("".to_string(), "No history found".to_string()));
                return rows;
            }
            for item in items {
                rows.push((format!("  {}", item.timestamp), item.item_type.clone()));
                rows.push(("  ".to_string(), item.summary.clone()));
                rows.push(("".to_string(), "".to_string()));
            }
            rows
        }
        Some(crate::lazy::Lazy::Error(e)) => error_rows(e),
    }
}

// ── CloudWatch Dashboard split pane ───────────────────────────────────────────

pub(super) fn render_cw_dashboard_split(app: &App, dash: &CwDashboard, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 1, "m dashboard");

    let mut block = theme::pane_block("CloudWatch Dashboard", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let header_lines = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                dash.name.clone(),
                Style::default().fg(crate::ui::theme::text_primary()).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("Dashboard", Style::default().fg(theme::aws_orange())),
            Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
            Span::styled(fmt_bytes(dash.size_bytes), Style::default().fg(theme::text_dim())),
            Span::styled(
                match dash.last_modified {
                    Some(secs) => format!(
                        "  ·  modified {}",
                        crate::aws::services::cloudwatch::fmt_epoch_secs(secs)
                    ),
                    None => String::new(),
                },
                Style::default().fg(theme::text_dim()),
            ),
        ]),
        Line::raw(""),
    ];
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
    render_cw_dashboard_section_tabs(app, chunks[2], frame);
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub(super) fn render_cw_dashboard_section_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let tabs = descriptor_tabs(app, &crate::aws::services::cloudwatch::CW_DASHBOARD_SECTIONS);
    render_section_tab_bar(app, area, frame, &tabs);
}

pub fn cw_dashboard_section_lines(
    dash: &CwDashboard,
    section: CwDashboardDetailSection,
    state: Option<&crate::lazy::Lazy<crate::aws::services::cloudwatch::CwDashboardBody>>,
) -> Vec<(String, String)> {
    let body = match state {
        None | Some(crate::lazy::Lazy::Loading) => {
            // Content-line shape (leading-space key, empty value) and the
            // canonical wording: an empty key with a value renders as a
            // key-value row, so it came out as 24 blank columns and a stray
            // colon. `spin_loading_row` rewrites this centrally.
            return vec![("  Loading…".to_string(), String::new())];
        }
        Some(crate::lazy::Lazy::Error(e)) => return error_rows(e),
        Some(crate::lazy::Lazy::Loaded(b)) => b,
    };

    match section {
        CwDashboardDetailSection::Raw => {
            // Plain content lines of the pretty-printed JSON; `e` opens the full
            // body in $EDITOR (open_in_editor downcasts CwDashboard).
            let mut rows: Vec<(String, String)> = Vec::new();
            if !dash.arn.is_empty() {
                rows.push(("Dashboard".to_string(), String::new()));
                rows.push(("  ARN".to_string(), dash.arn.clone()));
                rows.push(("".to_string(), String::new()));
            }
            // The alarms a dashboard watches are the one cross-reference the
            // rendered grid can't follow (`⏎` is zoom there), so the ARNs keep
            // their Enter-jump here.
            let alarms: Vec<&String> = body.widgets.iter().flat_map(|w| &w.alarm_arns).collect();
            if !alarms.is_empty() {
                rows.push(("Alarms".to_string(), String::new()));
                for arn in alarms {
                    let short = arn.rsplit(':').next().unwrap_or(arn);
                    rows.push((format!("  {}", short), arn.clone()));
                }
                rows.push(("".to_string(), String::new()));
            }

            rows.push((format!("Body ({} widgets)", body.widgets.len()), String::new()));
            rows.push(("".to_string(), String::new()));
            for line in body.raw_json.lines() {
                rows.push((format!("  {}", line), String::new()));
            }
            rows
        }
    }
}

// ── CloudWatch Composite Alarm split pane ─────────────────────────────────────

pub(super) fn render_cw_composite_alarm_split(
    app: &App,
    alarm: &CwCompositeAlarm,
    area: Rect,
    frame: &mut Frame,
) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 2, "");

    let mut block = theme::pane_block("CloudWatch Composite Alarm", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let (state_color, state_dot) = match alarm.state.as_str() {
        "OK" => (theme::success(), "● "),
        "ALARM" => (theme::error(), "● "),
        _ => (theme::warning(), "◌ "),
    };
    let header_lines = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                alarm.alarm_name.clone(),
                Style::default().fg(crate::ui::theme::text_primary()).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(state_dot.to_string(), Style::default().fg(state_color)),
            Span::styled(
                alarm.state_display().to_string(),
                Style::default().fg(state_color).add_modifier(Modifier::BOLD),
            ),
            Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
            Span::styled("Composite", Style::default().fg(theme::aws_orange())),
        ]),
        Line::raw(""),
    ];
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
    render_cw_composite_section_tabs(app, chunks[2], frame);
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub(super) fn render_cw_composite_section_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let tabs = descriptor_tabs(app, &crate::aws::services::cloudwatch::CW_COMPOSITE_ALARM_SECTIONS);
    render_section_tab_bar(app, area, frame, &tabs);
}

pub fn cw_composite_alarm_section_lines(
    alarm: &CwCompositeAlarm,
    section: CwCompositeAlarmDetailSection,
) -> Vec<(String, String)> {
    match section {
        CwCompositeAlarmDetailSection::Rule => cw_composite_rule_lines(alarm),
        CwCompositeAlarmDetailSection::Actions => cw_composite_actions_lines(alarm),
    }
}

pub(super) fn cw_composite_rule_lines(alarm: &CwCompositeAlarm) -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = Vec::new();

    rows.push(("State".to_string(), "".to_string()));
    rows.push(("".to_string(), "".to_string()));
    rows.push(("  State".to_string(), alarm.state.clone()));
    if !alarm.state_reason.is_empty() {
        rows.push(("  Reason".to_string(), alarm.state_reason.clone()));
    }
    rows.push((
        "  Actions enabled".to_string(),
        if alarm.actions_enabled { "✓".to_string() } else { "✗".to_string() },
    ));

    rows.push(("".to_string(), "".to_string()));
    rows.push(("Rule".to_string(), "".to_string()));
    rows.push(("".to_string(), "".to_string()));
    if alarm.alarm_rule.is_empty() {
        rows.push(("  (no rule)".to_string(), "".to_string()));
    } else {
        // The raw boolean expression as plain content lines (wrapped on AND/OR).
        for line in wrap_rule(&alarm.alarm_rule) {
            rows.push((format!("  {}", line), "".to_string()));
        }
    }

    let children = crate::aws::services::cloudwatch::alarm_rule_children(&alarm.alarm_rule);
    if !children.is_empty() {
        rows.push(("".to_string(), "".to_string()));
        rows.push(("Child alarms".to_string(), "".to_string()));
        rows.push(("".to_string(), "".to_string()));
        for child in children {
            // Show a short label; the value carries the raw token (name or ARN)
            // so `Enter` can resolve + jump to it via the child classifier.
            let label = child.rsplit(':').next().unwrap_or(&child).to_string();
            rows.push((format!("  {}", label), child));
        }
    }

    rows.push(("".to_string(), "".to_string()));
    rows.push(("  ARN".to_string(), alarm.alarm_arn.clone()));
    if !alarm.alarm_description.is_empty() {
        rows.push(("  Description".to_string(), alarm.alarm_description.clone()));
    }

    rows
}

/// Wrap a composite alarm rule onto multiple lines at top-level AND/OR so a long
/// expression stays readable.
pub(super) fn wrap_rule(rule: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut cur = String::new();
    for tok in rule.split_inclusive([' ']) {
        let trimmed = tok.trim();
        if (trimmed == "AND" || trimmed == "OR") && !cur.trim().is_empty() {
            lines.push(cur.trim_end().to_string());
            cur.clear();
            cur.push_str(tok);
        } else {
            cur.push_str(tok);
        }
    }
    if !cur.trim().is_empty() {
        lines.push(cur.trim_end().to_string());
    }
    if lines.is_empty() {
        lines.push(rule.to_string());
    }
    lines
}

pub(super) fn cw_composite_actions_lines(alarm: &CwCompositeAlarm) -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = Vec::new();

    let push_actions = |rows: &mut Vec<(String, String)>, label: &str, actions: &[String]| {
        rows.push((label.to_string(), "".to_string()));
        rows.push(("".to_string(), "".to_string()));
        if actions.is_empty() {
            rows.push(("  (none)".to_string(), "".to_string()));
        } else {
            for arn in actions {
                let short = arn.rsplit(':').next().unwrap_or(arn.as_str());
                rows.push((format!("  {}", short), arn.clone()));
            }
        }
        rows.push(("".to_string(), "".to_string()));
    };

    push_actions(&mut rows, "Alarm Actions", &alarm.alarm_actions);
    push_actions(&mut rows, "OK Actions", &alarm.ok_actions);
    push_actions(&mut rows, "Insufficient Data Actions", &alarm.insufficient_data_actions);

    rows
}

// ── CloudWatch Log Group split pane ───────────────────────────────────────────

pub(super) fn render_cw_log_group_split(app: &App, lg: &CwLogGroup, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 3, "f search");

    let mut block = theme::pane_block("CloudWatch Log Group", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let header_lines = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                lg.log_group_name.clone(),
                Style::default().fg(crate::ui::theme::text_primary()).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(lg.retention_display(), Style::default().fg(theme::text_dim())),
            Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
            Span::styled(lg.stored_bytes_display(), Style::default().fg(theme::aws_orange())),
        ]),
        Line::raw(""),
    ];
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
    render_cw_log_group_section_tabs(app, chunks[2], frame);
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub(super) fn render_cw_log_group_section_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let tabs = descriptor_tabs(app, &crate::aws::services::cloudwatch::CW_LOG_GROUP_SECTIONS);
    render_section_tab_bar(app, area, frame, &tabs);
}

pub fn cw_log_group_section_lines(
    lg: &CwLogGroup,
    section: CwLogGroupDetailSection,
    filters_state: Option<&crate::lazy::Lazy<crate::aws::services::cloudwatch::CwLogGroupFilters>>,
    streams_state: Option<&crate::lazy::Lazy<Vec<crate::aws::services::cloudwatch::CwLogStream>>>,
) -> Vec<(String, String)> {
    match section {
        CwLogGroupDetailSection::Streams => match streams_state {
            None | Some(crate::lazy::Lazy::Loading) => {
                vec![("".to_string(), "Loading streams…".to_string())]
            }
            Some(crate::lazy::Lazy::Loaded(streams)) => {
                let mut rows: Vec<(String, String)> = Vec::new();
                rows.push((
                    format!("Streams ({}, newest first)", streams.len()),
                    "".to_string(),
                ));
                rows.push(("".to_string(), "".to_string()));
                if streams.is_empty() {
                    rows.push(("  (no streams)".to_string(), "".to_string()));
                } else {
                    rows.push((
                        "  ⏎/t tails · f searches the selected stream".to_string(),
                        "".to_string(),
                    ));
                    rows.push(("".to_string(), "".to_string()));
                    for s in streams {
                        // Short name (last '/' segment) + last-event time; the
                        // value carries nothing jumpable — drill-in is via t/f,
                        // which map the cursor row to a stream by index.
                        let last = s
                            .last_event_ms
                            .map(|ms| crate::aws::services::cloudwatch::fmt_epoch_secs(ms / 1000))
                            .unwrap_or_else(|| "—".to_string());
                        let short = s.name.rsplit('/').next().unwrap_or(&s.name);
                        rows.push((format!("  {}", short), format!("last event {}", last)));
                    }
                }
                rows
            }
            Some(crate::lazy::Lazy::Error(e)) => error_rows(e),
        },
        CwLogGroupDetailSection::Details => {
            let mut rows: Vec<(String, String)> = Vec::new();
            rows.push(("Log Group".to_string(), "".to_string()));
            rows.push(("".to_string(), "".to_string()));
            rows.push(("  Name".to_string(), lg.log_group_name.clone()));
            rows.push((
                "  Class".to_string(),
                lg.log_group_class.clone().unwrap_or_else(|| "STANDARD".to_string()),
            ));
            rows.push(("  Retention".to_string(), lg.retention_display()));
            rows.push(("  Stored".to_string(), lg.stored_bytes_display()));
            if let Some(ms) = lg.creation_time_ms {
                rows.push((
                    "  Created".to_string(),
                    crate::aws::services::cloudwatch::fmt_epoch_secs(ms / 1000),
                ));
            }
            rows.push((
                "  Metric filters".to_string(),
                lg.metric_filter_count.unwrap_or(0).to_string(),
            ));

            rows.push(("".to_string(), "".to_string()));
            rows.push(("Encryption".to_string(), "".to_string()));
            rows.push(("".to_string(), "".to_string()));
            match &lg.kms_key_id {
                Some(k) if !k.is_empty() => {
                    rows.push(("  KMS key".to_string(), k.clone()));
                }
                _ => rows.push(("  KMS key".to_string(), "None (AWS-owned)".to_string())),
            }

            rows.push(("".to_string(), "".to_string()));
            rows.push(("  ARN".to_string(), lg.arn.clone()));
            rows
        }
        CwLogGroupDetailSection::Filters => match filters_state {
            None | Some(crate::lazy::Lazy::Loading) => {
                vec![("".to_string(), "Loading filters…".to_string())]
            }
            Some(crate::lazy::Lazy::Loaded(f)) => {
                let mut rows: Vec<(String, String)> = Vec::new();

                rows.push((
                    format!("Subscription filters ({})", f.subscription_filters.len()),
                    "".to_string(),
                ));
                rows.push(("".to_string(), "".to_string()));
                if f.subscription_filters.is_empty() {
                    rows.push(("  (none)".to_string(), "".to_string()));
                } else {
                    for (name, dest) in &f.subscription_filters {
                        // dest ARN's service is a jump-free informational hint.
                        let dest_short = dest.rsplit(':').next().unwrap_or(dest.as_str());
                        rows.push((format!("  {}", name), dest_short.to_string()));
                    }
                }

                rows.push(("".to_string(), "".to_string()));
                rows.push((
                    format!("Metric filters ({})", f.metric_filters.len()),
                    "".to_string(),
                ));
                rows.push(("".to_string(), "".to_string()));
                if f.metric_filters.is_empty() {
                    rows.push(("  (none)".to_string(), "".to_string()));
                } else {
                    rows.push((
                        "  (press f on a filter to run its pattern against the logs)".to_string(),
                        "".to_string(),
                    ));
                    for (name, pattern) in &f.metric_filters {
                        rows.push((format!("  {}", name), "".to_string()));
                        if !pattern.is_empty() {
                            rows.push(("    pattern".to_string(), pattern.clone()));
                        }
                    }
                }

                rows.push(("".to_string(), "".to_string()));
                rows
            }
            Some(crate::lazy::Lazy::Error(e)) => error_rows(e),
        },
    }
}

// ── CW Metric Stream / Insight Rule / Account Policy Split Panes ─────────────

pub(super) fn render_cw_metric_stream_split(
    app: &App,
    ms: &crate::aws::services::cloudwatch::CwMetricStream,
    area: Rect,
    frame: &mut Frame,
) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 2, "");

    let mut block = theme::pane_block("Metric Stream", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let state_color = match ms.state.as_str() {
        "running" => theme::success(),
        "stopped" => theme::warning(),
        _ => theme::text_dim(),
    };
    let header_lines = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                ms.name.clone(),
                Style::default().fg(theme::text_primary()).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(ms.state.clone(), Style::default().fg(state_color)),
            Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
            Span::styled(ms.output_format.clone(), Style::default().fg(theme::text_dim())),
        ]),
        Line::raw(""),
    ];
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
    let tabs = descriptor_tabs(app, &crate::aws::services::cloudwatch::CW_METRIC_STREAM_SECTIONS);
    render_section_tab_bar(app, chunks[2], frame, &tabs);
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub(super) fn render_cw_insight_rule_split(
    app: &App,
    rule: &crate::aws::services::cloudwatch::CwInsightRule,
    area: Rect,
    frame: &mut Frame,
) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 2, "e definition");

    let mut block = theme::pane_block("Contributor Insights Rule", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let state_color = match rule.state.as_str() {
        "ENABLED" => theme::success(),
        _ => theme::text_dim(),
    };
    let kind = if rule.managed { "managed rule" } else { "custom rule" };
    let header_lines = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                rule.name.clone(),
                Style::default().fg(theme::text_primary()).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(rule.state.clone(), Style::default().fg(state_color)),
            Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
            Span::styled(kind, Style::default().fg(theme::text_dim())),
        ]),
        Line::raw(""),
    ];
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
    let tabs = descriptor_tabs(app, &crate::aws::services::cloudwatch::CW_INSIGHT_RULE_SECTIONS);
    render_section_tab_bar(app, chunks[2], frame, &tabs);
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub(super) fn render_cw_account_policy_split(
    app: &App,
    pol: &crate::aws::services::cloudwatch::CwAccountPolicy,
    area: Rect,
    frame: &mut Frame,
) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 2, "e document");

    let mut block = theme::pane_block("Logs Account Policy", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let header_lines = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                pol.policy_name.clone(),
                Style::default().fg(theme::text_primary()).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(pol.type_display.clone(), Style::default().fg(theme::aws_orange())),
            Span::styled("  ·  account-wide", Style::default().fg(theme::text_dim())),
        ]),
        Line::raw(""),
    ];
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
    let tabs = descriptor_tabs(app, &crate::aws::services::cloudwatch::CW_ACCOUNT_POLICY_SECTIONS);
    render_section_tab_bar(app, chunks[2], frame, &tabs);
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub fn cw_metric_stream_section_lines(
    ms: &crate::aws::services::cloudwatch::CwMetricStream,
    section: crate::aws::services::cloudwatch::CwMetricStreamDetailSection,
    detail_state: Option<&crate::lazy::Lazy<crate::aws::services::cloudwatch::CwMetricStreamDetail>>,
) -> Vec<(String, String)> {
    use crate::aws::services::cloudwatch::CwMetricStreamDetailSection as S;
    match section {
        S::Details => {
            let mut rows: Vec<(String, String)> = vec![
                ("Stream".to_string(), "".to_string()),
                ("".to_string(), "".to_string()),
                ("  Name".to_string(), ms.name.clone()),
                ("  State".to_string(), ms.state.clone()),
                ("  Output format".to_string(), ms.output_format.clone()),
            ];
            if let Some(created) = ms.created_ms {
                rows.push((
                    "  Created".to_string(),
                    crate::aws::services::cloudwatch::fmt_epoch_secs(created / 1000),
                ));
            }
            if let Some(updated) = ms.updated_ms {
                rows.push((
                    "  Updated".to_string(),
                    crate::aws::services::cloudwatch::fmt_epoch_secs(updated / 1000),
                ));
            }
            rows.push(("".to_string(), "".to_string()));
            rows.push(("Destination".to_string(), "".to_string()));
            rows.push(("".to_string(), "".to_string()));
            rows.push(("  Firehose".to_string(), ms.firehose_arn.clone()));
            rows.push(("".to_string(), "".to_string()));
            rows.push(("  ARN".to_string(), ms.arn.clone()));
            rows.push(("".to_string(), "".to_string()));
            rows.push((
                "  · Continuously exports CloudWatch metrics through the Firehose destination."
                    .to_string(),
                "".to_string(),
            ));
            rows
        }
        S::Filters => match detail_state {
            None | Some(crate::lazy::Lazy::Loading) => {
                vec![("".to_string(), "Loading filters…".to_string())]
            }
            Some(crate::lazy::Lazy::Loaded(d)) => {
                let mut rows: Vec<(String, String)> = Vec::new();
                let filter_rows = |rows: &mut Vec<(String, String)>,
                                   filters: &[(String, Vec<String>)]| {
                    for (ns, metrics) in filters {
                        let value = if metrics.is_empty() {
                            "all metrics".to_string()
                        } else {
                            format!("{}: {}", metrics.len(), metrics.join(", "))
                        };
                        rows.push((format!("  {}", ns), value));
                    }
                };
                if d.include_filters.is_empty() && d.exclude_filters.is_empty() {
                    rows.push(("Namespaces".to_string(), "".to_string()));
                    rows.push(("".to_string(), "".to_string()));
                    rows.push((
                        "  · no namespace filters — every metric is streamed".to_string(),
                        "".to_string(),
                    ));
                } else if !d.include_filters.is_empty() {
                    rows.push(("Included namespaces".to_string(), "".to_string()));
                    rows.push(("".to_string(), "".to_string()));
                    filter_rows(&mut rows, &d.include_filters);
                } else {
                    rows.push(("Excluded namespaces".to_string(), "".to_string()));
                    rows.push(("".to_string(), "".to_string()));
                    filter_rows(&mut rows, &d.exclude_filters);
                    rows.push(("".to_string(), "".to_string()));
                    rows.push((
                        "  · everything not excluded above is streamed".to_string(),
                        "".to_string(),
                    ));
                }

                rows.push(("".to_string(), "".to_string()));
                rows.push(("Delivery".to_string(), "".to_string()));
                rows.push(("".to_string(), "".to_string()));
                if !d.role_arn.is_empty() {
                    rows.push(("  Role".to_string(), d.role_arn.clone()));
                }
                rows.push((
                    "  Linked accounts".to_string(),
                    if d.include_linked_accounts {
                        "metrics from linked source accounts included".to_string()
                    } else {
                        "this account only".to_string()
                    },
                ));

                if !d.stats_configs.is_empty() {
                    rows.push(("".to_string(), "".to_string()));
                    rows.push(("Extra statistics".to_string(), "".to_string()));
                    rows.push(("".to_string(), "".to_string()));
                    for (stats, metrics) in &d.stats_configs {
                        rows.push((
                            format!("  {}", stats),
                            format!("{} metric(s)", metrics.len()),
                        ));
                        for m in metrics {
                            rows.push((format!("    {}", m), "".to_string()));
                        }
                    }
                }
                rows
            }
            Some(crate::lazy::Lazy::Error(e)) => error_rows(e),
        },
    }
}

pub fn cw_insight_rule_section_lines(
    rule: &crate::aws::services::cloudwatch::CwInsightRule,
    section: crate::aws::services::cloudwatch::CwInsightRuleDetailSection,
) -> Vec<(String, String)> {
    use crate::aws::services::cloudwatch::CwInsightRuleDetailSection as S;
    match section {
        S::Details => {
            let mut rows: Vec<(String, String)> = Vec::new();
            rows.push(("Rule".to_string(), "".to_string()));
            rows.push(("".to_string(), "".to_string()));
            rows.push(("  Name".to_string(), rule.name.clone()));
            rows.push(("  State".to_string(), rule.state.clone()));
            rows.push((
                "  Managed".to_string(),
                if rule.managed {
                    "yes (created by an AWS service)".to_string()
                } else {
                    "no (custom rule)".to_string()
                },
            ));
            if !rule.schema.is_empty() {
                rows.push(("  Schema".to_string(), rule.schema.clone()));
            }
            rows.push((
                "  Transformed logs".to_string(),
                if rule.on_transformed_logs {
                    "evaluated after transformer policies".to_string()
                } else {
                    "evaluated on original log events".to_string()
                },
            ));
            rows
        }
        S::Definition => {
            let mut rows: Vec<(String, String)> = Vec::new();
            rows.push(("Definition".to_string(), "".to_string()));
            rows.push(("".to_string(), "".to_string()));
            if rule.definition.is_empty() {
                rows.push(("  (no definition returned)".to_string(), "".to_string()));
            } else {
                for line in rule.definition_pretty().lines() {
                    rows.push((format!("  {}", line), "".to_string()));
                }
            }
            rows
        }
    }
}

pub fn cw_account_policy_section_lines(
    pol: &crate::aws::services::cloudwatch::CwAccountPolicy,
    section: crate::aws::services::cloudwatch::CwAccountPolicyDetailSection,
) -> Vec<(String, String)> {
    use crate::aws::services::cloudwatch::CwAccountPolicyDetailSection as S;
    match section {
        S::Details => {
            let mut rows: Vec<(String, String)> = vec![
                ("Policy".to_string(), "".to_string()),
                ("".to_string(), "".to_string()),
                ("  Name".to_string(), pol.policy_name.clone()),
                ("  Type".to_string(), pol.type_display.clone()),
            ];
            if !pol.scope.is_empty() {
                rows.push(("  Scope".to_string(), pol.scope.clone()));
            }
            if !pol.selection_criteria.is_empty() {
                rows.push((
                    "  Selection criteria".to_string(),
                    pol.selection_criteria.clone(),
                ));
            }
            if let Some(ms) = pol.last_updated_ms {
                rows.push((
                    "  Updated".to_string(),
                    crate::aws::services::cloudwatch::fmt_epoch_secs(ms / 1000),
                ));
            }
            if !pol.account_id.is_empty() {
                rows.push(("  Account".to_string(), pol.account_id.clone()));
            }
            rows.push(("".to_string(), "".to_string()));
            rows.push((
                "  · Applies account-wide to CloudWatch Logs (subject to the selection criteria)."
                    .to_string(),
                "".to_string(),
            ));
            rows
        }
        S::Document => {
            let mut rows: Vec<(String, String)> = Vec::new();
            rows.push(("Policy document".to_string(), "".to_string()));
            rows.push(("".to_string(), "".to_string()));
            if pol.document.is_empty() {
                rows.push(("  (no document returned)".to_string(), "".to_string()));
            } else {
                for line in pol.document.lines() {
                    rows.push((format!("  {}", line), "".to_string()));
                }
            }
            rows
        }
    }
}
