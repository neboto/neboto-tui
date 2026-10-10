use super::*;

// ── Subnet Split Pane ─────────────────────────────────────────────────────────

pub(super) fn render_subnet_split(app: &App, subnet: &Subnet, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 4, "");

    let mut block = theme::pane_block("Subnet", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let header_lines = build_subnet_header_lines(subnet);
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
    render_subnet_section_tabs(app, chunks[2], frame);
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub(super) fn build_subnet_header_lines(subnet: &Subnet) -> Vec<Line<'static>> {
    let state_color = match subnet.state.as_str() {
        "available" => theme::success(),
        "pending" => theme::warning(),
        _ => crate::ui::theme::text_muted(),
    };
    let state_dot = match subnet.state.as_str() {
        "available" => "● ",
        _ => "◌ ",
    };
    let name = subnet
        .tags
        .get("Name")
        .map(|s| s.as_str())
        .unwrap_or(&subnet.subnet_id);

    let mut lines: Vec<Line<'static>> = vec![Line::raw("")];

    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            name.to_string(),
            Style::default().fg(crate::ui::theme::text_primary()).add_modifier(Modifier::BOLD),
        ),
    ]));

    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(subnet.subnet_id.clone(), Style::default().fg(theme::text_dim())),
        Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
        Span::styled(state_dot.to_string(), Style::default().fg(state_color)),
        Span::styled(
            subnet.state.clone(),
            Style::default().fg(state_color).add_modifier(Modifier::BOLD),
        ),
        Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
        Span::styled(subnet.availability_zone.clone(), Style::default().fg(theme::aws_orange())),
    ]));

    lines.push(Line::raw(""));
    lines.push(header_kv("VPC", &subnet.vpc_id));
    lines.push(header_kv("CIDR", &subnet.cidr_block));
    lines.push(header_kv("Available IPs", &subnet.available_ip_count.to_string()));
    if subnet.default_for_az {
        lines.push(header_kv("Default for AZ", "Yes"));
    }
    lines.push(Line::raw(""));
    lines
}

pub(super) fn render_subnet_section_tabs(app: &App, area: Rect, frame: &mut Frame) {
    render_section_tab_bar(app, area, frame, &descriptor_tabs(app, &crate::aws::services::vpc::SUBNET_SECTIONS));
}

/// Render a list of route-table entries as a fixed-width Destination / Target
/// / State / Origin table. Shared by the Route Table pane and the Subnet's
/// Routes section. Built-in (local) routes sort first.
pub(super) fn route_rows(routes: &[RouteEntry]) -> Vec<(String, String)> {
    let mut rows = vec![];
    if routes.is_empty() {
        rows.push(("  (none)".to_string(), "".to_string()));
        return rows;
    }
    let mut sorted = routes.to_vec();
    sorted.sort_by_key(|r| if r.origin == "Built-in" { 0 } else { 1 });
    rows.push((
        format!("  {:<26}  {:<26}  {:<12}  {}", "Destination", "Target", "State", "Origin"),
        "".to_string(),
    ));
    rows.push((
        format!("  {:<26}  {:<26}  {:<12}  {}", "──────────────────────────", "──────────────────────────", "────────────", "────────────"),
        "".to_string(),
    ));
    for route in &sorted {
        rows.push((
            format!("  {:<26}  {:<26}  {:<12}  {}",
                route.destination, route.target, route.state, route.origin),
            "".to_string(),
        ));
    }
    rows
}

