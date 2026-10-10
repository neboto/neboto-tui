use super::*;

// ── CloudFormation Stack Split Pane ───────────────────────────────────────────

pub(super) fn render_cfn_stack_split(app: &App, stack: &CfnStack, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let (title, section_count) = if stack.deleted {
        ("CFN Deleted Stack", 4)
    } else {
        ("CFN Stack", 9)
    };
    // Advertise the Events section's stream ↔ progress-rollup toggle.
    let extras = if app.cfn_events_section_active().is_some() {
        if app.cfn_events_progress { "f events" } else { "f progress" }
    } else {
        ""
    };
    let footer = detail_footer(app, focused, section_count, extras);

    let mut block = theme::pane_block(title, focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let header_lines = build_cfn_stack_header_lines(stack);
    let header_h = header_lines.len() as u16;

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(header_h),
            Constraint::Length(1), // rule
            Constraint::Length(1), // tab bar (single row, like every other pane)
            Constraint::Length(1), // rule
            Constraint::Min(0),    // body
        ])
        .split(inner);

    frame.render_widget(Paragraph::new(header_lines), chunks[0]);
    render_hr(chunks[1], frame);
    let descriptor = if stack.deleted {
        &crate::aws::services::cloudformation::CFN_DELETED_STACK_SECTIONS
    } else {
        &crate::aws::services::cloudformation::CFN_STACK_SECTIONS
    };
    render_section_tab_bar(app, chunks[2], frame, &descriptor_tabs(app, descriptor));
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub(super) fn build_cfn_stack_header_lines(stack: &CfnStack) -> Vec<Line<'static>> {
    let state_color = match stack.status.as_str() {
        s if s.ends_with("_COMPLETE") && !s.contains("ROLLBACK") && !s.contains("DELETE") => {
            theme::success()
        }
        s if s.ends_with("_IN_PROGRESS") => theme::warning(),
        s if s.ends_with("_FAILED") || s.contains("ROLLBACK") => theme::error(),
        _ => crate::ui::theme::text_muted(),
    };
    let state_dot = match stack.status.as_str() {
        s if s.ends_with("_COMPLETE") && !s.contains("ROLLBACK") && !s.contains("DELETE") => {
            "● "
        }
        s if s.ends_with("_IN_PROGRESS") => "◌ ",
        _ => "○ ",
    };

    let mut lines: Vec<Line<'static>> = vec![Line::raw("")];

    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            stack.stack_name.clone(),
            Style::default()
                .fg(crate::ui::theme::text_primary())
                .add_modifier(Modifier::BOLD),
        ),
    ]));

    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(state_dot.to_string(), Style::default().fg(state_color)),
        Span::styled(
            stack.status.clone(),
            Style::default()
                .fg(state_color)
                .add_modifier(Modifier::BOLD),
        ),
    ]));

    lines.push(Line::raw(""));
    lines.push(header_kv("Created", &stack.creation_time));
    if let Some(updated) = &stack.last_updated {
        lines.push(header_kv("Updated", updated));
    }
    if let Some(deleted) = &stack.deletion_time {
        lines.push(header_kv("Deleted", deleted));
    }
    if let Some(drift) = &stack.drift_status {
        lines.push(header_kv("Drift", drift));
    }
    if !stack.capabilities.is_empty() {
        lines.push(header_kv("Capabilities", &stack.capabilities.join(", ")));
    }

    lines.push(Line::raw(""));
    lines
}

/// Called by `App::get_detail_lines` — single source of truth for CFN stack body content.
pub fn cfn_stack_section_lines(
    stack: &CfnStack,
    section: CfnStackDetailSection,
    resources_state: Option<&Lazy<Vec<crate::aws::services::cloudformation::CfnStackResourceEntry>>>,
    events_state: Option<&Lazy<Vec<crate::aws::services::cloudformation::CfnStackEvent>>>,
    template_state: Option<&Lazy<String>>,
    drift_state: Option<&Lazy<Vec<crate::aws::services::cloudformation::CfnResourceDrift>>>,
    changesets_state: Option<&Lazy<Vec<crate::aws::services::cloudformation::CfnChangeSet>>>,
    policy_state: Option<&Lazy<Option<String>>>,
    events_progress: bool,
) -> Vec<(String, String)> {
    match section {
        CfnStackDetailSection::Overview => cfn_overview_lines(stack),
        CfnStackDetailSection::Resources => cfn_resources_lines(resources_state),
        CfnStackDetailSection::Outputs => cfn_outputs_lines(stack),
        CfnStackDetailSection::Parameters => cfn_parameters_lines(stack),
        CfnStackDetailSection::Tags => cfn_tags_lines(stack),
        CfnStackDetailSection::Events if events_progress => {
            cfn_progress_lines(stack, events_state, template_state)
        }
        CfnStackDetailSection::Events => cfn_events_lines(events_state),
        CfnStackDetailSection::Template => cfn_template_lines(template_state),
        CfnStackDetailSection::Drift => cfn_drift_lines(stack, drift_state),
        CfnStackDetailSection::Changes => cfn_changes_lines(changesets_state),
        CfnStackDetailSection::Policy => cfn_policy_lines(policy_state),
    }
}

/// Stack policy section: which resources are update-protected. No policy
/// means every resource is updatable — said so, rather than an empty body
/// that reads like a failed fetch.
pub(super) fn cfn_policy_lines(policy_state: Option<&Lazy<Option<String>>>) -> Vec<(String, String)> {
    let mut rows = vec![(String::new(), String::new())];
    match policy_state {
        None | Some(Lazy::Loading) => {
            rows.push(("  Loading stack policy…".to_string(), String::new()));
        }
        Some(Lazy::Error(e)) => rows.extend(error_rows(e)),
        Some(Lazy::Loaded(None)) => {
            rows.push(("  No stack policy".to_string(), String::new()));
            rows.push((
                "  · every resource can be updated or replaced (no update protection)".to_string(),
                String::new(),
            ));
        }
        Some(Lazy::Loaded(Some(body))) => {
            match crate::aws::services::cloudformation::parse_stack_policy(body) {
                Some(stmts) => {
                    let denies = stmts.iter().filter(|s| s.effect == "Deny").count();
                    rows.push((
                        "Summary".to_string(),
                        String::new(),
                    ));
                    rows.push((
                        "  Statements".to_string(),
                        format!("{} ({} deny)", stmts.len(), denies),
                    ));
                    rows.push((
                        "  Protection".to_string(),
                        if denies == 0 {
                            "none — no Deny statement".to_string()
                        } else {
                            "⚠ some updates are denied".to_string()
                        },
                    ));
                    for (i, st) in stmts.iter().enumerate() {
                        rows.push((String::new(), String::new()));
                        rows.push((format!("Statement {}", i + 1), String::new()));
                        rows.push(("  Effect".to_string(), st.effect.clone()));
                        if st.principal != "*" {
                            rows.push(("  Principal".to_string(), st.principal.clone()));
                        }
                        rows.push((format!("  {}", st.actions.0), st.actions.1.join(", ")));
                        let (rk, rv) = &st.resources;
                        if rv.len() <= 1 {
                            rows.push((format!("  {}", rk), rv.join("")));
                        } else {
                            rows.push((format!("  {}", rk), format!("{} resources", rv.len())));
                            for r in rv {
                                rows.push((format!("      {}", r), String::new()));
                            }
                        }
                        for c in &st.conditions {
                            rows.push(("  Condition".to_string(), c.clone()));
                        }
                    }
                }
                None => {
                    for line in body.lines() {
                        rows.push((format!("  {}", line), String::new()));
                    }
                }
            }
            rows.push((String::new(), String::new()));
            rows.push(("  · press e to open the policy document".to_string(), String::new()));
        }
    }
    rows.push((String::new(), String::new()));
    rows
}

