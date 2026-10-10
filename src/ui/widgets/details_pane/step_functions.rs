use super::*;

// ── Step Functions state-machine split pane ────────────────────────────────────

pub(super) fn render_sfn_split(app: &App, sm: &SfnStateMachine, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 4, "");

    let mut block = theme::pane_block("State Machine", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let header_lines = build_sfn_header_lines(sm);
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
    render_sfn_section_tabs(app, chunks[2], frame);
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub(super) fn build_sfn_header_lines(sm: &SfnStateMachine) -> Vec<Line<'static>> {
    let mut lines = vec![Line::raw("")];

    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            sm.name.clone(),
            Style::default()
                .fg(crate::ui::theme::text_primary())
                .add_modifier(Modifier::BOLD),
        ),
    ]));

    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            SfnStateMachine::type_label(&sm.kind).to_string(),
            Style::default().fg(theme::text_dim()),
        ),
    ]));

    lines.push(Line::raw(""));
    lines
}

pub(super) fn render_sfn_section_tabs(app: &App, area: Rect, frame: &mut Frame) {
    render_section_tab_bar(
        app,
        area,
        frame,
        &descriptor_tabs(app, &crate::aws::services::step_functions::SFN_SECTIONS),
    );
}

pub fn sfn_section_lines(
    sm: &SfnStateMachine,
    section: SfnDetailSection,
    details_state: Option<&crate::lazy::Lazy<Box<crate::aws::services::step_functions::SfnDetails>>>,
    executions: &[&crate::aws::services::step_functions::SfnExecution],
) -> Vec<(String, String)> {
    match section {
        SfnDetailSection::Details => sfn_details_lines(sm, details_state),
        SfnDetailSection::Definition => sfn_definition_lines(details_state),
        SfnDetailSection::Executions => sfn_executions_lines(sm, executions),
        SfnDetailSection::Tags => sfn_tags_lines(details_state),
    }
}

pub(super) fn sfn_details_lines(
    sm: &SfnStateMachine,
    details_state: Option<&crate::lazy::Lazy<Box<crate::aws::services::step_functions::SfnDetails>>>,
) -> Vec<(String, String)> {
    let mut rows = vec![
        ("Name".to_string(), sm.name.clone()),
        ("ARN".to_string(), sm.arn.clone()),
        (
            "Type".to_string(),
            SfnStateMachine::type_label(&sm.kind).to_string(),
        ),
    ];

    match details_state {
        None | Some(crate::lazy::Lazy::Loading) => {
            rows.push(("".to_string(), "".to_string()));
            rows.push(("".to_string(), "Loading state machine details…".to_string()));
        }
        Some(crate::lazy::Lazy::Error(e)) => {
            rows.extend(error_rows(e));
        }
        Some(crate::lazy::Lazy::Loaded(d)) => {
            // Prefer the authoritative type from describe over the list summary.
            if let Some(type_row) = rows.iter_mut().find(|(k, _)| k == "Type") {
                type_row.1 = SfnStateMachine::type_label(&d.kind).to_string();
            }
            rows.push(("Status".to_string(), d.status.clone()));
            if let Some(created) = &d.created {
                rows.push(("Created".to_string(), created.clone()));
            }
            rows.push(("IAM Role".to_string(), d.role_arn.clone()));

            // Logging — the `t` tail resolves the same group, so surfacing it
            // here doubles as the explanation when `t` says logging is off.
            rows.push(("".to_string(), "".to_string()));
            rows.push(("Logging".to_string(), "".to_string()));
            rows.push(("  Level".to_string(), d.logging_level.clone()));
            match &d.log_group {
                Some(g) => {
                    rows.push(("  Log Group".to_string(), g.clone()));
                    rows.push((
                        "  Execution Data".to_string(),
                        if d.include_execution_data {
                            "included".to_string()
                        } else {
                            "not included".to_string()
                        },
                    ));
                    rows.push(("".to_string(), "  · t to tail these logs".to_string()));
                }
                None => {
                    rows.push((
                        "".to_string(),
                        "  ⚠ No CloudWatch Logs destination — execution logs aren't recorded"
                            .to_string(),
                    ));
                }
            }
            rows.push((
                "  X-Ray Tracing".to_string(),
                if d.tracing_enabled {
                    "✓ enabled".to_string()
                } else {
                    "disabled".to_string()
                },
            ));
        }
    }

    rows.push(("".to_string(), "".to_string()));
    rows
}

