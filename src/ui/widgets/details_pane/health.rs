use super::*;

// ── Health Event split pane ────────────────────────────────────────────────────

pub fn health_section_lines(
    evt: &crate::aws::services::health::HealthEvent,
    section: HealthEventDetailSection,
    details_state: Option<&crate::lazy::Lazy<crate::aws::services::health::HealthEventDetails>>,
) -> Vec<(String, String)> {
    match section {
        HealthEventDetailSection::Details => vec![
            ("Service".to_string(), evt.service.clone()),
            ("Event Type".to_string(), evt.event_type_code.clone()),
            ("Category".to_string(), evt.category.clone()),
            ("Status".to_string(), evt.status_code.clone()),
            ("Scope".to_string(), evt.scope.clone()),
            ("Region".to_string(), evt.region.clone()),
            ("Availability Zone".to_string(), evt.availability_zone.clone()),
            ("".to_string(), "".to_string()),
            ("Start Time".to_string(), evt.start_time.clone()),
            ("End Time".to_string(), evt.end_time.clone()),
            ("Last Updated".to_string(), evt.last_updated_time.clone()),
            ("".to_string(), "".to_string()),
            ("ARN".to_string(), evt.arn.clone()),
        ],
        HealthEventDetailSection::Description => match details_state {
            None | Some(crate::lazy::Lazy::Loading) => {
                vec![("".to_string(), "Loading description…".to_string())]
            }
            Some(crate::lazy::Lazy::Error(e)) => {
                error_rows(e)
            }
            Some(crate::lazy::Lazy::Loaded(d)) => {
                if d.description.is_empty() {
                    vec![("".to_string(), "No description available".to_string())]
                } else {
                    d.description
                        .lines()
                        .map(|line| (format!("  {}", line), "".to_string()))
                        .collect()
                }
            }
        },
        HealthEventDetailSection::AffectedEntities => match details_state {
            None | Some(crate::lazy::Lazy::Loading) => {
                vec![("".to_string(), "Loading affected entities…".to_string())]
            }
            Some(crate::lazy::Lazy::Error(e)) => {
                error_rows(e)
            }
            Some(crate::lazy::Lazy::Loaded(d)) => {
                if d.affected_entities.is_empty() {
                    return vec![("".to_string(), "No affected entities".to_string())];
                }
                let mut rows = Vec::new();
                for (i, entity) in d.affected_entities.iter().enumerate() {
                    if i > 0 {
                        rows.push(("".to_string(), "".to_string()));
                    }
                    rows.push(("Entity".to_string(), entity.entity_value.clone()));
                    if !entity.status_code.is_empty() {
                        rows.push(("  Status".to_string(), entity.status_code.clone()));
                    }
                    if !entity.last_updated_time.is_empty() {
                        rows.push(("  Last Updated".to_string(), entity.last_updated_time.clone()));
                    }
                }
                rows
            }
        },
    }
}

pub(super) fn render_health_split(
    app: &App,
    evt: &crate::aws::services::health::HealthEvent,
    area: ratatui::layout::Rect,
    frame: &mut Frame,
) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 3, "");
    let mut block = theme::pane_block("Health Event", focused);
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
                evt.display_name(),
                Style::default()
                    .fg(crate::ui::theme::text_primary())
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                format!("{} · {} · {}", evt.category, evt.status_code, evt.region),
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

    let sections = descriptor_tabs(app, &crate::aws::services::health::HEALTH_EVENT_SECTIONS);
    render_section_tab_bar(app, chunks[2], frame, &sections);
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}
