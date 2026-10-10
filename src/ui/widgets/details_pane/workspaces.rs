use super::*;

// ── WorkSpace split pane ───────────────────────────────────────────────────────

pub(super) fn render_workspace_split(
    app: &App,
    ws: &crate::aws::services::workspaces::Workspace,
    area: Rect,
    frame: &mut Frame,
) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 3, "");

    let mut block = theme::pane_block("WorkSpace", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let header_lines = build_workspace_header_lines(ws);
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
    render_workspace_section_tabs(app, chunks[2], frame);
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub(super) fn build_workspace_header_lines(
    ws: &crate::aws::services::workspaces::Workspace,
) -> Vec<Line<'static>> {
    let mut lines = vec![Line::raw("")];

    let display_name = if ws.user_name.is_empty() {
        ws.id.clone()
    } else {
        ws.user_name.clone()
    };
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            display_name,
            Style::default()
                .fg(crate::ui::theme::text_primary())
                .add_modifier(Modifier::BOLD),
        ),
    ]));

    let state_color = match ws.state.as_str() {
        "AVAILABLE" => theme::success(),
        "STOPPED" | "SUSPENDED" => theme::text_dim(),
        "ERROR" | "UNHEALTHY" | "IMPAIRED" => theme::error(),
        "TERMINATING" | "TERMINATED" => theme::text_dim(),
        _ => theme::warning(),
    };
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(ws.state.clone(), Style::default().fg(state_color)),
        Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
        Span::styled(ws.id.clone(), Style::default().fg(theme::text_dim())),
        Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
        Span::styled(
            ws.running_mode.clone(),
            Style::default().fg(theme::text_dim()),
        ),
    ]));

    lines.push(Line::raw(""));
    lines
}

pub(super) fn render_workspace_section_tabs(app: &App, area: Rect, frame: &mut Frame) {
    render_section_tab_bar(
        app,
        area,
        frame,
        &descriptor_tabs(app, &crate::aws::services::workspaces::WORKSPACE_SECTIONS),
    );
}

pub fn workspace_section_lines(
    ws: &crate::aws::services::workspaces::Workspace,
    section: WorkspaceDetailSection,
    connection: Option<&Lazy<crate::aws::services::workspaces::WorkspaceConnection>>,
    tags: Option<&Lazy<std::collections::HashMap<String, String>>>,
) -> Vec<(String, String)> {
    match section {
        WorkspaceDetailSection::Details => workspace_details_lines(ws),
        WorkspaceDetailSection::Connection => workspace_connection_lines(connection),
        WorkspaceDetailSection::Tags => workspace_tags_lines(tags),
    }
}

pub(super) fn workspace_details_lines(
    ws: &crate::aws::services::workspaces::Workspace,
) -> Vec<(String, String)> {
    let dash = || "—".to_string();
    let mut rows = vec![
        ("WorkSpace ID".to_string(), ws.id.clone()),
        ("User".to_string(), ws.user_name.clone()),
        ("".to_string(), "".to_string()),
        // Directory alias + the raw id row (subnet/dir ids are opaque). The id is
        // not yet jumpable (no Directory Service browser), so it's a plain value.
        ("Directory".to_string(), ws.directory_alias.clone()),
        ("Directory ID".to_string(), ws.directory_id.clone()),
        ("Bundle".to_string(), ws.bundle_name.clone()),
        ("Bundle ID".to_string(), ws.bundle_id.clone()),
        ("".to_string(), "".to_string()),
        ("State".to_string(), ws.state.clone()),
    ];

    // Running mode + auto-stop timeout (the operational signal).
    let mut running = ws.running_mode.clone();
    if ws.running_mode == "AUTO_STOP" {
        if let Some(t) = ws.running_mode_timeout_min {
            running = format!("{} (auto-stop after {} min)", running, t);
        }
    }
    rows.push(("Running Mode".to_string(), running));
    rows.push(("Compute Type".to_string(), ws.compute_type.clone()));
    rows.push(("".to_string(), "".to_string()));

    rows.push(("IP Address".to_string(), ws.ip_address.clone()));
    rows.push(("Computer Name".to_string(), ws.computer_name.clone()));
    // subnet- token in the value → resource_jump_target lights up the `→`.
    rows.push(("Subnet".to_string(), ws.subnet_id.clone()));
    rows.push(("".to_string(), "".to_string()));

    rows.push((
        "Root Volume".to_string(),
        ws.root_volume_gb
            .map(|v| format!("{} GiB", v))
            .unwrap_or_else(dash),
    ));
    rows.push((
        "User Volume".to_string(),
        ws.user_volume_gb
            .map(|v| format!("{} GiB", v))
            .unwrap_or_else(dash),
    ));
    rows.push((
        "Root Volume Encrypted".to_string(),
        if ws.root_volume_encryption {
            "✓ Yes".to_string()
        } else {
            "✗ No".to_string()
        },
    ));
    rows.push((
        "User Volume Encrypted".to_string(),
        if ws.user_volume_encryption {
            "✓ Yes".to_string()
        } else {
            "✗ No".to_string()
        },
    ));
    if let Some(key) = &ws.volume_encryption_key {
        // KMS arn/id in the value → arn_jump_target lights up the `→`.
        rows.push(("Encryption Key".to_string(), key.clone()));
    }

    if !ws.modification_states.is_empty() {
        rows.push(("".to_string(), "".to_string()));
        rows.push(("Modification States".to_string(), "".to_string()));
        for m in &ws.modification_states {
            rows.push((format!("  {}", m), "".to_string()));
        }
    }

    rows.push(("".to_string(), "".to_string()));
    rows
}

pub(super) fn workspace_connection_lines(
    state: Option<&Lazy<crate::aws::services::workspaces::WorkspaceConnection>>,
) -> Vec<(String, String)> {
    match state {
        None | Some(Lazy::Loading) => {
            vec![("".to_string(), "Loading connection status…".to_string())]
        }
        Some(Lazy::Error(e)) => {
            error_rows(e)
        }
        Some(Lazy::Loaded(c)) => {
            // CONNECTED renders green via the ✓ prefix; DISCONNECTED neutral.
            let cs = match c.connection_state.as_str() {
                "CONNECTED" => format!("✓ {}", c.connection_state),
                _ => c.connection_state.clone(),
            };
            let mut rows = vec![("Connection State".to_string(), cs)];
            rows.push((
                "Last Active".to_string(),
                c.last_active.clone().unwrap_or_else(|| "—".to_string()),
            ));
            rows.push((
                "Last User Connection".to_string(),
                c.last_known_user_connection
                    .clone()
                    .unwrap_or_else(|| "—".to_string()),
            ));
            rows.push(("".to_string(), "".to_string()));
            rows
        }
    }
}

pub(super) fn workspace_tags_lines(state: Option<&Lazy<std::collections::HashMap<String, String>>>) -> Vec<(String, String)> {
    match state {
        None | Some(Lazy::Loading) => {
            vec![("".to_string(), "Loading tags…".to_string())]
        }
        Some(Lazy::Error(e)) => {
            error_rows(e)
        }
        Some(Lazy::Loaded(tags)) => {
            if tags.is_empty() {
                return vec![("".to_string(), "No tags".to_string())];
            }
            let mut sorted: Vec<(&String, &String)> = tags.iter().collect();
            sorted.sort_by_key(|(k, _)| k.as_str());
            let mut rows: Vec<(String, String)> = sorted
                .into_iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            rows.push(("".to_string(), "".to_string()));
            rows
        }
    }
}