pub(super) fn sfn_definition_lines(details_state: Option<&crate::lazy::Lazy<Box<crate::aws::services::step_functions::SfnDetails>>>) -> Vec<(String, String)> {
    match details_state {
        None | Some(crate::lazy::Lazy::Loading) => {
            vec![("".to_string(), "Loading definition…".to_string())]
        }
        Some(crate::lazy::Lazy::Error(e)) => error_rows(e),
        Some(crate::lazy::Lazy::Loaded(d)) => {
            let line_count = d.definition.lines().count();
            vec![
                (
                    "Amazon States Language definition".to_string(),
                    "".to_string(),
                ),
                ("".to_string(), "".to_string()),
                ("Lines".to_string(), line_count.to_string()),
            ]
        }
    }
}

/// Executions are first-class rows on the Executions sub-tab, so this section
/// filters the siblings already loaded — zero fetch, and every ARN row is
/// `Enter`-jumpable into that execution's own pane.
pub(super) fn sfn_executions_lines(
    sm: &SfnStateMachine,
    executions: &[&crate::aws::services::step_functions::SfnExecution],
) -> Vec<(String, String)> {
    if sm.is_express() {
        return vec![
            (
                "".to_string(),
                "EXPRESS executions aren't retained by ListExecutions.".to_string(),
            ),
            (
                "".to_string(),
                "Tail the state machine's CloudWatch log group with `t` instead.".to_string(),
            ),
        ];
    }

    if executions.is_empty() {
        return vec![("".to_string(), "No recent executions.".to_string())];
    }

    let running = executions.iter().filter(|e| e.is_running()).count();
    let failed = executions.iter().filter(|e| e.failed()).count();

    let mut rows = vec![(
        "Recent Executions".to_string(),
        format!("{} shown", executions.len()),
    )];
    if running > 0 || failed > 0 {
        let mut parts = Vec::new();
        if running > 0 {
            parts.push(format!("{} running", running));
        }
        if failed > 0 {
            parts.push(format!("{} failed", failed));
        }
        rows.push(("  Status".to_string(), parts.join(", ")));
    }
    rows.push(("".to_string(), "".to_string()));

    for e in executions {
        rows.push((e.name.clone(), e.status.clone()));
        // Key-value so the ARN classifies as a jump into the execution pane.
        rows.push(("  Execution".to_string(), e.arn.clone()));
        if let Some(started) = &e.started {
            rows.push(("  Started".to_string(), started.clone()));
        }
        rows.push(("  Duration".to_string(), e.duration_label()));
        rows.push(("".to_string(), "".to_string()));
    }
    rows
}

pub(super) fn sfn_tags_lines(details_state: Option<&crate::lazy::Lazy<Box<crate::aws::services::step_functions::SfnDetails>>>) -> Vec<(String, String)> {
    match details_state {
        None | Some(crate::lazy::Lazy::Loading) => {
            vec![("".to_string(), "Loading tags…".to_string())]
        }
        Some(crate::lazy::Lazy::Error(e)) => error_rows(e),
        Some(crate::lazy::Lazy::Loaded(d)) => {
            let mut rows = vec![];
            if d.tags.is_empty() {
                rows.push(("".to_string(), "No tags".to_string()));
            } else {
                let mut sorted: Vec<(&String, &String)> = d.tags.iter().collect();
                sorted.sort_by_key(|(k, _)| k.as_str());
                for (key, value) in sorted {
                    rows.push((format!("  {}", key), value.clone()));
                }
            }
            rows.push(("".to_string(), "".to_string()));
            rows
        }
    }
}

// ── Step Functions execution split pane ────────────────────────────────────────

pub(super) fn render_sfn_exec_split(app: &App, exec: &SfnExecution, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 4, "");

    let mut block = theme::pane_block("Execution", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let header_lines = build_sfn_exec_header_lines(exec);
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
    render_section_tab_bar(
        app,
        chunks[2],
        frame,
        &descriptor_tabs(app, &crate::aws::services::step_functions::SFN_EXEC_SECTIONS),
    );
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub(super) fn build_sfn_exec_header_lines(exec: &SfnExecution) -> Vec<Line<'static>> {
    let mut lines = vec![Line::raw("")];

    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            exec.name.clone(),
            Style::default()
                .fg(theme::text_primary())
                .add_modifier(Modifier::BOLD),
        ),
    ]));

    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            exec.status.clone(),
            Style::default()
                .fg(theme::state_indicator(&execution_status_state(&exec.status)).1)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  ·  {}", exec.duration_label()),
            Style::default().fg(theme::text_dim()),
        ),
        Span::styled(
            format!("  ·  {}", exec.state_machine_name),
            Style::default().fg(theme::text_dim()),
        ),
    ]));

    lines.push(Line::raw(""));
    lines
}

