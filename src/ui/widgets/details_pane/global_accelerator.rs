use super::*;

// ── Global Accelerator split pane ──────────────────────────────────────────────

pub(super) fn render_ga_accelerator_split(app: &App, acc: &GaAccelerator, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 4, "");
    let mut block = theme::pane_block("Global Accelerator", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let state_color = if !acc.enabled {
        theme::text_dim()
    } else {
        match acc.status.as_str() {
            "DEPLOYED" => theme::success(),
            "IN_PROGRESS" => theme::warning(),
            _ => theme::text_dim(),
        }
    };
    let status_label = if acc.enabled {
        acc.status.clone()
    } else {
        format!("{} (disabled)", acc.status)
    };
    let header = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                acc.name.clone(),
                Style::default()
                    .fg(crate::ui::theme::text_primary())
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(acc.dns_name.clone(), Style::default().fg(theme::text_dim())),
            Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
            Span::styled(status_label, Style::default().fg(state_color)),
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
        &descriptor_tabs(app, &crate::aws::services::global_accelerator::GA_ACCELERATOR_SECTIONS),
    );
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub fn ga_accelerator_section_lines(
    acc: &GaAccelerator,
    section: GaAcceleratorDetailSection,
    listeners: Option<&Lazy<Vec<crate::aws::services::global_accelerator::GaListener>>>,
    groups: Option<&Lazy<Vec<crate::aws::services::global_accelerator::GaEndpointGroup>>>,
    tags: Option<&Lazy<std::collections::HashMap<String, String>>>,
) -> Vec<(String, String)> {
    match section {
        GaAcceleratorDetailSection::Details => {
            let mut rows = vec![
                ("Name".to_string(), acc.name.clone()),
                ("Status".to_string(), acc.status.clone()),
                (
                    "Enabled".to_string(),
                    if acc.enabled { "✓ yes" } else { "✗ no" }.to_string(),
                ),
                ("IP Type".to_string(), acc.ip_address_type.clone()),
                ("DNS Name".to_string(), acc.dns_name.clone()),
                (String::new(), String::new()),
                ("Static IPs".to_string(), String::new()), // group header
            ];
            if acc.static_ips.is_empty() {
                rows.push(("  (none)".to_string(), String::new()));
            } else {
                for ip in &acc.static_ips {
                    rows.push((format!("  {}", ip), String::new()));
                }
            }
            rows.push((String::new(), String::new()));
            if let Some(c) = &acc.created {
                rows.push(("Created".to_string(), c.clone()));
            }
            if let Some(m) = &acc.last_modified {
                rows.push(("Last Modified".to_string(), m.clone()));
            }
            rows.push(("ARN".to_string(), acc.arn.clone()));
            rows
        }
        GaAcceleratorDetailSection::Listeners => match listeners {
            None | Some(Lazy::Loading) => {
                vec![("".to_string(), "Loading listeners…".to_string())]
            }
            Some(Lazy::Error(e)) => error_rows(e),
            Some(Lazy::Loaded(ls)) => {
                let mut rows = vec![(format!("Listeners ({})", ls.len()), String::new())];
                rows.push((String::new(), String::new()));
                if ls.is_empty() {
                    rows.push(("  No listeners".to_string(), String::new()));
                } else {
                    rows.push((
                        format!("  {:<8}  {:<18}  {}", "Proto", "Ports", "Client Affinity"),
                        String::new(),
                    ));
                    for l in ls {
                        rows.push((
                            format!(
                                "  {:<8}  {:<18}  {}",
                                l.protocol, l.port_ranges, l.client_affinity
                            ),
                            String::new(),
                        ));
                    }
                }
                rows
            }
        },
        GaAcceleratorDetailSection::EndpointGroups => match groups {
            None | Some(Lazy::Loading) => {
                vec![("".to_string(), "Loading endpoint groups…".to_string())]
            }
            Some(Lazy::Error(e)) => error_rows(e),
            Some(Lazy::Loaded(gs)) => {
                if gs.is_empty() {
                    return vec![("".to_string(), "No endpoint groups".to_string())];
                }
                let mut rows = Vec::new();
                for g in gs {
                    // One subsection per group (region + protocol + traffic dial).
                    rows.push((
                        format!("{} · {} · dial {:.0}%", g.region, g.listener_protocol, g.traffic_dial),
                        String::new(),
                    ));
                    rows.push(("Health Check".to_string(), g.health_check.clone()));
                    if g.endpoints.is_empty() {
                        rows.push(("  No endpoints".to_string(), String::new()));
                    } else {
                        for e in &g.endpoints {
                            let health = if e.health_reason.is_empty() {
                                e.health_state.clone()
                            } else {
                                format!("{} ({})", e.health_state, e.health_reason)
                            };
                            rows.push((
                                format!("  {}  weight={}  {}", e.id, e.weight, health),
                                String::new(),
                            ));
                        }
                    }
                    rows.push((String::new(), String::new()));
                }
                rows
            }
        },
        GaAcceleratorDetailSection::Tags => match tags {
            None | Some(Lazy::Loading) => {
                vec![("".to_string(), "Loading tags…".to_string())]
            }
            Some(Lazy::Error(e)) => error_rows(e),
            Some(Lazy::Loaded(t)) => tag_rows(t),
        },
    }
}
