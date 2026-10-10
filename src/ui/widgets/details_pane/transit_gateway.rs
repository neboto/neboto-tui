use super::*;

// ── Transit Gateway split panes ────────────────────────────────────────────────

pub(super) fn render_tgw_split(app: &App, tgw: &TransitGateway, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 4, "");
    let mut block = theme::pane_block("Transit Gateway", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let header = build_tgw_header_lines(tgw);
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
    render_section_tab_bar(app, chunks[2], frame, &descriptor_tabs(app, &crate::aws::services::transit_gateway::TGW_SECTIONS));
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub(super) fn build_tgw_header_lines(tgw: &TransitGateway) -> Vec<Line<'static>> {
    let name = tgw
        .tags
        .get("Name")
        .filter(|n| !n.is_empty())
        .cloned()
        .unwrap_or_else(|| tgw.id.clone());
    let state_color = match tgw.state.as_str() {
        "available" => theme::success(),
        "pending" | "modifying" => theme::warning(),
        "deleting" | "deleted" => theme::text_dim(),
        _ => theme::text_dim(),
    };
    vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                name,
                Style::default().fg(crate::ui::theme::text_primary()).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(tgw.id.clone(), Style::default().fg(theme::text_dim())),
            Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
            Span::styled(tgw.state.clone(), Style::default().fg(state_color)),
        ]),
        Line::raw(""),
    ]
}

pub(super) fn render_tgw_route_table_split(
    app: &App,
    rt: &TgwRouteTable,
    area: Rect,
    frame: &mut Frame,
) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 2, "");
    let mut block = theme::pane_block("TGW Route Table", focused);
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
                rt.id.clone(),
                Style::default().fg(crate::ui::theme::text_primary()).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(rt.tgw_id.clone(), Style::default().fg(theme::text_dim())),
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
    render_section_tab_bar(app, chunks[2], frame, &descriptor_tabs(app, &crate::aws::services::transit_gateway::TGW_ROUTE_TABLE_SECTIONS));
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

/// Body lines for a Transit Gateway. Attachments/Route Tables are filtered from
/// the already-loaded list (passed in by `get_detail_lines`), so no extra fetch.
pub fn tgw_section_lines(
    tgw: &TransitGateway,
    section: TgwDetailSection,
    attachments: &[(String, String, String, String)], // id, kind, resource_id, state
    route_tables: &[(String, String)],                 // id, state
) -> Vec<(String, String)> {
    match section {
        TgwDetailSection::Details => {
            let mut rows = vec![
                ("ID".to_string(), tgw.id.clone()),
                ("ARN".to_string(), tgw.arn.clone()),
                ("State".to_string(), tgw.state.clone()),
                ("Owner".to_string(), tgw.owner_id.clone()),
            ];
            if let Some(asn) = tgw.amazon_side_asn {
                rows.push(("Amazon Side ASN".to_string(), asn.to_string()));
            }
            if !tgw.description.is_empty() {
                rows.push(("Description".to_string(), tgw.description.clone()));
            }
            rows.push((
                "DNS Support".to_string(),
                if tgw.dns_support { "✓ enabled" } else { "disabled" }.to_string(),
            ));
            rows.push((
                "Default RT Association".to_string(),
                if tgw.default_route_table_association { "✓ enabled" } else { "disabled" }.to_string(),
            ));
            rows.push((
                "Default RT Propagation".to_string(),
                if tgw.default_route_table_propagation { "✓ enabled" } else { "disabled" }.to_string(),
            ));
            if let Some(rt) = &tgw.association_default_route_table_id {
                rows.push(("Assoc Default RT".to_string(), rt.clone()));
            }
            if let Some(rt) = &tgw.propagation_default_route_table_id {
                rows.push(("Prop Default RT".to_string(), rt.clone()));
            }
            if let Some(ct) = &tgw.creation_time {
                rows.push(("Created".to_string(), ct.clone()));
            }
            rows
        }
        TgwDetailSection::Attachments => {
            let mut rows = vec![(
                format!("Attachments ({})", attachments.len()),
                String::new(),
            )];
            rows.push((String::new(), String::new()));
            if attachments.is_empty() {
                rows.push(("  No attachments".to_string(), String::new()));
            } else {
                rows.push((
                    format!("  {:<24}  {:<14}  {:<22}  {}", "ID", "Type", "Resource", "State"),
                    String::new(),
                ));
                for (id, kind, resource_id, state) in attachments {
                    rows.push((
                        format!("  {:<24}  {:<14}  {:<22}  {}", id, kind, resource_id, state),
                        String::new(),
                    ));
                }
            }
            rows
        }
        TgwDetailSection::RouteTables => {
            let mut rows = vec![(
                format!("Route Tables ({})", route_tables.len()),
                String::new(),
            )];
            rows.push((String::new(), String::new()));
            if route_tables.is_empty() {
                rows.push(("  No route tables".to_string(), String::new()));
            } else {
                for (id, state) in route_tables {
                    rows.push((format!("  {:<24}  {}", id, state), String::new()));
                }
            }
            rows
        }
        TgwDetailSection::Tags => tag_rows(&tgw.tags),
    }
}

