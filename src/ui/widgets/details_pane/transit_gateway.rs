use super::*;
use crate::aws::services::transit_gateway::{TgwRoute, TgwRoutes, TGW_ROUTES_MAX};

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
    let header = one_row_header(app, header, area.width.saturating_sub(2) as usize);
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
    let footer = detail_footer(app, focused, 3, "");
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
    let header = one_row_header(app, header, area.width.saturating_sub(2) as usize);
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
        TgwDetailSection::Overview => {
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

            rows.push((String::new(), String::new()));
            rows.push(("Options".to_string(), String::new()));
            let on_off = |on: bool| if on { "✓ enabled" } else { "disabled" }.to_string();
            rows.push((
                "Auto-accept Shared".to_string(),
                if tgw.auto_accept_shared_attachments {
                    "⚠ enabled — accounts it's shared with attach without approval".to_string()
                } else {
                    "disabled — shared attachments wait for acceptance".to_string()
                },
            ));
            rows.push(("VPN ECMP".to_string(), on_off(tgw.vpn_ecmp_support)));
            rows.push(("Multicast".to_string(), on_off(tgw.multicast_support)));
            rows.push(("SG Referencing".to_string(), on_off(tgw.security_group_referencing_support)));
            rows.push((
                "CIDR Blocks".to_string(),
                if tgw.cidr_blocks.is_empty() { "—".to_string() } else { tgw.cidr_blocks.join(", ") },
            ));
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

/// Routes, and blackhole routes, the Overview lists before pointing at
/// the Routes tab that has them all.
const TGW_RT_OVERVIEW_ROUTES: usize = 8;
const TGW_RT_OVERVIEW_BLACKHOLES: usize = 3;

fn tgw_routes_table_header() -> (String, String) {
    (format!("  {:<20}  {:<11}  {:<11}  {}", "CIDR", "State", "Type", "Target"), String::new())
}

/// Where a route sends traffic: the attached resource when there is one,
/// else the attachment (a blackhole route has neither).
fn tgw_route_target(r: &TgwRoute) -> String {
    let first = if !r.resource_id.is_empty() {
        format!("{} ({})", r.resource_id, r.resource_type)
    } else if !r.attachment_id.is_empty() {
        r.attachment_id.clone()
    } else {
        return "—".to_string();
    };
    // An ECMP route spreads across several attachments; say so rather than
    // read as one target.
    match r.extra_targets {
        0 => first,
        n => format!("{first} +{n} ECMP"),
    }
}

/// The note under a route count when the table holds more than one search
/// returns — counts and lists below are of the first page only.
fn tgw_routes_truncated_row() -> (String, String) {
    (
        format!("  ⚠ More routes than one search returns — showing the first {TGW_ROUTES_MAX}"),
        String::new(),
    )
}

/// `1000+` when the search was cut short.
fn tgw_route_count(set: &TgwRoutes) -> String {
    format!("{}{}", set.routes.len(), if set.more { "+" } else { "" })
}

/// One route as a fixed-width table line (a plain content line), shared by
/// the Routes tab and the Overview so the two can't drift.
fn tgw_route_line(r: &TgwRoute) -> (String, String) {
    (
        format!("  {:<20}  {:<11}  {:<11}  {}", r.cidr, r.state, r.route_type, tgw_route_target(r)),
        String::new(),
    )
}

fn tgw_route_table_overview_rows(
    rt: &TgwRouteTable,
    routes: Option<&crate::lazy::Lazy<TgwRoutes>>,
    associated: &[&crate::aws::services::transit_gateway::TgwAttachment],
) -> Vec<(String, String)> {
    let or_dash = |s: &str| if s.is_empty() { "—".to_string() } else { s.to_string() };
    // The header repeats the id and gateway, but only a body row can be
    // copied with `y` from here. tgw- values are Enter-jumpable.
    let mut rows = vec![("Route Table ID".to_string(), rt.id.clone())];
    if let Some(name) = rt.tags.get("Name").filter(|n| !n.is_empty()) {
        rows.push(("Name".to_string(), name.clone()));
    }
    rows.push(("Transit Gateway".to_string(), or_dash(&rt.tgw_id)));
    rows.push(("State".to_string(), or_dash(&rt.state)));
    if let Some(ct) = &rt.creation_time {
        rows.push(("Created".to_string(), ct.clone()));
    }

    rows.push((String::new(), String::new()));
    rows.push(("Routing".to_string(), String::new()));
    let yes_no = |on: bool, yes: &str| if on { format!("✓ yes — {yes}") } else { "no".to_string() };
    rows.push(("Default Association".to_string(), yes_no(rt.default_association, "new attachments associate here")));
    rows.push(("Default Propagation".to_string(), yes_no(rt.default_propagation, "new attachments propagate here")));
    rows.push((
        "Associations".to_string(),
        if associated.is_empty() {
            "none — no attachment routes by this table".to_string()
        } else {
            let mut kinds: Vec<(String, usize)> = Vec::new();
            for a in associated {
                let k = if a.resource_kind.is_empty() { "other" } else { a.resource_kind.as_str() };
                match kinds.iter_mut().find(|(kind, _)| kind == k) {
                    Some((_, n)) => *n += 1,
                    None => kinds.push((k.to_string(), 1)),
                }
            }
            let kinds: Vec<String> = kinds.iter().map(|(k, n)| format!("{n} {k}")).collect();
            let plural = if associated.len() == 1 { "" } else { "s" };
            format!("{} attachment{plural} ({})", associated.len(), kinds.join(" · "))
        },
    ));

    let routes = match routes {
        None | Some(crate::lazy::Lazy::Loading) => {
            rows.push(("Routes".to_string(), "Loading…".to_string()));
            return rows;
        }
        Some(crate::lazy::Lazy::Error(e)) => {
            rows.extend(error_rows(e));
            return rows;
        }
        Some(crate::lazy::Lazy::Loaded(set)) => set,
    };
    let count = tgw_route_count(routes);
    let routes = &routes.routes;
    let statics = routes.iter().filter(|r| r.route_type == "static").count();
    rows.push((
        "Routes".to_string(),
        format!("{count} ({statics} static · {} propagated)", routes.len() - statics),
    ));
    if count.ends_with('+') {
        rows.push(tgw_routes_truncated_row());
    }
    rows.push((
        "Default Route".to_string(),
        match routes.iter().find(|r| r.cidr == "0.0.0.0/0") {
            Some(r) if r.state == "blackhole" => "0.0.0.0/0 → blackhole".to_string(),
            Some(r) => format!("0.0.0.0/0 → {}", tgw_route_target(r)),
            None => "none — traffic matching no route is dropped".to_string(),
        },
    ));
    let blackholes: Vec<&TgwRoute> = routes.iter().filter(|r| r.state == "blackhole").collect();
    for r in blackholes.iter().take(TGW_RT_OVERVIEW_BLACKHOLES) {
        rows.push((format!("  ⚠ {} is a blackhole — traffic to it is dropped", r.cidr), String::new()));
    }
    if blackholes.len() > TGW_RT_OVERVIEW_BLACKHOLES {
        rows.push((
            String::new(),
            format!("· {} more blackhole routes — 2 Routes", blackholes.len() - TGW_RT_OVERVIEW_BLACKHOLES),
        ));
    }

    rows.push((String::new(), String::new()));
    rows.push((format!("Routes ({count})"), String::new()));
    if routes.is_empty() {
        rows.push((String::new(), "No routes".to_string()));
        return rows;
    }
    rows.push(tgw_routes_table_header());
    rows.extend(routes.iter().take(TGW_RT_OVERVIEW_ROUTES).map(tgw_route_line));
    if routes.len() > TGW_RT_OVERVIEW_ROUTES {
        rows.push((String::new(), format!("· {} more — 2 Routes", routes.len() - TGW_RT_OVERVIEW_ROUTES)));
    }
    rows
}

/// Body lines for a TGW route table. Routes are lazy (SearchTransitGatewayRoutes).
pub fn tgw_route_table_section_lines(
    rt: &TgwRouteTable,
    section: TgwRouteTableDetailSection,
    routes: Option<&crate::lazy::Lazy<TgwRoutes>>,
    associated: &[&crate::aws::services::transit_gateway::TgwAttachment],
) -> Vec<(String, String)> {
    match section {
        TgwRouteTableDetailSection::Overview => tgw_route_table_overview_rows(rt, routes, associated),
        TgwRouteTableDetailSection::Routes => match routes {
            None | Some(crate::lazy::Lazy::Loading) => {
                vec![("".to_string(), "Loading routes…".to_string())]
            }
            Some(crate::lazy::Lazy::Error(e)) => error_rows(e),
            Some(crate::lazy::Lazy::Loaded(set)) => {
                let mut rows = vec![(format!("Routes ({})", tgw_route_count(set)), String::new())];
                if set.more {
                    rows.push(tgw_routes_truncated_row());
                }
                let routes = &set.routes;
                if routes.is_empty() {
                    rows.push((String::new(), "No routes".to_string()));
                } else {
                    rows.push(tgw_routes_table_header());
                    rows.extend(routes.iter().map(tgw_route_line));
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