/// Same, for the reduced deleted-stack pane — the shared renderers behave
/// identically; only the section set shrinks.
pub fn cfn_deleted_stack_section_lines(
    stack: &CfnStack,
    section: crate::aws::services::cloudformation::CfnDeletedStackDetailSection,
    resources_state: Option<&Lazy<Vec<crate::aws::services::cloudformation::CfnStackResourceEntry>>>,
    events_state: Option<&Lazy<Vec<crate::aws::services::cloudformation::CfnStackEvent>>>,
    template_state: Option<&Lazy<String>>,
    events_progress: bool,
) -> Vec<(String, String)> {
    use crate::aws::services::cloudformation::CfnDeletedStackDetailSection as Del;
    match section {
        Del::Overview => cfn_overview_lines(stack),
        Del::Resources => cfn_resources_lines(resources_state),
        Del::Events if events_progress => cfn_progress_lines(stack, events_state, template_state),
        Del::Events => cfn_events_lines(events_state),
        Del::Template => cfn_template_lines(template_state),
    }
}

pub(super) fn cfn_overview_lines(stack: &CfnStack) -> Vec<(String, String)> {
    let mut rows = vec![];
    rows.push(("Stack Name".to_string(), stack.stack_name.clone()));
    rows.push(("Status".to_string(), stack.status.clone()));
    if let Some(reason) = &stack.status_reason {
        rows.push(("Status Reason".to_string(), reason.clone()));
    }
    rows.push(("".to_string(), "".to_string()));
    rows.push(("Stack ID".to_string(), stack.stack_id.clone()));
    rows.push(("Created".to_string(), stack.creation_time.clone()));
    if let Some(updated) = &stack.last_updated {
        rows.push(("Last Updated".to_string(), updated.clone()));
    }
    if let Some(deleted) = &stack.deletion_time {
        rows.push(("Deleted".to_string(), deleted.clone()));
    }
    if let Some(desc) = &stack.description {
        rows.push(("".to_string(), "".to_string()));
        rows.push(("Description".to_string(), desc.clone()));
    }
    if let Some(drift) = &stack.drift_status {
        rows.push(("Drift Status".to_string(), drift.clone()));
    }
    if !stack.capabilities.is_empty() {
        rows.push(("Capabilities".to_string(), stack.capabilities.join(", ")));
    }

    if stack.deleted {
        rows.push(("".to_string(), "".to_string()));
        rows.push((
            "  · retained ~90 days after deletion — Resources shows final statuses; ⚠ DELETE_SKIPPED rows were left behind".to_string(),
            "".to_string(),
        ));
        if stack.parent_id.is_some() || stack.root_id.is_some() {
            rows.push(("".to_string(), "".to_string()));
            rows.push(("Nesting".to_string(), "".to_string()));
            if let Some(parent) = &stack.parent_id {
                rows.push(("Parent Stack".to_string(), parent.clone()));
            }
            if let Some(root) = &stack.root_id {
                rows.push(("Root Stack".to_string(), root.clone()));
            }
        }
        rows.push(("".to_string(), "".to_string()));
        return rows;
    }

    // Protection / rollback / role — high-signal for "why won't this delete /
    // why did it fail as the wrong principal."
    rows.push(("".to_string(), "".to_string()));
    rows.push(("Protection".to_string(), "".to_string()));
    rows.push((
        "Termination Protection".to_string(),
        match stack.termination_protection {
            Some(true) => "Enabled".to_string(),
            Some(false) => "Disabled".to_string(),
            None => "—".to_string(),
        },
    ));
    if let Some(dr) = stack.disable_rollback {
        rows.push((
            "Rollback on Failure".to_string(),
            if dr { "Disabled" } else { "Enabled" }.to_string(),
        ));
    }
    if let Some(t) = stack.timeout_minutes {
        rows.push(("Creation Timeout".to_string(), format!("{} min", t)));
    }
    if let Some(role) = &stack.role_arn {
        rows.push(("Service Role".to_string(), role.clone()));
    }
    if let Some(m) = stack.rollback_monitoring_minutes {
        rows.push(("Rollback Monitoring".to_string(), format!("{} min", m)));
    }
    if let Some((first, rest)) = stack.rollback_triggers.split_first() {
        rows.push(("Rollback Triggers".to_string(), first.clone()));
        for arn in rest {
            rows.push((format!("  {}", arn), String::new()));
        }
    }
    if let Some((first, rest)) = stack.notification_arns.split_first() {
        rows.push(("Notification ARNs".to_string(), first.clone()));
        for arn in rest {
            rows.push((format!("  {}", arn), String::new()));
        }
    }

    if stack.parent_id.is_some() || stack.root_id.is_some() {
        rows.push(("".to_string(), "".to_string()));
        rows.push(("Nesting".to_string(), "".to_string()));
        if let Some(parent) = &stack.parent_id {
            rows.push(("Parent Stack".to_string(), parent.clone()));
        }
        if let Some(root) = &stack.root_id {
            rows.push(("Root Stack".to_string(), root.clone()));
        }
    }
    rows.push(("".to_string(), "".to_string()));
    rows
}

pub(super) fn cfn_resources_lines(resources_state: Option<&Lazy<Vec<crate::aws::services::cloudformation::CfnStackResourceEntry>>>) -> Vec<(String, String)> {
    match resources_state {
        None | Some(Lazy::Loading) => {
            vec![
                ("".to_string(), "".to_string()),
                ("  Loading stack resources...".to_string(), "".to_string()),
                ("".to_string(), "".to_string()),
            ]
        }
        Some(Lazy::Error(e)) => error_rows(e),
        Some(Lazy::Loaded(entries)) => {
            if entries.is_empty() {
                return vec![
                    ("".to_string(), "".to_string()),
                    ("  No resources found".to_string(), "".to_string()),
                    ("".to_string(), "".to_string()),
                ];
            }
            let mut rows = vec![("".to_string(), "".to_string())];
            for entry in entries {
                let physical = entry.physical_id.as_deref().unwrap_or("-");
                // Logical ID is a group header so `[[`/`]]` anchor on each resource.
                rows.push((entry.logical_id.clone(), "".to_string()));
                rows.push(("    Status".to_string(), decorate_cfn_status(&entry.status)));
                rows.push(("    Type".to_string(), entry.resource_type.clone()));
                rows.push(("    Physical ID".to_string(), physical.to_string()));
                if let Some(reason) = &entry.status_reason {
                    rows.push(("    Reason".to_string(), reason.clone()));
                }
                rows.push(("".to_string(), "".to_string()));
            }
            rows
        }
    }
}

