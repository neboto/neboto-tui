use super::*;

// ── Resource Group split pane ──────────────────────────────────────────────────

pub(super) fn render_resource_group_split(app: &App, rg: &RgGroup, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 4, "");

    let mut block = theme::pane_block("Resource Group", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let header_lines = build_resource_group_header_lines(rg);
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
    render_resource_group_section_tabs(app, chunks[2], frame);
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub(super) fn build_resource_group_header_lines(rg: &RgGroup) -> Vec<Line<'static>> {
    let mut lines = vec![Line::raw("")];
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            rg.name.clone(),
            Style::default()
                .fg(crate::ui::theme::text_primary())
                .add_modifier(Modifier::BOLD),
        ),
    ]));
    if !rg.description.is_empty() {
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(rg.description.clone(), Style::default().fg(theme::text_dim())),
        ]));
    }
    lines.push(Line::raw(""));
    lines
}

pub(super) fn render_resource_group_section_tabs(app: &App, area: Rect, frame: &mut Frame) {
    render_section_tab_bar(
        app,
        area,
        frame,
        &descriptor_tabs(app, &crate::aws::services::resource_groups::RESOURCE_GROUP_SECTIONS),
    );
}

pub fn resource_group_section_lines(
    rg: &RgGroup,
    section: ResourceGroupDetailSection,
    query_state: Option<&Lazy<crate::aws::services::resource_groups::ResourceGroupQuery>>,
    resources_state: Option<&Lazy<Vec<crate::aws::services::resource_groups::ResourceGroupMember>>>,
    tags_state: Option<&Lazy<std::collections::HashMap<String, String>>>,
) -> Vec<(String, String)> {
    match section {
        ResourceGroupDetailSection::Details => rg_details_lines(rg, query_state),
        ResourceGroupDetailSection::Query => rg_query_lines(query_state),
        ResourceGroupDetailSection::Resources => rg_resources_lines(resources_state),
        ResourceGroupDetailSection::Tags => rg_tags_lines(tags_state),
    }
}

pub(super) fn rg_details_lines(
    rg: &RgGroup,
    query_state: Option<&Lazy<crate::aws::services::resource_groups::ResourceGroupQuery>>,
) -> Vec<(String, String)> {
    let mut rows = vec![
        ("Name".to_string(), rg.name.clone()),
        ("ARN".to_string(), rg.arn.clone()),
    ];
    if !rg.description.is_empty() {
        rows.push(("Description".to_string(), rg.description.clone()));
    }
    rows.push(("".to_string(), "".to_string()));
    let qt = match query_state {
        Some(Lazy::Loaded(q)) => q.query_type.clone(),
        Some(Lazy::Loading) => "Loading…".to_string(),
        Some(Lazy::Error(e)) => format!("⚠ {}", e),
        None => "…".to_string(),
    };
    rows.push(("Query Type".to_string(), qt));
    rows
}

pub(super) fn rg_query_lines(query_state: Option<&Lazy<crate::aws::services::resource_groups::ResourceGroupQuery>>) -> Vec<(String, String)> {
    match query_state {
        None | Some(Lazy::Loading) => {
            vec![("".to_string(), "Loading query…".to_string())]
        }
        Some(Lazy::Error(e)) => {
            error_rows(e)
        }
        Some(Lazy::Loaded(q)) => {
            let mut rows = vec![("Type".to_string(), q.query_type.clone())];
            rows.push(("".to_string(), "".to_string()));
            // Pretty-print the query JSON if possible
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&q.query) {
                if let Ok(pretty) = serde_json::to_string_pretty(&parsed) {
                    for line in pretty.lines() {
                        rows.push(("".to_string(), line.to_string()));
                    }
                } else {
                    rows.push(("Query".to_string(), q.query.clone()));
                }
            } else {
                // Not JSON (e.g. CFN stack ARN) — show as-is
                rows.push(("Query".to_string(), q.query.clone()));
            }
            rows
        }
    }
}

pub(super) fn rg_resources_lines(
    state: Option<&Lazy<Vec<crate::aws::services::resource_groups::ResourceGroupMember>>>,
) -> Vec<(String, String)> {
    match state {
        None | Some(Lazy::Loading) => {
            vec![("".to_string(), "Loading resources…".to_string())]
        }
        Some(Lazy::Error(e)) => {
            error_rows(e)
        }
        Some(Lazy::Loaded(members)) => {
            if members.is_empty() {
                return vec![("".to_string(), "No resources".to_string())];
            }
            let mut rows: Vec<(String, String)> = Vec::with_capacity(members.len() + 1);
            rows.push(("Count".to_string(), members.len().to_string()));
            rows.push(("".to_string(), "".to_string()));
            for m in members.iter().take(200) {
                let label = if m.resource_type.is_empty() {
                    "→".to_string()
                } else {
                    format!("→ {}", m.resource_type)
                };
                rows.push((label, m.arn.clone()));
            }
            if members.len() > 200 {
                rows.push((
                    "".to_string(),
                    format!("… and {} more", members.len() - 200),
                ));
            }
            rows
        }
    }
}

pub(super) fn rg_tags_lines(state: Option<&Lazy<std::collections::HashMap<String, String>>>) -> Vec<(String, String)> {
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
            sorted
                .into_iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        }
    }
}