pub fn subnet_section_lines(
    subnet: &Subnet,
    section: SubnetDetailSection,
    route_table: Option<&RouteTable>,
) -> Vec<(String, String)> {
    match section {
        SubnetDetailSection::Details => {
            let mut rows = vec![
                ("  Subnet ID".to_string(), subnet.subnet_id.clone()),
                ("  VPC ID".to_string(), subnet.vpc_id.clone()),
                ("  CIDR Block".to_string(), subnet.cidr_block.clone()),
                ("  Availability Zone".to_string(), subnet.availability_zone.clone()),
                ("  Available IPs".to_string(), subnet.available_ip_count.to_string()),
                ("  State".to_string(), subnet.state.clone()),
                (
                    "  Default for AZ".to_string(),
                    if subnet.default_for_az { "Yes" } else { "No" }.to_string(),
                ),
                (
                    "  Auto-assign Public IP".to_string(),
                    if subnet.map_public_ip { "Yes" } else { "No" }.to_string(),
                ),
            ];
            rows.push(("".to_string(), "".to_string()));
            rows
        }
        SubnetDetailSection::Network => {
            let mut rows = vec![
                (
                    "  Auto-assign Public IPv4".to_string(),
                    if subnet.map_public_ip { "✓ Enabled" } else { "✗ Disabled" }.to_string(),
                ),
                (
                    "  Assign IPv6 on Creation".to_string(),
                    if subnet.assign_ipv6_on_creation { "✓ Enabled" } else { "✗ Disabled" }
                        .to_string(),
                ),
                (
                    "  DNS64".to_string(),
                    if subnet.enable_dns64 { "✓ Enabled" } else { "✗ Disabled" }.to_string(),
                ),
                (
                    "  IPv6 Native".to_string(),
                    if subnet.ipv6_native { "Yes" } else { "No" }.to_string(),
                ),
            ];
            rows.push(("".to_string(), "".to_string()));
            if subnet.ipv6_cidr_blocks.is_empty() {
                rows.push(("  IPv6 CIDRs".to_string(), "None".to_string()));
            } else {
                rows.push(("".to_string(), "".to_string()));
                rows.push(("IPv6 CIDR Blocks".to_string(), "".to_string()));
                rows.push(("".to_string(), "".to_string()));
                for cidr in &subnet.ipv6_cidr_blocks {
                    rows.push((format!("  {}", cidr), "".to_string()));
                }
            }
            rows.push(("".to_string(), "".to_string()));
            rows
        }
        SubnetDetailSection::Routes => {
            let mut rows = vec![];
            match route_table {
                Some(rt) => {
                    let explicit = rt.associations.iter().any(|a| {
                        a.subnet_id.as_deref() == Some(subnet.subnet_id.as_str())
                    });
                    // rtb- id is a jump target (Enter/gd → Route Tables sub-tab).
                    rows.push(("  Route Table".to_string(), rt.route_table_id.clone()));
                    rows.push((
                        "  Association".to_string(),
                        if explicit {
                            "Explicit".to_string()
                        } else {
                            "Main (implicit default)".to_string()
                        },
                    ));
                    rows.push(("".to_string(), "".to_string()));
                    rows.push(("Routes".to_string(), "".to_string()));
                    rows.push(("".to_string(), "".to_string()));
                    rows.extend(route_rows(&rt.routes));
                }
                None => {
                    rows.push((
                        "  Route Table".to_string(),
                        "Not resolved (load the Route Tables sub-tab)".to_string(),
                    ));
                }
            }
            rows.push(("".to_string(), "".to_string()));
            rows
        }
        SubnetDetailSection::Tags => {
            let mut rows = vec![];
            if subnet.tags.is_empty() {
                rows.push(("  (none)".to_string(), "No tags".to_string()));
            } else {
                let mut sorted: Vec<_> = subnet.tags.iter().collect();
                sorted.sort_by_key(|(k, _)| k.as_str());
                for (k, v) in sorted {
                    rows.push((format!("  {}", k), v.clone()));
                }
            }
            rows.push(("".to_string(), "".to_string()));
            rows
        }
    }
}

// ── Route Table Split Pane ────────────────────────────────────────────────────

pub(super) fn render_route_table_split(app: &App, rt: &RouteTable, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 3, "");

    let mut block = theme::pane_block("Route Table", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let header_lines = build_route_table_header_lines(rt);
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
    render_route_table_section_tabs(app, chunks[2], frame);
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub(super) fn build_route_table_header_lines(rt: &RouteTable) -> Vec<Line<'static>> {
    let name = rt.tags.get("Name").map(|s| s.as_str()).unwrap_or(&rt.route_table_id);
    let subnet_assoc_count = rt.associations.iter().filter(|a| a.subnet_id.is_some()).count();

    let mut lines: Vec<Line<'static>> = vec![Line::raw("")];

    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            name.to_string(),
            Style::default().fg(crate::ui::theme::text_primary()).add_modifier(Modifier::BOLD),
        ),
    ]));

    let mut id_line = vec![
        Span::raw("  "),
        Span::styled(rt.route_table_id.clone(), Style::default().fg(theme::text_dim())),
    ];
    if rt.is_main {
        id_line.push(Span::styled("  ·  ", Style::default().fg(theme::text_dim())));
        id_line.push(Span::styled(
            "MAIN",
            Style::default().fg(theme::aws_orange()).add_modifier(Modifier::BOLD),
        ));
    }
    lines.push(Line::from(id_line));

    lines.push(Line::raw(""));
    lines.push(header_kv("VPC", &rt.vpc_id));
    lines.push(header_kv("Routes", &rt.routes.len().to_string()));
    lines.push(header_kv("Associations", &subnet_assoc_count.to_string()));
    lines.push(Line::raw(""));
    lines
}

pub(super) fn render_route_table_section_tabs(app: &App, area: Rect, frame: &mut Frame) {
    render_section_tab_bar(app, area, frame, &descriptor_tabs(app, &crate::aws::services::vpc::ROUTE_TABLE_SECTIONS));
}

