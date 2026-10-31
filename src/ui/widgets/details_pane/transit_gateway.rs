use super::*;
use crate::aws::services::transit_gateway::{
    TgwAttachmentDetail, TgwKindDetail, TgwPropagation, TgwRoute, TgwRoutes, TGW_ROUTES_MAX,
};
use crate::lazy::Lazy;

/// `2 vpc · 1 vpn`, in first-seen order — how many of each attachment kind.
fn kind_breakdown<'a>(kinds: impl Iterator<Item = &'a str>) -> String {
    let mut counts: Vec<(&str, usize)> = Vec::new();
    for k in kinds {
        let k = if k.is_empty() { "other" } else { k };
        match counts.iter_mut().find(|(kind, _)| *kind == k) {
            Some((_, n)) => *n += 1,
            None => counts.push((k, 1)),
        }
    }
    counts.iter().map(|(k, n)| format!("{n} {k}")).collect::<Vec<_>>().join(" · ")
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

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
    propagations: Option<&Lazy<Vec<TgwPropagation>>>,
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
            format!(
                "{} attachment{} ({})",
                associated.len(),
                plural(associated.len()),
                kind_breakdown(associated.iter().map(|a| a.resource_kind.as_str()))
            )
        },
    ));
    match propagations {
        None | Some(Lazy::Loading) => rows.push(("Propagations".to_string(), "Loading…".to_string())),
        Some(Lazy::Error(e)) => rows.extend(error_rows(e)),
        Some(Lazy::Loaded(p)) if p.is_empty() => rows.push((
            "Propagations".to_string(),
            "none — no attachment's routes propagate here".to_string(),
        )),
        Some(Lazy::Loaded(p)) => rows.push((
            "Propagations".to_string(),
            format!(
                "{} attachment{} ({}) — 3 Propagations",
                p.len(),
                plural(p.len()),
                kind_breakdown(p.iter().map(|p| p.resource_type.as_str()))
            ),
        )),
    }

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
    propagations: Option<&Lazy<Vec<TgwPropagation>>>,
    associated: &[&crate::aws::services::transit_gateway::TgwAttachment],
) -> Vec<(String, String)> {
    match section {
        TgwRouteTableDetailSection::Overview => tgw_route_table_overview_rows(rt, routes, propagations, associated),
        TgwRouteTableDetailSection::Propagations => tgw_propagation_rows(propagations),
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

/// The route table's Propagations tab: every attachment whose routes it
/// learns — the source of its `propagated` routes.
fn tgw_propagation_rows(propagations: Option<&Lazy<Vec<TgwPropagation>>>) -> Vec<(String, String)> {
    let p = match propagations {
        None | Some(Lazy::Loading) => return vec![(String::new(), "Loading…".to_string())],
        Some(Lazy::Error(e)) => return error_rows(e),
        Some(Lazy::Loaded(p)) => p,
    };
    let mut rows = vec![(format!("Propagations ({})", p.len()), String::new())];
    if p.is_empty() {
        rows.push((String::new(), "No attachment's routes propagate into this table".to_string()));
        return rows;
    }
    rows.push((
        format!("  {:<28}  {:<12}  {:<24}  {}", "Attachment", "Type", "Resource", "State"),
        String::new(),
    ));
    for x in p {
        rows.push((
            format!("  {:<28}  {:<12}  {:<24}  {}", x.attachment_id, x.resource_type, x.resource_id, x.state),
            String::new(),
        ));
    }
    rows
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

/// Kinds whose own describe adds something to the Overview.
fn kind_has_detail(kind: &str) -> bool {
    matches!(kind, "vpc" | "peering" | "tgw-peering" | "connect")
}

pub fn tgw_attachment_section_lines(
    att: &crate::aws::services::transit_gateway::TgwAttachment,
    section: TgwAttachmentDetailSection,
    detail: Option<&Lazy<TgwAttachmentDetail>>,
    account_id: Option<&str>,
    region: &str,
) -> Vec<(String, String)> {
    match section {
        TgwAttachmentDetailSection::Overview => {
            let mut rows = vec![
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

            // Routing: the question an attachment is usually opened for.
            rows.push((String::new(), String::new()));
            rows.push(("Routing".to_string(), String::new()));
            rows.push((
                "Route Table".to_string(),
                att.association_route_table_id
                    .clone()
                    .unwrap_or_else(|| "none — traffic from it matches no table".to_string()),
            ));
            match detail {
                None | Some(Lazy::Loading) => rows.push(("Propagates To".to_string(), "Loading…".to_string())),
                Some(Lazy::Error(e)) => rows.extend(error_rows(e)),
                Some(Lazy::Loaded(d)) => match &d.propagations {
                    Err(e) => rows.extend(error_rows(e)),
                    Ok(p) if p.is_empty() => rows.push((
                        "Propagates To".to_string(),
                        "none — its routes appear in no table on their own".to_string(),
                    )),
                    Ok(p) => rows.push((
                        "Propagates To".to_string(),
                        format!("{} route table{} — 2 Association", p.len(), plural(p.len())),
                    )),
                },
            }

            if kind_has_detail(&att.resource_kind) {
                rows.push((String::new(), String::new()));
                match detail {
                    None | Some(Lazy::Loading) => {
                        rows.push((kind_heading(&att.resource_kind).to_string(), String::new()));
                        rows.push(("Details".to_string(), "Loading…".to_string()));
                    }
                    Some(Lazy::Error(_)) => {}
                    Some(Lazy::Loaded(d)) => rows.extend(kind_rows(att, d, account_id, region)),
                }
            }
            rows
        }
        TgwAttachmentDetailSection::Association => {
            let mut rows = Vec::new();
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
            rows.push((String::new(), String::new()));
            match detail {
                None | Some(Lazy::Loading) => rows.push(("Propagates To".to_string(), "Loading…".to_string())),
                Some(Lazy::Error(e)) => rows.extend(error_rows(e)),
                Some(Lazy::Loaded(d)) => match &d.propagations {
                    Err(e) => rows.extend(error_rows(e)),
                    Ok(p) => {
                        rows.push((format!("Propagates To ({})", p.len()), String::new()));
                        if p.is_empty() {
                            rows.push((String::new(), "Its routes propagate into no route table".to_string()));
                        }
                        for x in p {
                            rows.push((format!("  {:<28}  {}", x.route_table_id, x.state), String::new()));
                        }
                    }
                },
            }
            rows
        }
        TgwAttachmentDetailSection::Tags => tag_rows(&att.tags),
    }
}

fn kind_heading(kind: &str) -> &'static str {
    match kind {
        "vpc" => "VPC",
        "peering" | "tgw-peering" => "Peering",
        "connect" => "Connect",
        _ => "Details",
    }
}

/// The kind block of an attachment's Overview, from its own describe.
fn kind_rows(
    att: &crate::aws::services::transit_gateway::TgwAttachment,
    d: &TgwAttachmentDetail,
    account_id: Option<&str>,
    region: &str,
) -> Vec<(String, String)> {
    let heading = kind_heading(&att.resource_kind).to_string();
    let kind = match &d.kind {
        Err(e) => {
            let mut rows = vec![(heading, String::new())];
            rows.extend(error_rows(e));
            return rows;
        }
        Ok(k) => k,
    };
    let on = |b: bool| if b { "✓ enabled" } else { "disabled" }.to_string();
    let mut rows = vec![(heading, String::new())];
    match kind {
        TgwKindDetail::None => return Vec::new(),
        TgwKindDetail::Vpc(v) => {
            rows.push((
                "Appliance Mode".to_string(),
                if v.appliance_mode {
                    "✓ enabled — a flow keeps one AZ's path both ways".to_string()
                } else {
                    "disabled — an inspection VPC needs it for symmetric return traffic".to_string()
                },
            ));
            rows.push(("DNS Support".to_string(), on(v.dns_support)));
            rows.push(("SG Referencing".to_string(), on(v.sg_referencing)));
            rows.push((String::new(), String::new()));
            rows.push((format!("Subnets ({})", v.subnet_ids.len()), String::new()));
            for id in &v.subnet_ids {
                rows.push((format!("  {id}"), String::new()));
            }
        }
        TgwKindDetail::Peering(p) => {
            let (peer, side) = p.peer(&att.tgw_id);
            rows.push(("This Side".to_string(), side.to_string()));
            rows.push(("Peer Gateway".to_string(), if peer.tgw_id.is_empty() { "—".to_string() } else { peer.tgw_id.clone() }));
            if let Some(cn) = &peer.core_network_id {
                rows.push(("Peer Core Network".to_string(), cn.clone()));
            }
            let own_account = account_id.is_some_and(|a| a == peer.owner_id);
            rows.push((
                "Peer Account".to_string(),
                match (peer.owner_id.is_empty(), own_account) {
                    (true, _) => "—".to_string(),
                    (false, true) => format!("{} (this account)", peer.owner_id),
                    (false, false) => peer.owner_id.clone(),
                },
            ));
            rows.push((
                "Peer Region".to_string(),
                match peer.region.as_str() {
                    "" => "—".to_string(),
                    r if r == region => format!("{r} (this region)"),
                    r => r.to_string(),
                },
            ));
            if !peer.owner_id.is_empty() && account_id.is_some() && !own_account {
                rows.push((String::new(), "· the peer is in another account — P / s to browse it".to_string()));
            }
            let status = match (p.status_code.as_str(), p.status_message.as_str()) {
                ("", "") => "—".to_string(),
                (c, "") => c.to_string(),
                ("", m) => m.to_string(),
                (c, m) => format!("{c} — {m}"),
            };
            rows.push(("Status".to_string(), status));
            if let Some(dynamic) = p.dynamic_routing {
                rows.push(("Dynamic Routing".to_string(), on(dynamic)));
            }
        }
        TgwKindDetail::Connect(c) => {
            rows.push(("Transport Attachment".to_string(), if c.transport_attachment_id.is_empty() { "—".to_string() } else { c.transport_attachment_id.clone() }));
            rows.push(("Protocol".to_string(), if c.protocol.is_empty() { "—".to_string() } else { c.protocol.clone() }));
            rows.push((String::new(), String::new()));
            match &c.peers {
                Err(e) => {
                    rows.push(("Connect Peers".to_string(), String::new()));
                    rows.extend(error_rows(e));
                }
                Ok(peers) => {
                    rows.push((format!("Connect Peers ({})", peers.len()), String::new()));
                    if peers.is_empty() {
                        rows.push((String::new(), "No Connect peers — nothing is routing over this attachment".to_string()));
                    }
                    // Uncapped: AWS allows at most four peers per Connect
                    // attachment.
                    for peer in peers {
                        rows.push(("Peer".to_string(), format!("{} · {}", peer.id, peer.state)));
                        rows.push(("  GRE".to_string(), format!("{} → {}", peer.tgw_address, peer.peer_address)));
                        if !peer.inside_cidrs.is_empty() {
                            rows.push(("  Inside CIDRs".to_string(), peer.inside_cidrs.join(", ")));
                        }
                        for b in &peer.bgp {
                            let asn = |a: Option<i64>| a.map(|a| a.to_string()).unwrap_or_else(|| "?".to_string());
                            let status = match b.status.as_str() {
                                "up" => "✓ up".to_string(),
                                "down" => "✗ down".to_string(),
                                "" => "—".to_string(),
                                other => other.to_string(),
                            };
                            rows.push((
                                "  BGP".to_string(),
                                format!(
                                    "{} {} ↔ {} {} · {status}",
                                    asn(b.tgw_asn), b.tgw_address, asn(b.peer_asn), b.peer_address
                                ),
                            ));
                        }
                    }
                }
            }
        }
    }
    rows
}
