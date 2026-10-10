use super::*;

// ── EventBridge rule split pane ────────────────────────────────────────────────

pub(super) fn render_eb_rule_split(app: &App, rule: &EbRule, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 3, "");
    let mut block = theme::pane_block("EventBridge Rule", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let (state_color, state_label) = match rule.state.as_str() {
        "ENABLED" => (theme::success(), "ENABLED".to_string()),
        "DISABLED" => (theme::text_dim(), "DISABLED".to_string()),
        other => (theme::warning(), other.to_string()),
    };
    let kind = if rule.schedule.is_some() {
        "schedule"
    } else if rule.event_pattern.is_some() {
        "event pattern"
    } else {
        "rule"
    };
    let header = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                rule.name.clone(),
                Style::default().fg(crate::ui::theme::text_primary()).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(format!("{} · {}", rule.bus_name, kind), Style::default().fg(theme::text_dim())),
            Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
            Span::styled(state_label, Style::default().fg(state_color)),
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
        &descriptor_tabs(app, &crate::aws::services::eventbridge::EB_RULE_SECTIONS),
    );
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub fn eb_rule_section_lines(
    rule: &EbRule,
    section: EbRuleDetailSection,
    targets: Option<&crate::lazy::Lazy<Vec<crate::aws::services::eventbridge::EbTarget>>>,
) -> Vec<(String, String)> {
    match section {
        EbRuleDetailSection::Trigger => {
            let mut rows = vec![
                ("Name".to_string(), rule.name.clone()),
                ("State".to_string(), rule.state.clone()),
                ("Bus".to_string(), rule.bus_name.clone()),
            ];
            if !rule.description.is_empty() {
                rows.push(("Description".to_string(), rule.description.clone()));
            }
            if let Some(m) = &rule.managed_by {
                rows.push(("Managed By".to_string(), format!("⚠ {} (system rule)", m)));
            }
            rows.push((String::new(), String::new()));
            if let Some(sched) = &rule.schedule {
                rows.push(("Schedule".to_string(), String::new())); // group header
                rows.push((format!("  {}", sched), String::new()));
            } else if let Some(_pattern) = &rule.event_pattern {
                rows.push(("Event Pattern".to_string(), String::new())); // group header
                let pretty = rule.pretty_pattern().unwrap_or_default();
                for line in pretty.lines() {
                    rows.push((format!("  {}", line), String::new()));
                }
            } else {
                rows.push(("Trigger".to_string(), "(none — neither schedule nor pattern)".to_string()));
            }
            rows.push((String::new(), String::new()));
            rows.push(("ARN".to_string(), rule.arn.clone()));
            rows
        }
        EbRuleDetailSection::Targets => match targets {
            None | Some(crate::lazy::Lazy::Loading) => {
                vec![("".to_string(), "Loading targets…".to_string())]
            }
            Some(crate::lazy::Lazy::Error(e)) => {
                error_rows(e)
            }
            Some(crate::lazy::Lazy::Loaded(ts)) => {
                if ts.is_empty() {
                    return vec![("".to_string(), "No targets".to_string())];
                }
                let mut rows = vec![(format!("Targets ({})", ts.len()), String::new())];
                rows.push((String::new(), String::new()));
                for t in ts {
                    // Header line: kind + id. ARN row is a cross-service jump anchor.
                    rows.push((format!("{} · {}", t.kind, t.id), String::new()));
                    rows.push(("  ARN".to_string(), t.arn.clone()));
                    rows.push(("  Input".to_string(), t.input_summary.clone()));
                    if let Some(dlq) = &t.dead_letter {
                        rows.push(("  Dead Letter".to_string(), dlq.clone()));
                    }
                    rows.push((String::new(), String::new()));
                }
                rows
            }
        },
        EbRuleDetailSection::Tags => tag_rows(&rule.tags),
    }
}

// ── EventBridge: event-bus split pane ───────────────────────────────────────────