pub fn route_table_section_lines(
    rt: &RouteTable,
    section: RouteTableDetailSection,
) -> Vec<(String, String)> {
    match section {
        RouteTableDetailSection::Routes => {
            let mut rows = route_rows(&rt.routes);
            if !rt.propagating_vgws.is_empty() {
                rows.push(("".to_string(), "".to_string()));
                rows.push(("VGW Route Propagation".to_string(), "".to_string()));
                for v in &rt.propagating_vgws {
                    rows.push(("  Propagating VGW".to_string(), v.clone()));
                }
            }
            rows.push(("".to_string(), "".to_string()));
            rows
        }
        RouteTableDetailSection::Associations => {
            let mut rows = vec![];
            let subnet_assocs: Vec<_> =
                rt.associations.iter().filter(|a| a.subnet_id.is_some()).collect();
            if rt.is_main {
                rows.push(("  (main)  Implicitly associated with all subnets without an explicit association".to_string(), "".to_string()));
            }
            if subnet_assocs.is_empty() && !rt.is_main {
                rows.push(("  (none)".to_string(), "".to_string()));
            } else {
                rows.push((
                    format!("  {:<26}  {}", "Subnet ID", "State"),
                    "".to_string(),
                ));
                rows.push((
                    format!("  {:<26}  {}", "──────────────────────────", "──────────────"),
                    "".to_string(),
                ));
                for assoc in subnet_assocs {
                    if let Some(subnet_id) = &assoc.subnet_id {
                        rows.push((
                            format!("  {:<26}  {}", subnet_id, assoc.state),
                            "".to_string(),
                        ));
                    }
                }
            }
            // Edge associations — the table serves an IGW/VGW (ingress
            // routing) instead of a subnet. Key-value rows so the gateway
            // ids stay Enter-jumpable.
            let edge_assocs: Vec<_> = rt
                .associations
                .iter()
                .filter(|a| a.gateway_id.is_some())
                .collect();
            if !edge_assocs.is_empty() {
                rows.push(("".to_string(), "".to_string()));
                rows.push(("Edge Associations".to_string(), "".to_string()));
                for a in edge_assocs {
                    if let Some(g) = &a.gateway_id {
                        let val = if a.state == "associated" {
                            g.clone()
                        } else {
                            format!("{} — ⚠ {}", g, a.state)
                        };
                        rows.push(("  Gateway".to_string(), val));
                    }
                }
            }
            rows.push(("".to_string(), "".to_string()));
            rows
        }
        RouteTableDetailSection::Tags => {
            let mut rows = vec![];
            if rt.tags.is_empty() {
                rows.push(("  (none)".to_string(), "No tags".to_string()));
            } else {
                let mut sorted: Vec<_> = rt.tags.iter().collect();
                sorted.sort_by_key(|(k, _)| k.as_str());
                for (k, v) in sorted {
                    rows.push((format!("  {}", k), v.clone()));
                }
            }
            rows.push(("".to_string(), "".to_string()));
            rows
        }
    }
}

// ── Network ACL Split Pane ────────────────────────────────────────────────────

pub(super) fn render_network_acl_split(app: &App, acl: &NetworkAcl, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 4, "");

    let mut block = theme::pane_block("Network ACL", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let header_lines = build_network_acl_header_lines(acl);
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
    render_network_acl_section_tabs(app, chunks[2], frame);
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub(super) fn build_network_acl_header_lines(acl: &NetworkAcl) -> Vec<Line<'static>> {
    let name = acl.tags.get("Name").map(|s| s.as_str()).unwrap_or(&acl.acl_id);

    let mut lines: Vec<Line<'static>> = vec![Line::raw("")];

    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            name.to_string(),
            Style::default().fg(crate::ui::theme::text_primary()).add_modifier(Modifier::BOLD),
        ),
    ]));

    let mut id_line = vec![
        Span::raw("  "),
        Span::styled(acl.acl_id.clone(), Style::default().fg(theme::text_dim())),
    ];
    if acl.is_default {
        id_line.push(Span::styled("  ·  ", Style::default().fg(theme::text_dim())));
        id_line.push(Span::styled(
            "DEFAULT",
            Style::default().fg(theme::aws_orange()).add_modifier(Modifier::BOLD),
        ));
    }
    lines.push(Line::from(id_line));

    lines.push(Line::raw(""));
    lines.push(header_kv("VPC", &acl.vpc_id));
    lines.push(header_kv("Inbound Rules", &acl.inbound_entries.len().to_string()));
    lines.push(header_kv("Outbound Rules", &acl.outbound_entries.len().to_string()));
    lines.push(header_kv("Associations", &acl.associations.len().to_string()));
    lines.push(Line::raw(""));
    lines
}

pub(super) fn render_network_acl_section_tabs(app: &App, area: Rect, frame: &mut Frame) {
    render_section_tab_bar(app, area, frame, &descriptor_tabs(app, &crate::aws::services::vpc::NETWORK_ACL_SECTIONS));
}