pub(super) fn cfn_outputs_lines(stack: &CfnStack) -> Vec<(String, String)> {
    if stack.outputs.is_empty() {
        return vec![
            ("".to_string(), "".to_string()),
            ("  No outputs defined".to_string(), "".to_string()),
            ("".to_string(), "".to_string()),
        ];
    }
    let mut rows = vec![("".to_string(), "".to_string())];
    for output in &stack.outputs {
        // Output key is a group header so `[[`/`]]` anchor on each output.
        rows.push((output.key.clone(), "".to_string()));
        rows.push(("    Value".to_string(), output.value.clone()));
        if let Some(export) = &output.export_name {
            rows.push(("    Export Name".to_string(), export.clone()));
        }
        if let Some(desc) = &output.description {
            rows.push(("    Description".to_string(), desc.clone()));
        }
        rows.push(("".to_string(), "".to_string()));
    }
    rows
}

pub(super) fn cfn_parameters_lines(stack: &CfnStack) -> Vec<(String, String)> {
    if stack.parameters.is_empty() {
        return vec![
            ("".to_string(), "".to_string()),
            ("  No parameters".to_string(), "".to_string()),
            ("".to_string(), "".to_string()),
        ];
    }
    let mut rows = vec![("".to_string(), "".to_string())];
    for param in &stack.parameters {
        rows.push((format!("  {}", param.key), param.value.clone()));
    }
    rows.push(("".to_string(), "".to_string()));
    rows
}

pub(super) fn cfn_tags_lines(stack: &CfnStack) -> Vec<(String, String)> {
    if stack.tags.is_empty() {
        return vec![
            ("".to_string(), "".to_string()),
            ("  No tags".to_string(), "".to_string()),
            ("".to_string(), "".to_string()),
        ];
    }
    let mut rows = vec![("".to_string(), "".to_string())];
    let mut sorted: Vec<_> = stack.tags.iter().collect();
    sorted.sort_by_key(|(k, _)| k.as_str());
    for (k, v) in sorted {
        rows.push((format!("  {}", k), v.clone()));
    }
    rows.push(("".to_string(), "".to_string()));
    rows
}

/// Prefix a CloudFormation status with a ✓/✗/⚠ glyph so `style_detail_row`
/// colours it (green/red/amber) — failures and in-progress events stand out
/// when scanning the event log for what went wrong.
pub(super) fn decorate_cfn_status(status: &str) -> String {
    if status == "DELETE_SKIPPED" {
        // DeletionPolicy: Retain (or a failed delete) — the resource outlived
        // its stack. This is the "what got left behind" signal.
        format!("⚠ {} (retained)", status)
    } else if status.contains("FAILED") || status == "ROLLBACK_COMPLETE" {
        format!("✗ {}", status)
    } else if status.contains("IN_PROGRESS") || status.contains("ROLLBACK") {
        format!("⚠ {}", status)
    } else if status.ends_with("COMPLETE") {
        format!("✓ {}", status)
    } else {
        status.to_string()
    }
}

pub(super) fn cfn_events_lines(events_state: Option<&Lazy<Vec<crate::aws::services::cloudformation::CfnStackEvent>>>) -> Vec<(String, String)> {
    match events_state {
        None | Some(Lazy::Loading) => {
            vec![
                ("".to_string(), "".to_string()),
                ("  Loading events...".to_string(), "".to_string()),
                ("".to_string(), "".to_string()),
            ]
        }
        Some(Lazy::Error(e)) => error_rows(e),
        Some(Lazy::Loaded(events)) => {
            if events.is_empty() {
                return vec![
                    ("".to_string(), "".to_string()),
                    ("  No events found".to_string(), "".to_string()),
                    ("".to_string(), "".to_string()),
                ];
            }
            let mut rows = vec![
                ("".to_string(), "".to_string()),
            ];
            for event in events {
                // Logical ID is a flush-left group header so `[[`/`]]` anchor on
                // each event; status/time/type/reason are its indented rows.
                rows.push((event.logical_id.clone(), "".to_string()));
                rows.push(("    Status".to_string(), decorate_cfn_status(&event.status)));
                rows.push(("    Time".to_string(), event.timestamp.clone()));
                rows.push(("    Type".to_string(), event.resource_type.clone()));
                if let Some(reason) = &event.status_reason {
                    rows.push(("    Reason".to_string(), reason.clone()));
                }
                rows.push(("".to_string(), "".to_string()));
            }
            if events.len() >= crate::aws::services::cloudformation::CFN_EVENTS_CAP {
                rows.push((
                    format!(
                        "  · showing the most recent {} events (older history truncated)",
                        crate::aws::services::cloudformation::CFN_EVENTS_CAP
                    ),
                    "".to_string(),
                ));
                rows.push(("".to_string(), "".to_string()));
            }
            rows
        }
    }
}

/// Compact duration for the progress rollup ("45s", "2m 40s", "1h 12m").
pub(super) fn fmt_secs_compact(secs: i64) -> String {
    let s = secs.max(0);
    if s < 60 {
        format!("{}s", s)
    } else if s < 3600 {
        format!("{}m {:02}s", s / 60, s % 60)
    } else {
        format!("{}h {:02}m", s / 3600, (s % 3600) / 60)
    }
}