pub(super) fn render_eb_event_bus_split(app: &App, bus: &EbEventBus, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 3, "");
    let mut block = theme::pane_block("Event Bus", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let header = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                bus.name.clone(),
                Style::default().fg(crate::ui::theme::text_primary()).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(bus.kind.clone(), Style::default().fg(theme::text_dim())),
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
        &descriptor_tabs(app, &crate::aws::services::eventbridge::EB_EVENT_BUS_SECTIONS),
    );
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub fn eb_event_bus_section_lines(
    bus: &EbEventBus,
    section: EbEventBusDetailSection,
) -> Vec<(String, String)> {
    match section {
        EbEventBusDetailSection::Overview => {
            vec![
                ("".to_string(), "".to_string()),
                ("Name".to_string(), bus.name.clone()),
                ("Type".to_string(), bus.kind.clone()),
                ("ARN".to_string(), bus.arn.clone()),
                (
                    "Resource Policy".to_string(),
                    if bus.policy.is_some() {
                        "✓ set (see Permissions)".to_string()
                    } else {
                        "none".to_string()
                    },
                ),
            ]
        }
        EbEventBusDetailSection::Permissions => {
            let mut rows = vec![("".to_string(), "".to_string())];
            match &bus.policy {
                None => {
                    rows.push(("  No resource policy".to_string(), "".to_string()));
                    rows.push(("".to_string(), "".to_string()));
                    rows.push((
                        "  Nothing outside this account can PutEvents to this bus.".to_string(),
                        "".to_string(),
                    ));
                }
                Some(_) => {
                    // Pretty-print the JSON policy as plain content lines; `e`
                    // opens the raw document in $EDITOR.
                    rows.push(("Resource Policy (e to edit)".to_string(), "".to_string()));
                    rows.push(("".to_string(), "".to_string()));
                    for line in bus.pretty_policy().unwrap_or_default().lines() {
                        rows.push((format!("  {}", line), "".to_string()));
                    }
                }
            }
            rows.push(("".to_string(), "".to_string()));
            rows
        }
        EbEventBusDetailSection::Tags => tag_rows(&bus.tags),
    }
}

// ── EventBridge: schedule split pane ────────────────────────────────────────────