pub fn sfn_exec_section_lines(
    exec: &SfnExecution,
    section: SfnExecDetailSection,
    detail_state: Option<
        &crate::lazy::Lazy<Box<crate::aws::services::step_functions::SfnExecutionDetail>>,
    >,
    history_state: Option<
        &crate::lazy::Lazy<Box<crate::aws::services::step_functions::SfnHistory>>,
    >,
) -> Vec<(String, String)> {
    match section {
        SfnExecDetailSection::Overview => sfn_exec_overview_lines(exec, detail_state),
        SfnExecDetailSection::Input => sfn_exec_payload_lines(detail_state, true),
        SfnExecDetailSection::Output => sfn_exec_payload_lines(detail_state, false),
        SfnExecDetailSection::History => sfn_exec_history_lines(exec, history_state),
    }
}

pub(super) fn sfn_exec_overview_lines(
    exec: &SfnExecution,
    detail_state: Option<
        &crate::lazy::Lazy<Box<crate::aws::services::step_functions::SfnExecutionDetail>>,
    >,
) -> Vec<(String, String)> {
    let mut rows = vec![
        ("Name".to_string(), exec.name.clone()),
        ("Status".to_string(), exec.status.clone()),
        ("ARN".to_string(), exec.arn.clone()),
        // Key-value so the ARN jumps back to the State Machines sub-tab.
        ("State Machine".to_string(), exec.state_machine_arn.clone()),
    ];
    if let Some(started) = &exec.started {
        rows.push(("Started".to_string(), started.clone()));
    }
    match &exec.stopped {
        Some(stopped) => rows.push(("Stopped".to_string(), stopped.clone())),
        None => rows.push(("Stopped".to_string(), "— still running".to_string())),
    }
    rows.push(("Duration".to_string(), exec.duration_label()));
    if let Some(map_run) = &exec.map_run_arn {
        rows.push(("Map Run".to_string(), map_run.clone()));
    }
    if let Some(n) = exec.item_count {
        rows.push(("Map Items".to_string(), n.to_string()));
    }
    // From the list item, so a redriven execution reads as one before the
    // describe lands (which then adds the date and eligibility).
    if exec.redrive_count.unwrap_or(0) > 0 {
        rows.push((
            "Redriven".to_string(),
            format!("{} time(s)", exec.redrive_count.unwrap_or(0)),
        ));
    }

    match detail_state {
        None | Some(crate::lazy::Lazy::Loading) => {
            rows.push(("".to_string(), "".to_string()));
            rows.push(("".to_string(), "Loading execution details…".to_string()));
        }
        Some(crate::lazy::Lazy::Error(e)) => rows.extend(error_rows(e)),
        Some(crate::lazy::Lazy::Loaded(d)) => {
            if d.error.is_some() || d.cause.is_some() {
                rows.push(("".to_string(), "".to_string()));
                rows.push(("Failure".to_string(), "".to_string()));
                if let Some(err) = &d.error {
                    rows.push(("  Error".to_string(), err.clone()));
                }
                if let Some(cause) = &d.cause {
                    // The cause is often a stack trace or a nested JSON blob —
                    // show it in full as content lines rather than truncating.
                    push_wrapped_content(&mut rows, cause, 40);
                }
            }
            if d.redrive_count.unwrap_or(0) > 0 || d.redrive_status.is_some() {
                rows.push(("".to_string(), "".to_string()));
                rows.push(("Redrive".to_string(), "".to_string()));
                if let Some(n) = d.redrive_count {
                    rows.push(("  Count".to_string(), n.to_string()));
                }
                if let Some(date) = &d.redrive_date {
                    rows.push(("  Last Redriven".to_string(), date.clone()));
                }
                if let Some(status) = &d.redrive_status {
                    rows.push(("  Eligible".to_string(), status.clone()));
                }
                if let Some(reason) = &d.redrive_status_reason {
                    rows.push(("  Reason".to_string(), reason.clone()));
                }
            }
            if d.state_machine_alias_arn.is_some() || d.state_machine_version_arn.is_some() {
                rows.push(("".to_string(), "".to_string()));
                rows.push(("Revision".to_string(), "".to_string()));
                if let Some(a) = &d.state_machine_alias_arn {
                    rows.push(("  Alias".to_string(), a.clone()));
                }
                if let Some(v) = &d.state_machine_version_arn {
                    rows.push(("  Version".to_string(), v.clone()));
                }
            }
            if let Some(trace) = &d.trace_header {
                rows.push(("".to_string(), "".to_string()));
                rows.push(("X-Ray Trace".to_string(), trace.clone()));
            }
        }
    }

    rows.push(("".to_string(), "".to_string()));
    rows
}