pub fn network_acl_section_lines(
    acl: &NetworkAcl,
    section: NetworkAclDetailSection,
) -> Vec<(String, String)> {
    let render_entries = |entries: &[crate::aws::services::vpc::AclEntry]| {
        let mut rows: Vec<(String, String)> = Vec::new();
        rows.push(("".to_string(), "".to_string()));
        // Column header — plain content line (starts with space, empty value)
        rows.push((
            format!("  {:>5}  {:<9}  {:<10}  {:<22}  {}", "Rule", "Action", "Protocol", "CIDR", "Ports"),
            "".to_string(),
        ));
        rows.push((
            format!("  {:>5}  {:<9}  {:<10}  {:<22}  {}", "─────", "─────────", "──────────", "──────────────────────", "─────────────"),
            "".to_string(),
        ));
        if entries.is_empty() {
            rows.push(("  (none)".to_string(), "".to_string()));
        } else {
            for entry in entries {
                let action = if entry.action == "allow" { "✓ ALLOW" } else { "✗ DENY" };
                rows.push((
                    format!("  {:>5}  {:<9}  {:<10}  {:<22}  {}",
                        entry.rule_number, action, entry.protocol, entry.cidr, entry.port_range),
                    "".to_string(),
                ));
            }
        }
        rows.push(("".to_string(), "".to_string()));
        rows
    };

    match section {
        NetworkAclDetailSection::Inbound => render_entries(&acl.inbound_entries),
        NetworkAclDetailSection::Outbound => render_entries(&acl.outbound_entries),
        NetworkAclDetailSection::Associations => {
            let mut rows = vec![];
            if acl.associations.is_empty() {
                rows.push(("  (none)".to_string(), "".to_string()));
            } else {
                rows.push((
                    format!("  {:<24}  {}", "Subnet ID", "Association ID"),
                    "".to_string(),
                ));
                rows.push((
                    format!("  {:<24}  {}", "────────────────────────", "──────────────────────────────"),
                    "".to_string(),
                ));
                for assoc in &acl.associations {
                    rows.push((
                        format!("  {:<24}  {}", assoc.subnet_id, assoc.association_id),
                        "".to_string(),
                    ));
                }
            }
            rows.push(("".to_string(), "".to_string()));
            rows
        }
        NetworkAclDetailSection::Tags => {
            let mut rows = vec![];
            if acl.tags.is_empty() {
                rows.push(("  (none)".to_string(), "No tags".to_string()));
            } else {
                let mut sorted: Vec<_> = acl.tags.iter().collect();
                sorted.sort_by_key(|(k, _)| k.as_str());
                for (k, v) in sorted {
                    rows.push((format!("  {}", k), v.clone()));
                }
            }
            rows.push(("".to_string(), "".to_string()));
            rows
        }
    }
}

// ── VPC Endpoint split pane ────────────────────────────────────────────────────