/// The Events section's alternate body (`f`): a per-resource rollup of the
/// current operation answering "which resources are left". Derived entirely
/// from the already-fetched event stream, plus the template's resource set
/// for the Not-started group (create/delete operations only — an update
/// touches only changed resources, so "no events" ≠ "pending" there).
pub(super) fn cfn_progress_lines(
    stack: &CfnStack,
    events_state: Option<&Lazy<Vec<crate::aws::services::cloudformation::CfnStackEvent>>>,
    template_state: Option<&Lazy<String>>,
) -> Vec<(String, String)> {
    let events = match events_state {
        None | Some(Lazy::Loading) => {
            return vec![
                ("".to_string(), "".to_string()),
                ("  Loading events...".to_string(), "".to_string()),
                ("".to_string(), "".to_string()),
            ];
        }
        Some(Lazy::Error(e)) => return error_rows(e),
        Some(Lazy::Loaded(events)) => events,
    };
    if events.is_empty() {
        return vec![
            ("".to_string(), "".to_string()),
            ("  No events found".to_string(), "".to_string()),
            ("  · f returns to the event stream".to_string(), "".to_string()),
            ("".to_string(), "".to_string()),
        ];
    }
    let rollup =
        crate::aws::services::cloudformation::cfn_progress_rollup(&stack.stack_name, events);

    let mut in_prog: Vec<_> = Vec::new();
    let mut failed: Vec<_> = Vec::new();
    let mut done: Vec<_> = Vec::new();
    for r in &rollup.resources {
        if r.status.ends_with("_IN_PROGRESS") {
            in_prog.push(r);
        } else if r.status.ends_with("_FAILED") {
            failed.push(r);
        } else {
            // *_COMPLETE and DELETE_SKIPPED (terminal; decorated ⚠ retained).
            done.push(r);
        }
    }
    // Longest-running first — the stragglers are what you're waiting on.
    in_prog.sort_by_key(|r| r.ts_secs.unwrap_or(i64::MAX));

    let op = rollup.op_status.clone().unwrap_or_default();
    // "Not started" only means something while a create/delete is still
    // converging over a complete event window: an update touches only changed
    // resources, a settled operation has nothing left (whatever it never
    // touched was condition-skipped, not pending), and a truncated window
    // makes the event-seen set unreliable.
    let covers_all = (op.starts_with("CREATE") || op.starts_with("DELETE"))
        && rollup.in_progress()
        && !rollup.window_truncated;
    let mut not_started: Vec<crate::aws::services::cloudformation::CfnTemplateResource> =
        Vec::new();
    let mut template_parsed = false;
    let mut template_note: Option<&str> = None;
    if covers_all {
        match template_state {
            Some(Lazy::Loaded(body)) => {
                match crate::aws::services::cloudformation::parse_template_resource_ids(body) {
                    Some(ids) => {
                        template_parsed = true;
                        let seen: std::collections::HashSet<&str> = rollup
                            .resources
                            .iter()
                            .map(|r| r.logical_id.as_str())
                            .collect();
                        not_started = ids
                            .into_iter()
                            .filter(|r| !seen.contains(r.logical_id.as_str()))
                            .collect();
                    }
                    None => {
                        template_note =
                            Some("template unparseable — not-started resources can't be listed");
                    }
                }
            }
            Some(Lazy::Error(_)) => {
                template_note =
                    Some("template unavailable — not-started resources can't be listed");
            }
            None | Some(Lazy::Loading) => {
                template_note = Some("loading template for the full resource set…");
            }
        }
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let mut rows = vec![("".to_string(), "".to_string())];
    if !op.is_empty() {
        rows.push(("Operation".to_string(), decorate_cfn_status(&op)));
        if rollup.in_progress() {
            if let Some(start) = rollup.op_start_secs {
                rows.push((
                    "Started".to_string(),
                    format!("{} ago", fmt_secs_compact(now - start)),
                ));
            }
        }
    }
    let mut parts = vec![if template_parsed {
        format!(
            "{}/{} complete",
            done.len(),
            rollup.resources.len() + not_started.len()
        )
    } else {
        format!("{} complete", done.len())
    }];
    if !in_prog.is_empty() {
        parts.push(format!("{} in progress", in_prog.len()));
    }
    if !failed.is_empty() {
        parts.push(format!("{} failed", failed.len()));
    }
    if !not_started.is_empty() {
        let conditional = not_started.iter().filter(|r| r.conditional).count();
        if conditional > 0 {
            parts.push(format!(
                "{} not started ({} conditional)",
                not_started.len(),
                conditional
            ));
        } else {
            parts.push(format!("{} not started", not_started.len()));
        }
    }
    rows.push(("Progress".to_string(), parts.join(" · ")));
    rows.push((
        if rollup.in_progress() {
            "  · auto-refreshing every 10s — f returns to the event stream"
        } else {
            "  · f returns to the event stream"
        }
        .to_string(),
        "".to_string(),
    ));
    if op.starts_with("UPDATE") && !op.contains("ROLLBACK") {
        rows.push((
            "  · unchanged resources emit no events during an update".to_string(),
            "".to_string(),
        ));
    }
    if let Some(n) = template_note {
        rows.push((format!("  · {}", n), "".to_string()));
    }
    if rollup.window_truncated {
        rows.push((
            "  · event window truncated — the operation's start wasn't fetched".to_string(),
            "".to_string(),
        ));
    }
    rows.push(("".to_string(), "".to_string()));

    let id_w = rollup
        .resources
        .iter()
        .map(|r| r.logical_id.len())
        .chain(not_started.iter().map(|r| r.logical_id.len()))
        .max()
        .unwrap_or(0)
        .clamp(12, 40);

    if !in_prog.is_empty() {
        rows.push((format!("In progress ({})", in_prog.len()), "".to_string()));
        for r in &in_prog {
            let elapsed = r
                .ts_secs
                .map(|t| format!(" · {}", fmt_secs_compact(now - t)))
                .unwrap_or_default();
            rows.push((
                format!("    {:<id_w$}", r.logical_id),
                format!(
                    "{} · {}{}",
                    decorate_cfn_status(&r.status),
                    r.resource_type,
                    elapsed
                ),
            ));
        }
        rows.push(("".to_string(), "".to_string()));
    }
    if !not_started.is_empty() {
        rows.push((format!("Not started ({})", not_started.len()), "".to_string()));
        for r in &not_started {
            let mut value = "· not started".to_string();
            if !r.resource_type.is_empty() {
                value.push_str(&format!(" · {}", r.resource_type));
            }
            if r.conditional {
                value.push_str(" · conditional — skipped if its condition is false");
            }
            rows.push((format!("    {:<id_w$}", r.logical_id), value));
        }
        rows.push(("".to_string(), "".to_string()));
    }
    if !failed.is_empty() {
        rows.push((format!("Failed ({})", failed.len()), "".to_string()));
        for r in &failed {
            rows.push((
                format!("    {:<id_w$}", r.logical_id),
                format!("{} · {}", decorate_cfn_status(&r.status), r.resource_type),
            ));
            if let Some(reason) = &r.status_reason {
                rows.push((format!("      ✗ {}", reason), "".to_string()));
            }
        }
        rows.push(("".to_string(), "".to_string()));
    }
    if !done.is_empty() {
        rows.push((format!("Complete ({})", done.len()), "".to_string()));
        for r in &done {
            rows.push((
                format!("    {:<id_w$}", r.logical_id),
                format!("{} · {}", decorate_cfn_status(&r.status), r.resource_type),
            ));
        }
        rows.push(("".to_string(), "".to_string()));
    }
    rows
}

pub(super) fn cfn_template_lines(template_state: Option<&Lazy<String>>) -> Vec<(String, String)> {
    match template_state {
        None | Some(Lazy::Loading) => {
            vec![
                ("".to_string(), "".to_string()),
                ("  Loading template...".to_string(), "".to_string()),
                ("".to_string(), "".to_string()),
            ]
        }
        Some(Lazy::Error(e)) => error_rows(e),
        Some(Lazy::Loaded(body)) => {
            if body.is_empty() {
                return vec![
                    ("".to_string(), "".to_string()),
                    ("  Template not available".to_string(), "".to_string()),
                    ("".to_string(), "".to_string()),
                ];
            }
            // Pretty-print JSON templates; YAML is already readable.
            let display = if body.trim_start().starts_with('{') {
                serde_json::from_str::<serde_json::Value>(body)
                    .ok()
                    .and_then(|v| serde_json::to_string_pretty(&v).ok())
                    .unwrap_or_else(|| body.clone())
            } else {
                body.clone()
            };
            let mut rows = vec![("".to_string(), "".to_string())];
            for line in display.lines() {
                rows.push((format!("  {}", line), "".to_string()));
            }
            rows.push(("".to_string(), "".to_string()));
            rows
        }
    }
}

pub(super) fn cfn_drift_lines(
    stack: &CfnStack,
    drift_state: Option<&Lazy<Vec<crate::aws::services::cloudformation::CfnResourceDrift>>>,
) -> Vec<(String, String)> {
    let mut rows = vec![("".to_string(), "".to_string())];
    rows.push((
        "Stack Drift".to_string(),
        stack.drift_status.clone().unwrap_or_else(|| "NOT_CHECKED".to_string()),
    ));
    rows.push(("".to_string(), "".to_string()));
    match drift_state {
        None | Some(Lazy::Loading) => {
            rows.push(("  Loading resource drift…".to_string(), "".to_string()));
        }
        Some(Lazy::Error(e)) => rows.extend(error_rows(e)),
        Some(Lazy::Loaded(drifts)) => {
            // Only MODIFIED / DELETED entries are interesting; IN_SYNC is noise.
            let interesting: Vec<_> = drifts
                .iter()
                .filter(|d| d.drift_status != "IN_SYNC")
                .collect();
            if interesting.is_empty() {
                // Distinguish "clean" from "never checked": drift results only
                // exist after a DetectStackDrift run, which is a mutation this
                // app deliberately never calls.
                let never_checked = stack
                    .drift_status
                    .as_deref()
                    .is_none_or(|s| s == "NOT_CHECKED" || s == "UNKNOWN");
                if never_checked {
                    rows.push((
                        "  · drift never detected on this stack — run detection from the console (DetectStackDrift is a mutation, so neboto can't)".to_string(),
                        "".to_string(),
                    ));
                } else {
                    rows.push(("  No drifted resources".to_string(), "".to_string()));
                }
            } else {
                for d in interesting {
                    rows.push((format!("  {}", d.logical_id), d.drift_status.clone()));
                    rows.push(("    Type".to_string(), d.resource_type.clone()));
                    if let Some(pid) = &d.physical_id {
                        rows.push(("    Physical ID".to_string(), pid.clone()));
                    }
                    for diff in &d.differences {
                        rows.push((
                            format!("    {} [{}]", diff.path, diff.diff_type),
                            "".to_string(),
                        ));
                        rows.push(("      expected".to_string(), diff.expected.clone()));
                        rows.push(("      actual".to_string(), diff.actual.clone()));
                    }
                    rows.push(("".to_string(), "".to_string()));
                }
            }
        }
    }
    rows.push(("".to_string(), "".to_string()));
    rows
}

pub(super) fn cfn_changes_lines(changesets_state: Option<&Lazy<Vec<crate::aws::services::cloudformation::CfnChangeSet>>>) -> Vec<(String, String)> {
    let mut rows = vec![("".to_string(), "".to_string())];
    match changesets_state {
        None | Some(Lazy::Loading) => {
            rows.push(("  Loading change sets…".to_string(), "".to_string()));
        }
        Some(Lazy::Error(e)) => rows.extend(error_rows(e)),
        Some(Lazy::Loaded(sets)) => {
            if sets.is_empty() {
                rows.push((
                    "  No pending change sets".to_string(),
                    "".to_string(),
                ));
            } else {
                for cs in sets {
                    // Change-set name is a group header so `[[`/`]]` anchor on each.
                    rows.push((cs.name.clone(), "".to_string()));
                    rows.push(("    Status".to_string(), decorate_cfn_status(&cs.status)));
                    rows.push(("    Execution".to_string(), cs.execution_status.clone()));
                    rows.push(("    Created".to_string(), cs.creation_time.clone()));
                    if let Some(reason) = &cs.status_reason {
                        rows.push(("    Reason".to_string(), reason.clone()));
                    }
                    if let Some(desc) = &cs.description {
                        rows.push(("    Description".to_string(), desc.clone()));
                    }
                    if cs.changes.is_empty() {
                        rows.push(("    Changes".to_string(), "none".to_string()));
                    } else {
                        rows.push(("    Changes".to_string(), "".to_string()));
                        for ch in &cs.changes {
                            rows.extend(cfn_change_rows(ch));
                        }
                    }
                    // Deployment validations (CloudFormation Hooks): what will
                    // run before execution, and — once run — how it went.
                    if !cs.hooks.is_empty() {
                        rows.push((
                            format!("    Validations ({} hooks)", cs.hooks.len()),
                            "".to_string(),
                        ));
                        for h in &cs.hooks {
                            let mut v = Vec::new();
                            if let Some(p) = &h.invocation_point {
                                v.push(p.clone());
                            }
                            if let Some(m) = &h.failure_mode {
                                v.push(format!("{} mode", m));
                            }
                            if let Some(t) = &h.target {
                                v.push(t.clone());
                            }
                            rows.push((format!("      {}", h.type_name), v.join(" · ")));
                        }
                    }
                    if !cs.hook_results.is_empty() {
                        rows.push(("    Hook results".to_string(), "".to_string()));
                        for r in &cs.hook_results {
                            let mark = if r.status.contains("FAILED") {
                                "✗ "
                            } else if r.status.contains("SUCCEEDED") {
                                "✓ "
                            } else {
                                ""
                            };
                            let reason = r
                                .reason
                                .as_deref()
                                .map(|x| format!(" — {}", clip_inline(x, 120)))
                                .unwrap_or_default();
                            rows.push((
                                format!("      {}", r.type_name),
                                format!("{}{}{}", mark, r.status, reason),
                            ));
                        }
                    }
                    if cs.has_json_context() {
                        rows.push((
                            "      · e opens the full before/after JSON".to_string(),
                            "".to_string(),
                        ));
                    }
                    rows.push(("".to_string(), "".to_string()));
                }
            }
        }
    }
    rows.push(("".to_string(), "".to_string()));
    rows
}

/// Rows for one resource change: the action line, then a per-property diff
/// row per modification (before → after, dynamic-evaluation and
/// requires-recreation markers, and what caused it).
pub(super) fn cfn_change_rows(ch: &crate::aws::services::cloudformation::CfnChange) -> Vec<(String, String)> {
    let sym = match ch.action.as_str() {
        "Add" => "+",
        "Remove" => "−",
        "Modify" => "~",
        "Import" => "↷",
        _ => "?",
    };
    let mut markers = Vec::new();
    if let Some(r) = ch.replacement.as_deref().filter(|r| *r != "False") {
        // True or Conditional — the resource gets torn down and recreated.
        markers.push(format!("⚠ replacement: {}", r));
    }
    if let Some(p) = &ch.policy_action {
        markers.push(format!("on old resource: {}", p));
    }
    let marker_str = if markers.is_empty() {
        String::new()
    } else {
        format!("  {}", markers.join(" · "))
    };
    let mut rows = vec![(
        format!("      {} {} {}", sym, ch.action, ch.logical_id),
        format!("{}{}", ch.resource_type, marker_str),
    )];
    // The live resource being changed — the exact "Physical ID" label makes
    // the row Enter-jumpable (cfn_resource_jump_target keys on it).
    if let Some(pid) = &ch.physical_id {
        rows.push(("          Physical ID".to_string(), pid.clone()));
    }
    for d in &ch.details {
        let mut value = match (d.before.as_deref(), d.after.as_deref()) {
            (Some(b), Some(a)) => format!("{} → {}", clip_inline(b, 40), clip_inline(a, 40)),
            (Some(b), None) => format!("{} → (removed)", clip_inline(b, 40)),
            (None, Some(a)) => format!("(added) → {}", clip_inline(a, 40)),
            (None, None) => String::new(),
        };
        if d.evaluation.as_deref() == Some("Dynamic") {
            // The new value depends on a Ref/GetAtt only known at execution.
            if value.is_empty() {
                value = "resolved at execution".to_string();
            } else {
                value.push_str(" · resolved at execution");
            }
        }
        if let Some(rr) = d.requires_recreation.as_deref().filter(|r| *r != "Never") {
            value = format!("⚠ recreation: {} · {}", rr, value);
        }
        if let Some(src) = &d.change_source {
            let entity = d
                .causing_entity
                .as_deref()
                .map(|e| format!(" {}", e))
                .unwrap_or_default();
            value.push_str(&format!(" · via {}{}", src, entity));
        }
        rows.push((format!("          {}", d.label), value));
    }
    rows
}

// ── CFN Export Split Pane ─────────────────────────────────────────────────────

pub(super) fn render_cfn_export_split(app: &App, export: &CfnExport, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 2, "");

    let mut block = theme::pane_block("CloudFormation Export", focused);
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
                export.name.clone(),
                Style::default().fg(crate::ui::theme::text_primary()).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("Export", Style::default().fg(theme::aws_orange())),
            Span::styled("  ·  from ", Style::default().fg(theme::text_dim())),
            Span::styled(export.exporting_stack_name(), Style::default().fg(theme::text_dim())),
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
    render_cfn_export_section_tabs(app, chunks[2], frame);
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub(super) fn render_cfn_export_section_tabs(app: &App, area: Rect, frame: &mut Frame) {
    render_section_tab_bar(
        app,
        area,
        frame,
        &descriptor_tabs(app, &crate::aws::services::cloudformation::CFN_EXPORT_SECTIONS),
    );
}

pub fn cfn_export_section_lines(
    export: &CfnExport,
    section: CfnExportDetailSection,
    imports: Option<&Lazy<Vec<String>>>,
) -> Vec<(String, String)> {
    match section {
        CfnExportDetailSection::Details => {
            vec![
                ("Name".to_string(), export.name.clone()),
                ("".to_string(), "".to_string()),
                ("Value".to_string(), "".to_string()),
                // Plain content line so a long/ARN value wraps and (if an ARN) is
                // Enter-jumpable via the generic classifier.
                (format!(" {}", export.value), export.value.clone()),
                ("".to_string(), "".to_string()),
                // Carries the stack ARN as value → `Enter` jumps to the stack.
                ("Exporting stack".to_string(), export.exporting_stack_id.clone()),
            ]
        }
        CfnExportDetailSection::Imports => match imports {
            None | Some(Lazy::Loading) => {
                vec![("".to_string(), "Loading importing stacks…".to_string())]
            }
            Some(Lazy::Error(e)) => error_rows(e),
            Some(Lazy::Loaded(stacks)) if stacks.is_empty() => {
                vec![(
                    "".to_string(),
                    "Not imported by any stack (Fn::ImportValue).".to_string(),
                )]
            }
            Some(Lazy::Loaded(stacks)) => {
                let mut rows =
                    vec![(format!("Imported by ({})", stacks.len()), "".to_string()), ("".to_string(), "".to_string())];
                for s in stacks {
                    rows.push((format!("  {}", s), "".to_string()));
                }
                rows
            }
        },
    }
}

// ── CFN StackSet Split Pane ───────────────────────────────────────────────────

pub(super) fn render_cfn_stackset_split(app: &App, ss: &CfnStackSet, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 4, "");

    let mut block = theme::pane_block("CFN StackSet", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut header_lines: Vec<Line<'static>> = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                ss.stack_set_name.clone(),
                Style::default().fg(crate::ui::theme::text_primary()).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::raw(""),
    ];
    header_lines.push(header_kv("Status", &ss.status));
    if let Some(model) = &ss.permission_model {
        header_lines.push(header_kv("Permission Model", model));
    }
    header_lines.push(Line::raw(""));
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
    render_cfn_stackset_section_tabs(app, chunks[2], frame);
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub(super) fn render_cfn_stackset_section_tabs(app: &App, area: Rect, frame: &mut Frame) {
    render_section_tab_bar(
        app,
        area,
        frame,
        &descriptor_tabs(app, &crate::aws::services::cloudformation::CFN_STACKSET_SECTIONS),
    );
}

pub fn cfn_stackset_section_lines(
    ss: &CfnStackSet,
    section: CfnStackSetDetailSection,
    detail_state: Option<&Lazy<crate::aws::services::cloudformation::CfnStackSetDetailData>>,
    instances_state: Option<&Lazy<Vec<crate::aws::services::cloudformation::CfnStackInstance>>>,
    operations_state: Option<&Lazy<Vec<crate::aws::services::cloudformation::CfnStackSetOperation>>>,
    op_results: &crate::lazy::LazyMap<Vec<crate::aws::services::cloudformation::CfnStackSetOpResult>>,
) -> Vec<(String, String)> {
    match section {
        CfnStackSetDetailSection::Config => cfn_stackset_config_lines(ss, detail_state),
        CfnStackSetDetailSection::Instances => cfn_stackset_instances_lines(instances_state),
        CfnStackSetDetailSection::Operations => {
            cfn_stackset_operations_lines(&ss.stack_set_name, operations_state, op_results)
        }
        CfnStackSetDetailSection::Tags => cfn_stackset_tags_lines(detail_state),
    }
}

pub(super) fn cfn_stackset_config_lines(
    ss: &CfnStackSet,
    detail_state: Option<&Lazy<crate::aws::services::cloudformation::CfnStackSetDetailData>>,
) -> Vec<(String, String)> {
    let mut rows = vec![("".to_string(), "".to_string())];
    rows.push(("Stack Set Name".to_string(), ss.stack_set_name.clone()));
    rows.push(("Stack Set ID".to_string(), ss.stack_set_id.clone()));
    rows.push(("Status".to_string(), ss.status.clone()));
    match detail_state {
        None | Some(Lazy::Loading) => {
            rows.push(("".to_string(), "".to_string()));
            rows.push(("  Loading configuration…".to_string(), "".to_string()));
        }
        Some(Lazy::Error(e)) => rows.extend(error_rows(e)),
        Some(Lazy::Loaded(d)) => {
            if let Some(desc) = &d.description {
                rows.push(("Description".to_string(), desc.clone()));
            }
            if let Some(pm) = &d.permission_model {
                rows.push(("Permission Model".to_string(), pm.clone()));
            }
            if let Some(drift) = &d.drift_status {
                rows.push(("Drift Status".to_string(), drift.clone()));
            }

            rows.push(("".to_string(), "".to_string()));
            rows.push(("Deployment".to_string(), "".to_string()));
            rows.push((
                "Auto-Deployment".to_string(),
                match d.auto_deployment_enabled {
                    Some(true) => "Enabled".to_string(),
                    Some(false) => "Disabled".to_string(),
                    None => "—".to_string(),
                },
            ));
            if let Some(retain) = d.retain_on_account_removal {
                rows.push((
                    "Retain on Account Removal".to_string(),
                    if retain { "Yes" } else { "No" }.to_string(),
                ));
            }
            if let Some(me) = d.managed_execution {
                rows.push((
                    "Managed Execution".to_string(),
                    if me { "Active" } else { "Inactive" }.to_string(),
                ));
            }
            if !d.capabilities.is_empty() {
                rows.push(("Capabilities".to_string(), d.capabilities.join(", ")));
            }

            if d.administration_role_arn.is_some()
                || d.execution_role_name.is_some()
                || !d.organizational_unit_ids.is_empty()
            {
                rows.push(("".to_string(), "".to_string()));
                rows.push(("Permissions".to_string(), "".to_string()));
                if let Some(arn) = &d.administration_role_arn {
                    rows.push(("Admin Role".to_string(), arn.clone()));
                }
                if let Some(name) = &d.execution_role_name {
                    rows.push(("Execution Role".to_string(), name.clone()));
                }
                if let Some((first, rest)) = d.organizational_unit_ids.split_first() {
                    rows.push(("Target OUs".to_string(), first.clone()));
                    for ou in rest {
                        rows.push((format!("  {}", ou), String::new()));
                    }
                }
            }

            if !d.parameters.is_empty() {
                rows.push(("".to_string(), "".to_string()));
                rows.push(("Parameters".to_string(), "".to_string()));
                for (k, v) in &d.parameters {
                    rows.push((format!("  {}", k), v.clone()));
                }
            }
        }
    }
    rows.push(("".to_string(), "".to_string()));
    rows
}

pub(super) fn cfn_stackset_instances_lines(
    instances_state: Option<&Lazy<Vec<crate::aws::services::cloudformation::CfnStackInstance>>>,
) -> Vec<(String, String)> {
    let mut rows = vec![("".to_string(), "".to_string())];
    match instances_state {
        None | Some(Lazy::Loading) => {
            rows.push(("  Loading instances…".to_string(), "".to_string()));
        }
        Some(Lazy::Error(e)) => rows.extend(error_rows(e)),
        Some(Lazy::Loaded(instances)) => {
            if instances.is_empty() {
                rows.push(("  No stack instances".to_string(), "".to_string()));
            } else {
                rows.push((
                    format!("  {} instance(s)", instances.len()),
                    "".to_string(),
                ));
                rows.push(("".to_string(), "".to_string()));
                for inst in instances {
                    rows.push((
                        format!("  {} / {}", inst.account, inst.region),
                        inst.status.clone(),
                    ));
                    if let Some(drift) = &inst.drift_status {
                        rows.push(("    Drift".to_string(), drift.clone()));
                    }
                    if let Some(reason) = &inst.status_reason {
                        rows.push(("    Reason".to_string(), reason.clone()));
                    }
                    if let Some(sid) = &inst.stack_id {
                        rows.push(("    Stack".to_string(), sid.clone()));
                    }
                    rows.push(("".to_string(), "".to_string()));
                }
            }
        }
    }
    rows.push(("".to_string(), "".to_string()));
    rows
}

pub(super) fn cfn_stackset_operations_lines(
    stack_set: &str,
    operations_state: Option<&Lazy<Vec<crate::aws::services::cloudformation::CfnStackSetOperation>>>,
    op_results: &crate::lazy::LazyMap<Vec<crate::aws::services::cloudformation::CfnStackSetOpResult>>,
) -> Vec<(String, String)> {
    let mut rows = vec![("".to_string(), "".to_string())];
    match operations_state {
        None | Some(Lazy::Loading) => {
            rows.push(("  Loading operations…".to_string(), "".to_string()));
        }
        Some(Lazy::Error(e)) => rows.extend(error_rows(e)),
        Some(Lazy::Loaded(ops)) => {
            if ops.is_empty() {
                rows.push(("  No operations".to_string(), "".to_string()));
            } else {
                for op in ops {
                    rows.push((format!("  {}", op.action), op.status.clone()));
                    rows.push(("    Started".to_string(), op.creation_time.clone()));
                    if let Some(end) = &op.end_time {
                        rows.push(("    Ended".to_string(), end.clone()));
                    }
                    if let Some(reason) = &op.status_reason {
                        rows.push(("    Reason".to_string(), reason.clone()));
                    }
                    rows.push(("    Operation ID".to_string(), op.operation_id.clone()));
                    let key = crate::aws::services::cloudformation::cfn_op_results_key(
                        stack_set,
                        &op.operation_id,
                    );
                    rows.extend(cfn_op_results_rows(op_results.get(&key)));
                    rows.push(("".to_string(), "".to_string()));
                }
            }
        }
    }
    rows.push(("".to_string(), "".to_string()));
    rows
}

/// The per-account/region outcome rows under one operation. Collapsed (no
/// entry) = a hint row; `⏎` on the operation toggles it. The `Results` key
/// is load-bearing — `cfn_op_results_row_target` keys on it.
pub(super) fn cfn_op_results_rows(
    state: Option<&Lazy<Vec<crate::aws::services::cloudformation::CfnStackSetOpResult>>>,
) -> Vec<(String, String)> {
    let mut rows = Vec::new();
    match state {
        None => rows.push((
            "    Results".to_string(),
            "· press ⏎ for per-account/region outcomes".to_string(),
        )),
        Some(Lazy::Loading) => rows.push(("    Results".to_string(), "Loading…".to_string())),
        Some(Lazy::Error(e)) => {
            rows.push(("    Results".to_string(), format!("⚠ {}", e)));
        }
        Some(Lazy::Loaded(results)) => {
            let failed = results
                .iter()
                .filter(|r| r.status == "FAILED" || r.status == "CANCELLED")
                .count();
            let capped = results.len() >= crate::aws::services::cloudformation::CFN_OP_RESULTS_CAP;
            rows.push((
                "    Results".to_string(),
                format!(
                    "{}{} account/region{} · {} failed",
                    results.len(),
                    if capped { "+" } else { "" },
                    if results.len() == 1 { "" } else { "s" },
                    failed
                ),
            ));
            if results.is_empty() {
                rows.push(("      No per-account results".to_string(), String::new()));
            }
            for r in results {
                let status = match r.status.as_str() {
                    "SUCCEEDED" => format!("✓ {}", r.status),
                    "FAILED" | "CANCELLED" => format!("✗ {}", r.status),
                    "RUNNING" | "PENDING" => format!("⚠ {}", r.status),
                    _ => r.status.clone(),
                };
                rows.push((format!("      {} {}", r.account, r.region), status));
                if let Some(reason) = &r.status_reason {
                    rows.push(("        Reason".to_string(), reason.clone()));
                }
                if let Some((gate, why)) = &r.account_gate {
                    if gate != "SUCCEEDED" {
                        let v = match why {
                            Some(w) => format!("{} — {}", gate, w),
                            None => gate.clone(),
                        };
                        rows.push(("        Account gate".to_string(), v));
                    }
                }
                if let Some(ou) = &r.ou_id {
                    rows.push(("        OU".to_string(), ou.clone()));
                }
            }
            if capped {
                rows.push((
                    format!(
                        "      · showing the first {} (failures first)",
                        crate::aws::services::cloudformation::CFN_OP_RESULTS_CAP
                    ),
                    String::new(),
                ));
            }
        }
    }
    rows
}

/// Which stack-set operation a row in the Operations section belongs to,
/// for the `⏎` results toggle. Matches the operation's action header
/// (`("  CREATE", status)`), its `Operation ID` row, and its `Results` row;
/// anything else (a result row, a timestamp) is `None` so `⏎` falls through
/// to the normal jump.
pub fn cfn_op_results_row_target(rows: &[(String, String)], idx: usize) -> Option<String> {
    let (key, value) = rows.get(idx)?;
    let is_header = |k: &str, v: &str| {
        k.starts_with("  ") && !k[2..].starts_with(' ') && !k.trim().is_empty() && !v.is_empty()
    };
    let trimmed = key.trim();
    let op_id_at = |i: usize| -> Option<String> {
        let (k, v) = rows.get(i)?;
        (k.trim() == "Operation ID" && k.starts_with("    ")).then(|| v.clone())
    };
    if trimmed == "Operation ID" {
        return op_id_at(idx);
    }
    if trimmed == "Results" && key.starts_with("    ") && !key.starts_with("     ") {
        // The Results row sits directly under the Operation ID row.
        return idx.checked_sub(1).and_then(op_id_at);
    }
    if is_header(key, value) {
        for (k, v) in &rows[idx + 1..] {
            if k.trim() == "Operation ID" && k.starts_with("    ") {
                return Some(v.clone());
            }
            if (k.is_empty() && v.is_empty()) || is_header(k, v) {
                return None;
            }
        }
    }
    None
}

pub(super) fn cfn_stackset_tags_lines(
    detail_state: Option<&Lazy<crate::aws::services::cloudformation::CfnStackSetDetailData>>,
) -> Vec<(String, String)> {
    let mut rows = vec![("".to_string(), "".to_string())];
    match detail_state {
        None | Some(Lazy::Loading) => {
            rows.push(("  Loading tags…".to_string(), "".to_string()));
        }
        Some(Lazy::Error(e)) => rows.extend(error_rows(e)),
        Some(Lazy::Loaded(d)) => {
            if d.tags.is_empty() {
                rows.push(("  No tags".to_string(), "".to_string()));
            } else {
                let mut sorted = d.tags.clone();
                sorted.sort_by(|a, b| a.0.cmp(&b.0));
                for (k, v) in sorted {
                    rows.push((format!("  {}", k), v));
                }
            }
        }
    }
    rows.push(("".to_string(), "".to_string()));
    rows
}

#[cfg(test)]
mod cfn_op_results_tests {
    use super::*;

    fn r(k: &str, v: &str) -> (String, String) {
        (k.to_string(), v.to_string())
    }

    #[test]
    fn row_target_resolves_header_id_and_results_rows() {
        let rows = vec![
            r("", ""),
            r("  CREATE", "FAILED"),
            r("    Started", "2026-10-01"),
            r("    Operation ID", "op-1"),
            r("    Results", "· press ⏎ for per-account/region outcomes"),
            r("", ""),
            r("  UPDATE", "SUCCEEDED"),
            r("    Started", "2026-10-02"),
            r("    Operation ID", "op-2"),
            r("    Results", "2 account/regions · 1 failed"),
            r("      111111111111 us-east-1", "✗ FAILED"),
            r("        Reason", "boom"),
            r("", ""),
        ];
        let t = |i| cfn_op_results_row_target(&rows, i);
        assert_eq!(t(1).as_deref(), Some("op-1"));
        assert_eq!(t(3).as_deref(), Some("op-1"));
        assert_eq!(t(4).as_deref(), Some("op-1"));
        assert_eq!(t(6).as_deref(), Some("op-2"));
        assert_eq!(t(9).as_deref(), Some("op-2"));
        // Timestamp, result and spacer rows don't toggle.
        assert_eq!(t(2), None);
        assert_eq!(t(10), None);
        assert_eq!(t(11), None);
        assert_eq!(t(0), None);
    }

    #[test]
    fn policy_lines_cover_all_states() {
        let none = cfn_policy_lines(Some(&Lazy::Loaded(None)));
        assert!(none.iter().any(|(k, _)| k.contains("No stack policy")));
        let body = r#"{"Statement":[{"Effect":"Deny","Action":"Update:*","Principal":"*","Resource":"LogicalResourceId/Db"}]}"#;
        let some = cfn_policy_lines(Some(&Lazy::Loaded(Some(body.to_string()))));
        assert!(some.iter().any(|(k, v)| k == "  Resource" && v == "LogicalResourceId/Db"));
        assert!(some.iter().any(|(k, v)| k == "  Statements" && v == "1 (1 deny)"));
        let err = cfn_policy_lines(Some(&Lazy::Error("denied".into())));
        assert!(err.iter().any(|(k, _)| k.contains("⚠ denied")));
    }
}