pub(super) fn render_eb_schedule_split(app: &App, sched: &EbSchedule, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 2, "");
    let mut block = theme::pane_block("Schedule", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let (state_color, state_label) = match sched.state.as_str() {
        "ENABLED" => (theme::success(), "ENABLED".to_string()),
        "DISABLED" => (theme::text_dim(), "DISABLED".to_string()),
        other => (theme::warning(), other.to_string()),
    };
    let header = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                sched.name.clone(),
                Style::default().fg(crate::ui::theme::text_primary()).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                format!("group: {}", sched.group),
                Style::default().fg(theme::text_dim()),
            ),
            Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
            Span::styled(state_label, Style::default().fg(state_color)),
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
        &descriptor_tabs(app, &crate::aws::services::eventbridge::EB_SCHEDULE_SECTIONS),
    );
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub fn eb_schedule_section_lines(
    sched: &EbSchedule,
    section: EbScheduleDetailSection,
    detail: Option<&crate::lazy::Lazy<Box<crate::aws::services::eventbridge::EbScheduleDetail>>>,
) -> Vec<(String, String)> {
    // Both sections need the GetSchedule payload (summaries have no expression).
    let loaded = match detail {
        None | Some(crate::lazy::Lazy::Loading) => None,
        Some(crate::lazy::Lazy::Error(e)) => return error_rows(e),
        Some(crate::lazy::Lazy::Loaded(d)) => Some(d),
    };
    match section {
        EbScheduleDetailSection::Overview => {
            let mut rows = vec![
                ("Name".to_string(), sched.name.clone()),
                ("Group".to_string(), sched.group.clone()),
                ("State".to_string(), sched.state.clone()),
            ];
            match loaded {
                None => {
                    rows.push((String::new(), String::new()));
                    rows.push(("".to_string(), "Loading schedule…".to_string()));
                }
                Some(d) => {
                    if let Some(desc) = &d.description {
                        rows.push(("Description".to_string(), desc.clone()));
                    }
                    rows.push((String::new(), String::new()));
                    rows.push(("Schedule".to_string(), String::new()));
                    rows.push(("  Expression".to_string(), d.expression.clone()));
                    if let Some(tz) = &d.timezone {
                        rows.push(("  Timezone".to_string(), tz.clone()));
                    }
                    let window = match (d.window_mode.as_str(), d.window_minutes) {
                        ("FLEXIBLE", Some(m)) => format!("FLEXIBLE ({} min)", m),
                        (mode, _) => mode.to_string(),
                    };
                    rows.push(("  Flexible Window".to_string(), window));
                    if let Some(s) = &d.start_date {
                        rows.push(("  Start Date".to_string(), s.clone()));
                    }
                    if let Some(e) = &d.end_date {
                        rows.push(("  End Date".to_string(), e.clone()));
                    }
                    if let Some(a) = &d.after_completion {
                        rows.push(("  After Completion".to_string(), a.clone()));
                    }
                    if let Some(kms) = &d.kms_key_arn {
                        rows.push(("KMS Key".to_string(), kms.clone()));
                    }
                }
            }
            rows.push((String::new(), String::new()));
            if let Some(c) = &sched.created {
                rows.push(("Created".to_string(), c.clone()));
            }
            if let Some(m) = &sched.modified {
                rows.push(("Modified".to_string(), m.clone()));
            }
            rows.push(("ARN".to_string(), sched.arn.clone()));
            rows
        }
        EbScheduleDetailSection::Target => match loaded {
            None => vec![("".to_string(), "Loading target…".to_string())],
            Some(d) => {
                let mut rows = Vec::new();
                rows.push((
                    "Target ARN".to_string(),
                    d.target_arn
                        .clone()
                        .or_else(|| sched.target_arn.clone())
                        .unwrap_or_else(|| "—".to_string()),
                ));
                if let Some(role) = &d.role_arn {
                    rows.push(("Execution Role".to_string(), role.clone()));
                }
                if let Some(input) = &d.input {
                    rows.push((String::new(), String::new()));
                    rows.push(("Input".to_string(), String::new()));
                    for line in input.lines() {
                        rows.push((format!("  {}", line), String::new()));
                    }
                }
                rows.push((String::new(), String::new()));
                rows.push(("Retry".to_string(), String::new()));
                rows.push((
                    "  Max Attempts".to_string(),
                    d.retry_max_attempts
                        .map(|n| n.to_string())
                        .unwrap_or_else(|| "185 (default)".to_string()),
                ));
                rows.push((
                    "  Max Event Age".to_string(),
                    d.retry_max_age_secs
                        .map(|s| format!("{} s", s))
                        .unwrap_or_else(|| "86400 s (default)".to_string()),
                ));
                if let Some(dlq) = &d.dead_letter_arn {
                    rows.push(("  Dead Letter Queue".to_string(), dlq.clone()));
                }
                rows
            }
        },
    }
}

// ── EventBridge: pipe split pane ────────────────────────────────────────────────