pub(super) fn render_vpc_endpoint_split(app: &App, ep: &VpcEndpoint, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 4, "");
    let mut block = theme::pane_block("VPC Endpoint", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let state_color = match ep.state.as_str() {
        "available" => theme::success(),
        "pending" | "pendingAcceptance" => theme::warning(),
        _ => theme::text_dim(),
    };
    let header = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                ep.name().to_string(),
                Style::default().fg(crate::ui::theme::text_primary()).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(ep.id.clone(), Style::default().fg(theme::text_dim())),
            Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
            Span::styled(ep.kind.clone(), Style::default().fg(theme::accent())),
            Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
            Span::styled(ep.state.clone(), Style::default().fg(state_color)),
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
    render_section_tab_bar(app, chunks[2], frame, &descriptor_tabs(app, &crate::aws::services::vpc::VPC_ENDPOINT_SECTIONS));
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub fn vpc_endpoint_section_lines(
    ep: &VpcEndpoint,
    section: VpcEndpointDetailSection,
) -> Vec<(String, String)> {
    let list_rows = |label: &str, items: &[String]| -> Vec<(String, String)> {
        let mut rows = vec![(label.to_string(), String::new())];
        if items.is_empty() {
            rows.push(("  —".to_string(), String::new()));
        } else {
            for it in items {
                rows.push((format!("  {}", it), String::new()));
            }
        }
        rows.push((String::new(), String::new()));
        rows
    };
    match section {
        VpcEndpointDetailSection::Details => {
            let mut rows = vec![
                ("Service".to_string(), ep.service_name.clone()),
                ("Type".to_string(), ep.kind.clone()),
                ("State".to_string(), ep.state.clone()),
                ("VPC".to_string(), ep.vpc_id.clone()),
                (
                    "Private DNS".to_string(),
                    if ep.private_dns_enabled { "✓ enabled" } else { "disabled" }.to_string(),
                ),
            ];
            if let Some(c) = &ep.creation {
                rows.push(("Created".to_string(), c.clone()));
            }
            rows
        }
        VpcEndpointDetailSection::Network => {
            // Interface vs Gateway expose different fields.
            let mut rows = Vec::new();
            if ep.kind == "Interface" || ep.kind == "GatewayLoadBalancer" {
                rows.extend(list_rows("Subnets", &ep.subnet_ids));
                rows.extend(list_rows("Security Groups", &ep.security_group_ids));
                rows.extend(list_rows("Network Interfaces", &ep.network_interface_ids));
                rows.extend(list_rows("DNS Entries", &ep.dns_entries));
            } else {
                rows.extend(list_rows("Route Tables", &ep.route_table_ids));
            }
            rows
        }
        VpcEndpointDetailSection::Policy => match &ep.policy_document {
            Some(doc) => {
                let pretty = crate::aws::services::iam::pretty_policy_document(doc);
                let mut rows = vec![("Policy".to_string(), String::new()), (String::new(), String::new())];
                for line in pretty.lines() {
                    rows.push((format!(" {}", line), String::new()));
                }
                rows
            }
            None => vec![("".to_string(), "No endpoint policy (full access)".to_string())],
        },
        VpcEndpointDetailSection::Tags => tag_rows(&ep.tags),
    }
}

// ── VPN Connection split pane ──────────────────────────────────────────────────

pub(super) fn render_vpn_connection_split(app: &App, vpn: &VpnConnection, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 4, "");
    let mut block = theme::pane_block("VPN Connection", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let up = vpn.tunnels_up();
    let total = vpn.tunnels.len();
    let tun_color = if total > 0 && up == total {
        theme::success()
    } else if up == 0 {
        theme::error()
    } else {
        theme::warning()
    };
    let header = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                vpn.name().to_string(),
                Style::default().fg(crate::ui::theme::text_primary()).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(vpn.id.clone(), Style::default().fg(theme::text_dim())),
            Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
            Span::styled(vpn.state.clone(), Style::default().fg(theme::text_dim())),
            Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
            Span::styled(
                format!("{}/{} tunnels up", up, total),
                Style::default().fg(tun_color),
            ),
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
    render_section_tab_bar(app, chunks[2], frame, &descriptor_tabs(app, &crate::aws::services::vpc::VPN_CONNECTION_SECTIONS));
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub fn vpn_connection_section_lines(
    vpn: &VpnConnection,
    section: VpnConnectionDetailSection,
) -> Vec<(String, String)> {
    match section {
        VpnConnectionDetailSection::Tunnels => {
            let mut rows = Vec::new();
            if vpn.tunnels.is_empty() {
                rows.push(("".to_string(), "No tunnel telemetry".to_string()));
                return rows;
            }
            for (i, t) in vpn.tunnels.iter().enumerate() {
                let mark = if t.status == "UP" { "✓" } else { "✗" };
                rows.push((format!("Tunnel {}", i + 1), String::new()));
                rows.push(("  Outside IP".to_string(), t.outside_ip.clone()));
                rows.push(("  Status".to_string(), format!("{} {}", mark, t.status)));
                rows.push((
                    "  Accepted Routes".to_string(),
                    t.accepted_route_count.to_string(),
                ));
                if let Some(c) = &t.last_status_change {
                    rows.push(("  Last Change".to_string(), c.clone()));
                }
                if !t.status_message.is_empty() {
                    rows.push(("  Message".to_string(), t.status_message.clone()));
                }
                rows.push((String::new(), String::new()));
            }
            rows
        }
        VpnConnectionDetailSection::Routes => {
            if vpn.routing.starts_with("dynamic") {
                return vec![("".to_string(), "Dynamic routing (BGP) — no static routes".to_string())];
            }
            let mut rows = vec![(format!("Static Routes ({})", vpn.routes.len()), String::new())];
            rows.push((String::new(), String::new()));
            if vpn.routes.is_empty() {
                rows.push(("  No static routes".to_string(), String::new()));
            } else {
                for cidr in &vpn.routes {
                    rows.push((format!("  {}", cidr), String::new()));
                }
            }
            rows
        }
        VpnConnectionDetailSection::Details => {
            let mut rows = vec![
                ("State".to_string(), vpn.state.clone()),
                ("Category".to_string(), vpn.category.clone()),
                ("Routing".to_string(), vpn.routing.clone()),
                (String::new(), String::new()),
                ("Customer Gateway".to_string(), String::new()),
                ("  ID".to_string(), vpn.customer_gateway_id.clone()),
            ];
            if let Some(ip) = &vpn.customer_gateway_ip {
                rows.push(("  IP Address".to_string(), ip.clone()));
            }
            if let Some(asn) = &vpn.customer_gateway_asn {
                rows.push(("  BGP ASN".to_string(), asn.clone()));
            }
            rows.push((String::new(), String::new()));
            if let Some(tgw) = &vpn.transit_gateway_id {
                rows.push(("Transit Gateway".to_string(), tgw.clone()));
            } else if let Some(vgw) = &vpn.vpn_gateway_id {
                rows.push(("VPN Gateway".to_string(), vgw.clone()));
            }
            rows
        }
        VpnConnectionDetailSection::Tags => tag_rows(&vpn.tags),
    }
}

pub(super) fn render_vpc_split(app: &App, vpc: &Vpc, area: Rect, frame: &mut Frame) {
    let name = vpc
        .tags
        .get("Name")
        .map(|s| s.as_str())
        .unwrap_or(&vpc.vpc_id);
    let extra = vpc.secondary_cidrs.len() + vpc.ipv6_cidrs.len();
    let subtitle = if extra > 0 {
        format!("{} +{}", vpc.cidr_block, extra)
    } else {
        vpc.cidr_block.clone()
    };
    render_simple_split(
        app,
        area,
        frame,
        "VPC",
        name,
        &subtitle,
        &descriptor_tabs(app, &crate::aws::services::vpc::VPC_SECTIONS),
    );
}

#[allow(clippy::too_many_arguments)]
pub fn vpc_section_lines(
    vpc: &Vpc,
    section: VpcDetailSection,
    subnets: &[&Subnet],
    igws: &[&InternetGateway],
    nats: &[&NatGateway],
    endpoints: &[&VpcEndpoint],
    dhcp: Option<&DhcpOptionsSet>,
    flow_logs: Option<&Lazy<Vec<FlowLogInfo>>>,
    dns_attrs: Option<&Lazy<(bool, bool)>>,
) -> Vec<(String, String)> {
    match section {
        VpcDetailSection::Overview => {
            let mut rows = vec![
                (String::new(), String::new()),
                ("VPC ID".to_string(), vpc.vpc_id.clone()),
                ("State".to_string(), vpc.state.clone()),
                ("CIDR Block".to_string(), vpc.cidr_block.clone()),
            ];
            rows.extend(vpc.cidr_rows());
            rows.push((
                "Default VPC".to_string(),
                if vpc.is_default {
                    "✓ yes".to_string()
                } else {
                    "no".to_string()
                },
            ));
            if let Some(owner) = &vpc.owner_id {
                rows.push(("Owner".to_string(), owner.clone()));
            }
            if let Some(t) = &vpc.instance_tenancy {
                rows.push(("Instance Tenancy".to_string(), t.clone()));
            }
            if let Some(d) = &vpc.dhcp_options_id {
                rows.push(("DHCP Options".to_string(), d.clone()));
            }
            rows.push((String::new(), String::new()));
            rows.push(("DNS / DHCP".to_string(), String::new()));
            // The two attribute flags are a lazy DescribeVpcAttribute pair.
            match dns_attrs {
                None | Some(Lazy::Loading) => {
                    rows.push(("  Loading…".to_string(), String::new()));
                }
                Some(Lazy::Error(e)) => rows.extend(error_rows(e)),
                Some(Lazy::Loaded((support, hostnames))) => {
                    let on_off = |on: bool| {
                        if on {
                            "✓ enabled".to_string()
                        } else {
                            "✗ disabled".to_string()
                        }
                    };
                    rows.push(("  DNS Support".to_string(), on_off(*support)));
                    rows.push(("  DNS Hostnames".to_string(), on_off(*hostnames)));
                }
            }
            // Resolved from the sibling DHCP options set — no extra fetch.
            if let Some(d) = dhcp {
                for (k, values) in &d.configs {
                    rows.push((format!("  {}", pretty_dhcp_key(k)), values.join(", ")));
                }
            }
            rows.push((String::new(), String::new()));
            rows.push(("In this VPC".to_string(), String::new()));
            rows.push(("Subnets".to_string(), subnets.len().to_string()));
            rows.push(("Internet Gateways".to_string(), igws.len().to_string()));
            rows.push(("NAT Gateways".to_string(), nats.len().to_string()));
            rows.push(("VPC Endpoints".to_string(), endpoints.len().to_string()));
            rows
        }
        VpcDetailSection::Subnets => {
            let mut rows = vec![(String::new(), String::new())];
            if subnets.is_empty() {
                rows.push(("".to_string(), "No subnets in this VPC".to_string()));
            }
            for s in subnets {
                // IPv6-native subnets have no IPv4 CIDR — show the IPv6 one.
                let cidr = if s.cidr_block == "unknown" {
                    s.ipv6_cidr_blocks
                        .first()
                        .cloned()
                        .unwrap_or_else(|| "—".to_string())
                } else {
                    s.cidr_block.clone()
                };
                rows.push((
                    s.subnet_id.clone(),
                    format!(
                        "{} · {} · {} IPs free",
                        cidr, s.availability_zone, s.available_ip_count
                    ),
                ));
            }
            rows
        }
        VpcDetailSection::Gateways => {
            let mut rows = vec![(String::new(), String::new())];
            rows.push(("Internet Gateways".to_string(), String::new()));
            if igws.is_empty() {
                rows.push(("  none".to_string(), String::new()));
            }
            for g in igws {
                rows.push((g.igw_id.clone(), g.state.clone()));
            }
            rows.push((String::new(), String::new()));
            rows.push(("NAT Gateways".to_string(), String::new()));
            if nats.is_empty() {
                rows.push(("  none".to_string(), String::new()));
            }
            for n in nats {
                let ip = n
                    .public_ip
                    .as_deref()
                    .or(n.private_ip.as_deref())
                    .unwrap_or("—");
                rows.push((n.nat_gateway_id.clone(), format!("{} · {}", n.state, ip)));
            }
            rows.push((String::new(), String::new()));
            rows.push(("VPC Endpoints".to_string(), String::new()));
            if endpoints.is_empty() {
                rows.push(("  none".to_string(), String::new()));
            }
            for e in endpoints {
                rows.push((e.id.clone(), format!("{} · {}", e.kind, e.service_name)));
            }
            rows
        }
        VpcDetailSection::FlowLogs => match flow_logs {
            None | Some(Lazy::Loading) => {
                vec![
                    (String::new(), String::new()),
                    ("  Loading…".to_string(), String::new()),
                ]
            }
            Some(Lazy::Error(e)) => error_rows(e),
            Some(Lazy::Loaded(logs)) => {
                let mut rows = vec![(String::new(), String::new())];
                if logs.is_empty() {
                    rows.push((
                        "  No VPC-level flow logs — traffic is not being captured"
                            .to_string(),
                        String::new(),
                    ));
                    rows.push((
                        "  (subnet- or ENI-level flow logs aren't listed here)".to_string(),
                        String::new(),
                    ));
                    return rows;
                }
                for fl in logs {
                    rows.push((fl.id.clone(), String::new()));
                    rows.push((
                        "  Status".to_string(),
                        if fl.status == "ACTIVE" {
                            "✓ active".to_string()
                        } else {
                            fl.status.clone()
                        },
                    ));
                    rows.push(("  Traffic".to_string(), fl.traffic_type.clone()));
                    rows.push((
                        format!("  Destination ({})", fl.dest_type),
                        fl.destination.clone(),
                    ));
                    if fl.log_format_custom {
                        rows.push(("  Format".to_string(), "custom".to_string()));
                    }
                    if let Some(secs) = fl.aggregation_secs {
                        rows.push(("  Aggregation".to_string(), format!("{}s", secs)));
                    }
                    if let Some(err) = &fl.deliver_error {
                        rows.push((format!("  ⚠ delivery: {}", err), String::new()));
                    }
                    rows.push((String::new(), String::new()));
                }
                rows
            }
        },
        VpcDetailSection::Tags => tag_rows(&vpc.tags),
    }
}

pub(super) fn render_dhcp_options_split(app: &App, dopt: &DhcpOptionsSet, area: Rect, frame: &mut Frame) {
    let name = dopt
        .tags
        .get("Name")
        .map(|s| s.as_str())
        .unwrap_or(&dopt.id);
    let subtitle = dopt
        .values_for("domain-name")
        .unwrap_or_else(|| "DHCP options set".to_string());
    render_simple_split(
        app,
        area,
        frame,
        "DHCP Options",
        name,
        &subtitle,
        &descriptor_tabs(app, &crate::aws::services::vpc::DHCP_OPTIONS_SECTIONS),
    );
}

pub fn dhcp_options_section_lines(
    dopt: &DhcpOptionsSet,
    section: DhcpOptionsDetailSection,
    vpcs: &[&Vpc],
) -> Vec<(String, String)> {
    match section {
        DhcpOptionsDetailSection::Overview => {
            let mut rows = vec![
                (String::new(), String::new()),
                ("ID".to_string(), dopt.id.clone()),
            ];
            if let Some(owner) = &dopt.owner_id {
                rows.push(("Owner".to_string(), owner.clone()));
            }
            rows.push((String::new(), String::new()));
            rows.push(("Configuration".to_string(), String::new()));
            if dopt.configs.is_empty() {
                rows.push(("  no options configured".to_string(), String::new()));
            }
            for (k, values) in &dopt.configs {
                rows.push((format!("  {}", pretty_dhcp_key(k)), values.join(", ")));
            }
            rows.push((String::new(), String::new()));
            rows.push(("VPCs Using".to_string(), vpcs.len().to_string()));
            rows
        }
        DhcpOptionsDetailSection::Vpcs => {
            let mut rows = vec![(String::new(), String::new())];
            if vpcs.is_empty() {
                rows.push((
                    "  No VPCs use this options set".to_string(),
                    String::new(),
                ));
            }
            for v in vpcs {
                let name = v.tags.get("Name").map(|s| s.as_str()).unwrap_or("—");
                rows.push((v.vpc_id.clone(), format!("{} · {}", name, v.cidr_block)));
            }
            rows
        }
        DhcpOptionsDetailSection::Tags => tag_rows(&dopt.tags),
    }
}

pub(super) fn render_vpc_peering_split(app: &App, peer: &VpcPeering, area: Rect, frame: &mut Frame) {
    let name = peer
        .tags
        .get("Name")
        .map(|s| s.as_str())
        .unwrap_or(&peer.id);
    let subtitle = format!(
        "{} ⇄ {} · {}",
        peer.requester.vpc_id.as_deref().unwrap_or("—"),
        peer.accepter.vpc_id.as_deref().unwrap_or("—"),
        peer.status
    );
    render_simple_split(
        app,
        area,
        frame,
        "Peering Connection",
        name,
        &subtitle,
        &descriptor_tabs(app, &crate::aws::services::vpc::VPC_PEERING_SECTIONS),
    );
}

pub fn vpc_peering_section_lines(
    peer: &VpcPeering,
    section: VpcPeeringDetailSection,
) -> Vec<(String, String)> {
    match section {
        VpcPeeringDetailSection::Overview => {
            let mut rows = vec![
                (String::new(), String::new()),
                ("ID".to_string(), peer.id.clone()),
                ("Status".to_string(), peer.status.clone()),
            ];
            if let Some(m) = &peer.status_message {
                if m != &peer.status {
                    rows.push(("  ".to_string() + m, String::new()));
                }
            }
            if let Some(exp) = &peer.expiration_time {
                rows.push(("Expires".to_string(), exp.clone()));
            }
            let side_rows = |rows: &mut Vec<(String, String)>, label: &str, s: &PeeringSide| {
                rows.push((String::new(), String::new()));
                rows.push((label.to_string(), String::new()));
                rows.push((
                    "  VPC".to_string(),
                    s.vpc_id.clone().unwrap_or_else(|| "—".to_string()),
                ));
                if !s.cidrs.is_empty() {
                    rows.push(("  CIDRs".to_string(), s.cidrs.join(", ")));
                }
                if let Some(o) = &s.owner_id {
                    rows.push(("  Owner".to_string(), o.clone()));
                }
                if let Some(r) = &s.region {
                    rows.push(("  Region".to_string(), r.clone()));
                }
                if let Some(dns) = s.allow_remote_dns {
                    rows.push((
                        "  Remote DNS Resolution".to_string(),
                        if dns {
                            "✓ enabled".to_string()
                        } else {
                            "disabled".to_string()
                        },
                    ));
                }
            };
            side_rows(&mut rows, "Requester", &peer.requester);
            side_rows(&mut rows, "Accepter", &peer.accepter);
            // Cross-account / cross-region peers can't resolve locally.
            let local_owner_differs = peer.requester.owner_id.is_some()
                && peer.requester.owner_id != peer.accepter.owner_id;
            let region_differs = peer.requester.region.is_some()
                && peer.requester.region != peer.accepter.region;
            if local_owner_differs || region_differs {
                rows.push((String::new(), String::new()));
                rows.push((
                    "  VPC jumps only resolve in the browsed account/region".to_string(),
                    String::new(),
                ));
            }
            rows
        }
        VpcPeeringDetailSection::Tags => tag_rows(&peer.tags),
    }
}

pub(super) fn render_prefix_list_split(app: &App, pl: &PrefixList, area: Rect, frame: &mut Frame) {
    let subtitle = format!(
        "{} · {}",
        pl.address_family.as_deref().unwrap_or("—"),
        if pl.is_aws_managed() {
            "AWS-managed"
        } else {
            "customer-managed"
        }
    );
    render_simple_split(
        app,
        area,
        frame,
        "Prefix List",
        &pl.name,
        &subtitle,
        &descriptor_tabs(app, &crate::aws::services::vpc::PREFIX_LIST_SECTIONS),
    );
}

pub fn prefix_list_section_lines(
    pl: &PrefixList,
    section: PrefixListDetailSection,
    entries: Option<&Lazy<Vec<(String, String)>>>,
) -> Vec<(String, String)> {
    match section {
        PrefixListDetailSection::Overview => {
            let mut rows = vec![
                (String::new(), String::new()),
                ("ID".to_string(), pl.id.clone()),
                ("Name".to_string(), pl.name.clone()),
                ("State".to_string(), pl.state.clone()),
            ];
            if let Some(m) = &pl.state_message {
                rows.push(("  ".to_string() + m, String::new()));
            }
            if let Some(f) = &pl.address_family {
                rows.push(("Address Family".to_string(), f.clone()));
            }
            if let Some(m) = pl.max_entries {
                rows.push(("Max Entries".to_string(), m.to_string()));
            }
            if let Some(v) = pl.version {
                rows.push(("Version".to_string(), v.to_string()));
            }
            rows.push((
                "Owner".to_string(),
                if pl.is_aws_managed() {
                    "AWS (managed list)".to_string()
                } else {
                    pl.owner_id.clone().unwrap_or_else(|| "—".to_string())
                },
            ));
            if let Some(arn) = &pl.arn {
                rows.push(("ARN".to_string(), arn.clone()));
            }
            rows
        }
        PrefixListDetailSection::Entries => match entries {
            None | Some(Lazy::Loading) => {
                vec![
                    (String::new(), String::new()),
                    ("  Loading…".to_string(), String::new()),
                ]
            }
            Some(Lazy::Error(e)) => error_rows(e),
            Some(Lazy::Loaded(list)) => {
                let mut rows = vec![(String::new(), String::new())];
                let cap = pl
                    .max_entries
                    .map(|m| format!(" of max {}", m))
                    .unwrap_or_default();
                rows.push((format!("Entries ({}{})", list.len(), cap), String::new()));
                if list.is_empty() {
                    rows.push(("  no entries".to_string(), String::new()));
                }
                for (cidr, desc) in list {
                    rows.push((format!("  {}", cidr), desc.clone()));
                }
                rows
            }
        },
        PrefixListDetailSection::Tags => tag_rows(&pl.tags),
    }
}

pub(super) fn render_nat_gateway_split(app: &App, nat: &NatGateway, area: Rect, frame: &mut Frame) {
    let subtitle = format!("{} · {}", nat.connectivity_type, nat.state);
    render_simple_split(
        app,
        area,
        frame,
        "NAT Gateway",
        &nat.nat_gateway_id,
        &subtitle,
        &descriptor_tabs(app, &crate::aws::services::vpc::NAT_GATEWAY_SECTIONS),
    );
}

pub fn nat_gateway_section_lines(
    nat: &NatGateway,
    section: NatGatewayDetailSection,
) -> Vec<(String, String)> {
    match section {
        NatGatewayDetailSection::Overview => {
            let mut rows = vec![
                (String::new(), String::new()),
                ("NAT Gateway ID".to_string(), nat.nat_gateway_id.clone()),
                ("State".to_string(), nat.state.clone()),
                ("Connectivity".to_string(), nat.connectivity_type.clone()),
            ];
            if let Some(t) = &nat.create_time {
                rows.push(("Created".to_string(), t.clone()));
            }
            if let Some(code) = &nat.failure_code {
                rows.push((String::new(), String::new()));
                rows.push((
                    format!(
                        "  ⚠ {}: {}",
                        code,
                        nat.failure_message.as_deref().unwrap_or("no message")
                    ),
                    String::new(),
                ));
            }
            rows
        }
        NatGatewayDetailSection::Network => {
            let mut rows = vec![
                (String::new(), String::new()),
                ("VPC".to_string(), nat.vpc_id.clone()),
                ("Subnet".to_string(), nat.subnet_id.clone()),
            ];
            rows.extend(nat.address_rows());
            rows
        }
        NatGatewayDetailSection::Tags => tag_rows(&nat.tags),
    }
}
