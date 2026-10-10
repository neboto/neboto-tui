use super::*;

// ── RAM resource share split pane ──────────────────────────────────────────────

pub(super) fn render_ram_share_split(
    app: &App,
    share: &crate::aws::services::ram::RamResourceShare,
    area: Rect,
    frame: &mut Frame,
) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 4, "");

    let mut block = theme::pane_block("RAM Resource Share", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let header_lines = build_ram_share_header_lines(share);
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
    render_ram_section_tabs(app, chunks[2], frame);
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub(super) fn build_ram_share_header_lines(
    share: &crate::aws::services::ram::RamResourceShare,
) -> Vec<Line<'static>> {
    let mut lines = vec![Line::raw("")];

    let display_name = if share.name.is_empty() {
        share.arn.clone()
    } else {
        share.name.clone()
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

    let state_color = match share.status.as_str() {
        "ACTIVE" => theme::success(),
        "PENDING" => theme::warning(),
        "FAILED" => theme::error(),
        "DELETING" | "DELETED" => theme::text_dim(),
        _ => theme::text_dim(),
    };
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(share.status.clone(), Style::default().fg(state_color)),
        Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
        Span::styled(format!("owner: {}", share.owner), Style::default().fg(theme::text_dim())),
        Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
        Span::styled(
            share.owning_account_id.clone(),
            Style::default().fg(theme::text_dim()),
        ),
    ]));

    lines.push(Line::raw(""));
    lines
}

pub(super) fn render_ram_section_tabs(app: &App, area: Rect, frame: &mut Frame) {
    render_section_tab_bar(
        app,
        area,
        frame,
        &descriptor_tabs(app, &crate::aws::services::ram::RAM_SHARE_SECTIONS),
    );
}

pub fn ram_share_section_lines(
    share: &crate::aws::services::ram::RamResourceShare,
    section: RamResourceShareDetailSection,
    resources: Option<&Lazy<Vec<crate::aws::services::ram::RamSharedResource>>>,
    principals: Option<&Lazy<Vec<crate::aws::services::ram::RamPrincipal>>>,
) -> Vec<(String, String)> {
    match section {
        RamResourceShareDetailSection::Details => ram_details_lines(share),
        RamResourceShareDetailSection::Resources => ram_resources_lines(resources),
        RamResourceShareDetailSection::Principals => ram_principals_lines(principals),
        RamResourceShareDetailSection::Tags => ram_tags_lines(share),
    }
}

pub(super) fn ram_details_lines(
    share: &crate::aws::services::ram::RamResourceShare,
) -> Vec<(String, String)> {
    let mut rows = vec![
        ("Name".to_string(), share.name.clone()),
        ("ARN".to_string(), share.arn.clone()),
        ("Status".to_string(), share.status.clone()),
    ];

    // statusMessage in red (✗ prefix) when present — typically on FAILED shares.
    if !share.status_message.is_empty() {
        rows.push((
            "Status Message".to_string(),
            format!("✗ {}", share.status_message),
        ));
    }

    rows.push(("".to_string(), "".to_string()));
    rows.push((
        "Owning Account".to_string(),
        share.owning_account_id.clone(),
    ));
    rows.push(("Owner".to_string(), share.owner.clone()));
    rows.push((
        "External Principals".to_string(),
        if share.allow_external_principals {
            "✓ Allowed".to_string()
        } else {
            "✗ Not allowed".to_string()
        },
    ));
    rows.push(("Feature Set".to_string(), share.feature_set.clone()));
    rows.push(("".to_string(), "".to_string()));

    if let Some(created) = &share.created_time {
        rows.push(("Created".to_string(), created.clone()));
    }
    if let Some(updated) = &share.last_updated_time {
        rows.push(("Updated".to_string(), updated.clone()));
    }
    rows.push(("".to_string(), "".to_string()));
    rows
}

pub(super) fn ram_resources_lines(state: Option<&Lazy<Vec<crate::aws::services::ram::RamSharedResource>>>) -> Vec<(String, String)> {
    match state {
        None | Some(Lazy::Loading) => {
            vec![("".to_string(), "Loading shared resources…".to_string())]
        }
        Some(Lazy::Error(e)) => {
            if e.contains("nauthorized") || e.contains("AccessDenied") || e.contains("UnknownResource") {
                vec![
                    ("".to_string(), "Not available — resource details are managed by the sharing account.".to_string()),
                ]
            } else {
                error_rows(e)
            }
        }
        Some(Lazy::Loaded(resources)) => {
            if resources.is_empty() {
                return vec![("".to_string(), "No shared resources".to_string())];
            }
            let mut rows = vec![];
            for r in resources {
                // The ARN is the value → generic arn_jump_target lights up the
                // `→` and Enter jumps to the underlying resource.
                rows.push(("Resource".to_string(), r.arn.clone()));
                if !r.status.is_empty() {
                    rows.push(("Status".to_string(), r.status.clone()));
                }
                rows.push(("".to_string(), "".to_string()));
            }
            rows
        }
    }
}

pub(super) fn ram_principals_lines(state: Option<&Lazy<Vec<crate::aws::services::ram::RamPrincipal>>>) -> Vec<(String, String)> {
    match state {
        None | Some(Lazy::Loading) => {
            vec![("".to_string(), "Loading principals…".to_string())]
        }
        Some(Lazy::Error(e)) => {
            if e.contains("nauthorized") || e.contains("AccessDenied") || e.contains("UnknownResource") {
                vec![
                    ("".to_string(), "Not available — principal details are managed by the sharing account.".to_string()),
                ]
            } else {
                error_rows(e)
            }
        }
        Some(Lazy::Loaded(principals)) => {
            if principals.is_empty() {
                return vec![("".to_string(), "No principals".to_string())];
            }
            let mut rows = vec![];
            for p in principals {
                rows.push(("Principal".to_string(), p.principal.clone()));
                if !p.status.is_empty() {
                    rows.push(("Status".to_string(), p.status.clone()));
                }
                rows.push((
                    "External".to_string(),
                    if p.external { "✓ Yes" } else { "✗ No" }.to_string(),
                ));
                rows.push(("".to_string(), "".to_string()));
            }
            rows
        }
    }
}

pub(super) fn ram_tags_lines(share: &crate::aws::services::ram::RamResourceShare) -> Vec<(String, String)> {
    if share.tags.is_empty() {
        return vec![("".to_string(), "No tags".to_string())];
    }
    let mut sorted: Vec<(&String, &String)> = share.tags.iter().collect();
    sorted.sort_by_key(|(k, _)| k.as_str());
    let mut rows: Vec<(String, String)> = sorted
        .into_iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    rows.push(("".to_string(), "".to_string()));
    rows
}