/// Body lines for a TGW route table. Routes are lazy (SearchTransitGatewayRoutes).
pub fn tgw_route_table_section_lines(
    rt: &TgwRouteTable,
    section: TgwRouteTableDetailSection,
    routes: Option<&crate::lazy::Lazy<Vec<crate::aws::services::transit_gateway::TgwRoute>>>,
) -> Vec<(String, String)> {
    match section {
        TgwRouteTableDetailSection::Routes => match routes {
            None | Some(crate::lazy::Lazy::Loading) => {
                vec![("".to_string(), "Loading routes…".to_string())]
            }
            Some(crate::lazy::Lazy::Error(e)) => error_rows(e),
            Some(crate::lazy::Lazy::Loaded(routes)) => {
                let mut rows = vec![(format!("Routes ({})", routes.len()), String::new())];
                rows.push((String::new(), String::new()));
                if routes.is_empty() {
                    rows.push(("  No active routes".to_string(), String::new()));
                } else {
                    rows.push((
                        format!("  {:<20}  {:<11}  {:<11}  {}", "CIDR", "State", "Type", "Target"),
                        String::new(),
                    ));
                    for r in routes {
                        let target = if r.resource_id.is_empty() {
                            r.attachment_id.clone()
                        } else {
                            format!("{} ({})", r.resource_id, r.resource_type)
                        };
                        rows.push((
                            format!(
                                "  {:<20}  {:<11}  {:<11}  {}",
                                r.cidr, r.state, r.route_type, target
                            ),
                            String::new(),
                        ));
                    }
                }
                rows
            }
        },
        TgwRouteTableDetailSection::Tags => tag_rows(&rt.tags),
    }
}

pub(super) fn render_tgw_attachment_split(
    app: &App,
    att: &crate::aws::services::transit_gateway::TgwAttachment,
    area: Rect,
    frame: &mut Frame,
) {
    render_simple_split(
        app,
        area,
        frame,
        "TGW Attachment",
        &att.id,
        &att.resource_kind,
        &descriptor_tabs(
            app,
            &crate::aws::services::transit_gateway::TGW_ATTACHMENT_SECTIONS,
        ),
    );
}

pub fn tgw_attachment_section_lines(
    att: &crate::aws::services::transit_gateway::TgwAttachment,
    section: TgwAttachmentDetailSection,
) -> Vec<(String, String)> {
    match section {
        TgwAttachmentDetailSection::Overview => {
            let mut rows = vec![
                (String::new(), String::new()),
                ("Attachment ID".to_string(), att.id.clone()),
                ("Transit Gateway".to_string(), att.tgw_id.clone()),
                ("Resource Type".to_string(), att.resource_kind.clone()),
                ("Resource ID".to_string(), att.resource_id.clone()),
                ("State".to_string(), att.state.clone()),
            ];
            if !att.resource_owner_id.is_empty() {
                rows.push(("Resource Owner".to_string(), att.resource_owner_id.clone()));
            }
            if let Some(c) = &att.creation_time {
                rows.push(("Created".to_string(), c.clone()));
            }
            rows
        }
        TgwAttachmentDetailSection::Association => {
            let mut rows = vec![(String::new(), String::new())];
            match &att.association_route_table_id {
                Some(rt) => {
                    rows.push(("Route Table".to_string(), rt.clone()));
                    if let Some(s) = &att.association_state {
                        rows.push(("Association State".to_string(), s.clone()));
                    }
                }
                None => rows.push((
                    "".to_string(),
                    "Not associated with a route table".to_string(),
                )),
            }
            rows
        }
        TgwAttachmentDetailSection::Tags => tag_rows(&att.tags),
    }
}