pub(super) fn render_eb_pipe_split(app: &App, pipe: &EbPipe, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 3, "");
    let mut block = theme::pane_block("Pipe", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let (state_color, state_label) = match pipe.current_state.as_str() {
        "RUNNING" => (theme::success(), "RUNNING".to_string()),
        "STOPPED" => (theme::text_dim(), "STOPPED".to_string()),
        s if s.ends_with("_FAILED") => (theme::error(), s.to_string()),
        other => (theme::warning(), other.to_string()),
    };
    let header = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                pipe.name.clone(),
                Style::default().fg(crate::ui::theme::text_primary()).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                if pipe.enrichment.is_some() {
                    "source → enrichment → target"
                } else {
                    "source → target"
                },
                Style::default().fg(theme::text_dim()),
            ),
            Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
            Span::styled(state_label, Style::default().fg(state_color)),
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
        &descriptor_tabs(app, &crate::aws::services::eventbridge::EB_PIPE_SECTIONS),
    );
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub fn eb_pipe_section_lines(
    pipe: &EbPipe,
    section: EbPipeDetailSection,
    detail: Option<&crate::lazy::Lazy<crate::aws::services::eventbridge::EbPipeDetail>>,
) -> Vec<(String, String)> {
    match section {
        EbPipeDetailSection::Overview => {
            let mut rows = vec![
                ("Name".to_string(), pipe.name.clone()),
                ("State".to_string(), pipe.current_state.clone()),
            ];
            if pipe.desired_state != pipe.current_state && !pipe.desired_state.is_empty() {
                rows.push(("Desired State".to_string(), pipe.desired_state.clone()));
            }
            if let Some(reason) = &pipe.state_reason {
                rows.push(("State Reason".to_string(), format!("⚠ {}", reason)));
            }
            rows.push((String::new(), String::new()));
            rows.push(("Pipeline".to_string(), String::new()));
            rows.push(("  Source".to_string(), pipe.source.clone()));
            if let Some(enrichment) = &pipe.enrichment {
                rows.push(("  Enrichment".to_string(), enrichment.clone()));
            }
            rows.push(("  Target".to_string(), pipe.target.clone()));
            rows.push((String::new(), String::new()));
            if let Some(c) = &pipe.created {
                rows.push(("Created".to_string(), c.clone()));
            }
            if let Some(m) = &pipe.modified {
                rows.push(("Modified".to_string(), m.clone()));
            }
            rows.push(("ARN".to_string(), pipe.arn.clone()));
            rows
        }
        EbPipeDetailSection::Configuration => match detail {
            None | Some(crate::lazy::Lazy::Loading) => {
                vec![("".to_string(), "Loading configuration…".to_string())]
            }
            Some(crate::lazy::Lazy::Error(e)) => error_rows(e),
            Some(crate::lazy::Lazy::Loaded(d)) => {
                let mut rows = Vec::new();
                if let Some(desc) = &d.description {
                    rows.push(("Description".to_string(), desc.clone()));
                }
                if let Some(role) = &d.role_arn {
                    rows.push(("Execution Role".to_string(), role.clone()));
                }
                if let Some(kms) = &d.kms_key {
                    rows.push(("KMS Key".to_string(), kms.clone()));
                }
                if rows.is_empty() && d.filter_patterns.is_empty() && d.log_level.is_none() {
                    return vec![("".to_string(), "No additional configuration".to_string())];
                }
                if !d.filter_patterns.is_empty() {
                    rows.push((String::new(), String::new()));
                    rows.push((
                        format!("Event Filters ({})", d.filter_patterns.len()),
                        String::new(),
                    ));
                    for pat in &d.filter_patterns {
                        for line in pat.lines() {
                            rows.push((format!("  {}", line), String::new()));
                        }
                        rows.push((String::new(), String::new()));
                    }
                }
                if d.log_level.is_some() || d.log_destination.is_some() {
                    rows.push((String::new(), String::new()));
                    rows.push(("Logging".to_string(), String::new()));
                    if let Some(level) = &d.log_level {
                        rows.push(("  Level".to_string(), level.clone()));
                    }
                    if let Some(dest) = &d.log_destination {
                        rows.push(("  Destination".to_string(), dest.clone()));
                    }
                }
                rows
            }
        },
        EbPipeDetailSection::Tags => match detail {
            None | Some(crate::lazy::Lazy::Loading) => {
                vec![("".to_string(), "Loading tags…".to_string())]
            }
            Some(crate::lazy::Lazy::Error(e)) => error_rows(e),
            Some(crate::lazy::Lazy::Loaded(d)) => tag_rows(&d.tags),
        },
    }
}