/// Input and Output share one `DescribeExecution`, so they share a renderer.
pub(super) fn sfn_exec_payload_lines(
    detail_state: Option<
        &crate::lazy::Lazy<Box<crate::aws::services::step_functions::SfnExecutionDetail>>,
    >,
    input: bool,
) -> Vec<(String, String)> {
    let what = if input { "input" } else { "output" };
    match detail_state {
        None | Some(crate::lazy::Lazy::Loading) => {
            vec![("".to_string(), format!("Loading {}…", what))]
        }
        Some(crate::lazy::Lazy::Error(e)) => error_rows(e),
        Some(crate::lazy::Lazy::Loaded(d)) => {
            let (payload, truncated) = if input {
                (&d.input, d.input_truncated)
            } else {
                (&d.output, d.output_truncated)
            };
            let mut rows = Vec::new();
            if truncated {
                rows.push((
                    "".to_string(),
                    format!(
                        "  ⚠ The {} exceeded the inline size limit and was written to S3",
                        what
                    ),
                ));
                rows.push(("".to_string(), "".to_string()));
            }
            match payload {
                Some(p) if !p.is_empty() => {
                    rows.push(("".to_string(), "  · e opens the raw JSON".to_string()));
                    rows.push(("".to_string(), "".to_string()));
                    push_wrapped_content(&mut rows, p, 400);
                }
                _ if !input => {
                    // A running or failed execution has no output at all; say
                    // which, rather than showing an empty section.
                    rows.push((
                        "".to_string(),
                        if d.error.is_some() {
                            "No output — the execution failed (see Overview for the cause)"
                                .to_string()
                        } else {
                            "No output yet — the execution hasn't completed".to_string()
                        },
                    ));
                }
                _ => rows.push(("".to_string(), format!("No {}", what))),
            }
            rows
        }
    }
}

pub(super) fn sfn_exec_history_lines(
    exec: &SfnExecution,
    history_state: Option<
        &crate::lazy::Lazy<Box<crate::aws::services::step_functions::SfnHistory>>,
    >,
) -> Vec<(String, String)> {
    use crate::aws::services::step_functions::{fmt_duration, fmt_epoch_time};

    match history_state {
        None | Some(crate::lazy::Lazy::Loading) => {
            vec![("".to_string(), "Loading execution history…".to_string())]
        }
        Some(crate::lazy::Lazy::Error(e)) => error_rows(e),
        Some(crate::lazy::Lazy::Loaded(h)) => {
            let mut rows = Vec::new();

            if h.capped {
                rows.push((
                    "".to_string(),
                    "  ⚠ Long history — showing the most recent events only".to_string(),
                ));
                rows.push(("".to_string(), "".to_string()));
            }

            if let Some((error, cause)) = &h.failure {
                rows.push(("Failure".to_string(), "".to_string()));
                rows.push(("  Error".to_string(), error.clone()));
                if !cause.is_empty() {
                    push_wrapped_content(&mut rows, cause, 20);
                }
                rows.push(("".to_string(), "".to_string()));
            }

            // ── The states, in order, with how long each took. The one still
            // open on a RUNNING execution is what it's doing right now.
            rows.push(("States".to_string(), format!("{}", h.spans.len())));
            if h.spans.is_empty() {
                rows.push((
                    "".to_string(),
                    "  No state transitions recorded yet".to_string(),
                ));
            }
            for span in &h.spans {
                let marker = if span.error.is_some() {
                    "✗"
                } else if span.is_open() {
                    if exec.is_running() {
                        "▸"
                    } else {
                        // Not running and never exited — the execution ended
                        // inside this state.
                        "⚠"
                    }
                } else {
                    "✓"
                };
                let timing = match span.duration_secs {
                    Some(d) => fmt_duration(d),
                    None if exec.is_running() => "running".to_string(),
                    None => "did not complete".to_string(),
                };
                rows.push((
                    format!("  {} {}", marker, span.name),
                    format!("{}  ({})", timing, fmt_epoch_time(span.entered_secs)),
                ));
                if let Some(res) = &span.resource {
                    // Key-value so a Lambda / SNS / ECS ARN is Enter-jumpable.
                    rows.push(("      Resource".to_string(), res.clone()));
                }
                if let Some(err) = &span.error {
                    rows.push(("      Error".to_string(), err.clone()));
                    if let Some(cause) = &span.cause {
                        push_wrapped_content(&mut rows, cause, 12);
                    }
                }
            }

            // ── The raw event stream underneath, for when the span view isn't
            // enough (choice evaluations, retries, wait states).
            rows.push(("".to_string(), "".to_string()));
            rows.push(("Events".to_string(), format!("{}", h.events.len())));
            for e in &h.events {
                let mut label = format!("  {}  #{} {}", fmt_epoch_time(e.ts_secs), e.id, e.kind);
                if let Some(state) = &e.state {
                    label.push_str(&format!("  {}", state));
                }
                rows.push((label, String::new()));
                if let Some(err) = &e.error {
                    rows.push(("".to_string(), format!("      ✗ {}", err)));
                }
            }

            rows.push(("".to_string(), "".to_string()));
            rows
        }
    }
}
