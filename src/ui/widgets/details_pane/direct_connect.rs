use super::*;

// ── Direct Connect split panes ─────────────────────────────────────────────────

pub(super) fn render_dx_connection_split(app: &App, conn: &DxConnection, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 3, "");
    let mut block = theme::pane_block("DX Connection", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let header = dx_header_lines(&conn.name, &conn.id, &conn.state, dx_state_color(&conn.state));
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
        &descriptor_tabs(app, &crate::aws::services::direct_connect::DX_CONNECTION_SECTIONS),
    );
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub(super) fn render_dx_vif_split(app: &App, vif: &DxVirtualInterface, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 3, "");
    let mut block = theme::pane_block("DX Virtual Interface", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Header carries the BGP state too — it's the operational signal.
    let name = vif.name.clone();
    let bgp = if vif.bgp_status.is_empty() {
        "no peers".to_string()
    } else {
        format!("BGP {}", vif.bgp_status)
    };
    let bgp_color = match vif.bgp_status.as_str() {
        "up" => theme::success(),
        "down" => theme::error(),
        _ => theme::text_dim(),
    };
    let header = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                name,
                Style::default()
                    .fg(crate::ui::theme::text_primary())
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(vif.id.clone(), Style::default().fg(theme::text_dim())),
            Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
            Span::styled(vif.state.clone(), Style::default().fg(dx_state_color(&vif.state))),
            Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
            Span::styled(bgp, Style::default().fg(bgp_color).add_modifier(Modifier::BOLD)),
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
        &descriptor_tabs(app, &crate::aws::services::direct_connect::DX_VIF_SECTIONS),
    );
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub(super) fn dx_state_color(state: &str) -> Color {
    match state {
        "available" => theme::success(),
        "down" | "rejected" => theme::error(),
        "pending" | "requested" | "ordering" | "confirming" | "verifying" => theme::warning(),
        _ => theme::text_dim(),
    }
}

pub(super) fn dx_header_lines(name: &str, id: &str, state: &str, state_color: Color) -> Vec<Line<'static>> {
    vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                name.to_string(),
                Style::default()
                    .fg(crate::ui::theme::text_primary())
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(id.to_string(), Style::default().fg(theme::text_dim())),
            Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
            Span::styled(state.to_string(), Style::default().fg(state_color)),
        ]),
        Line::raw(""),
    ]
}

/// Body lines for a DX connection. Virtual Interfaces are filtered from the
/// already-loaded list (passed in by `get_detail_lines`), so no extra fetch.
pub fn dx_connection_section_lines(
    conn: &DxConnection,
    section: DxConnectionDetailSection,
    vifs: &[(String, String, String, String)], // id, type, state, bgp
) -> Vec<(String, String)> {
    match section {
        DxConnectionDetailSection::Details => {
            let mut rows = vec![
                ("ID".to_string(), conn.id.clone()),
                ("Name".to_string(), conn.name.clone()),
                ("State".to_string(), conn.state.clone()),
                ("Bandwidth".to_string(), conn.bandwidth.clone()),
                ("Location".to_string(), conn.location.clone()),
                ("Region".to_string(), conn.region.clone()),
            ];
            if let Some(v) = conn.vlan {
                rows.push(("VLAN".to_string(), v.to_string()));
            }
            if !conn.partner.is_empty() {
                rows.push(("Partner".to_string(), conn.partner.clone()));
            }
            if let Some(lag) = &conn.lag_id {
                rows.push(("LAG".to_string(), lag.clone()));
            }
            if !conn.aws_device.is_empty() {
                rows.push(("AWS Device".to_string(), conn.aws_device.clone()));
            }
            rows.push((String::new(), String::new()));
            rows.push((
                "Jumbo Frames".to_string(),
                if conn.jumbo_capable { "✓ capable" } else { "no" }.to_string(),
            ));
            rows.push((
                "MACsec".to_string(),
                if conn.mac_sec_capable { "✓ capable" } else { "no" }.to_string(),
            ));
            if !conn.encryption_mode.is_empty() {
                rows.push(("Encryption Mode".to_string(), conn.encryption_mode.clone()));
            }
            rows
        }
        DxConnectionDetailSection::VirtualInterfaces => {
            let mut rows = vec![(format!("Virtual Interfaces ({})", vifs.len()), String::new())];
            rows.push((String::new(), String::new()));
            if vifs.is_empty() {
                rows.push(("  No virtual interfaces".to_string(), String::new()));
            } else {
                rows.push((
                    format!("  {:<22}  {:<9}  {:<12}  {}", "ID", "Type", "State", "BGP"),
                    String::new(),
                ));
                for (id, kind, state, bgp) in vifs {
                    let bgp_label = if bgp.is_empty() { "—" } else { bgp.as_str() };
                    rows.push((
                        format!("  {:<22}  {:<9}  {:<12}  {}", id, kind, state, bgp_label),
                        String::new(),
                    ));
                }
            }
            rows
        }
        DxConnectionDetailSection::Tags => tag_rows(&conn.tags),
    }
}

/// Body lines for a DX virtual interface. BGP peers come from the VIF itself.
pub fn dx_vif_section_lines(
    vif: &DxVirtualInterface,
    section: DxVifDetailSection,
) -> Vec<(String, String)> {
    match section {
        DxVifDetailSection::Details => {
            let mut rows = vec![
                ("ID".to_string(), vif.id.clone()),
                ("Name".to_string(), vif.name.clone()),
                ("Type".to_string(), vif.kind.clone()),
                ("State".to_string(), vif.state.clone()),
                ("Connection".to_string(), vif.connection_id.clone()),
            ];
            if !vif.gateway_id.is_empty() {
                rows.push(("Gateway".to_string(), vif.gateway_id.clone()));
            }
            if let Some(v) = vif.vlan {
                rows.push(("VLAN".to_string(), v.to_string()));
            }
            if let Some(mtu) = vif.mtu {
                rows.push(("MTU".to_string(), mtu.to_string()));
            }
            if !vif.address_family.is_empty() {
                rows.push(("Address Family".to_string(), vif.address_family.clone()));
            }
            if !vif.amazon_address.is_empty() {
                rows.push(("Amazon Address".to_string(), vif.amazon_address.clone()));
            }
            if !vif.customer_address.is_empty() {
                rows.push(("Customer Address".to_string(), vif.customer_address.clone()));
            }
            rows
        }
        DxVifDetailSection::Bgp => {
            let status_label = if vif.bgp_status.is_empty() {
                "no peers".to_string()
            } else {
                vif.bgp_status.clone()
            };
            let mut rows = vec![
                ("BGP Status".to_string(), status_label),
                (
                    "BGP ASN".to_string(),
                    vif.bgp_asn.map(|a| a.to_string()).unwrap_or_default(),
                ),
                (String::new(), String::new()),
                (format!("Peers ({})", vif.peers.len()), String::new()),
                (String::new(), String::new()),
            ];
            if vif.peers.is_empty() {
                rows.push(("  No BGP peers".to_string(), String::new()));
            } else {
                for p in &vif.peers {
                    let title = format!("ASN {}  ·  {}", p.asn, p.address_family);
                    rows.push((title, String::new())); // group header
                    rows.push((
                        "Status".to_string(),
                        if p.status.is_empty() {
                            p.peer_state.clone()
                        } else {
                            format!("{} ({})", p.status, p.peer_state)
                        },
                    ));
                    if !p.amazon_address.is_empty() {
                        rows.push(("Amazon Address".to_string(), p.amazon_address.clone()));
                    }
                    if !p.customer_address.is_empty() {
                        rows.push(("Customer Address".to_string(), p.customer_address.clone()));
                    }
                    rows.push((
                        "Auth Key".to_string(),
                        if p.has_auth_key { "✓ set" } else { "none" }.to_string(),
                    ));
                    rows.push((String::new(), String::new()));
                }
            }
            rows
        }
        DxVifDetailSection::Tags => tag_rows(&vif.tags),
    }
}

// ── DX Gateway split pane ─────────────────────────────────────────────────────

pub(super) fn render_dx_gateway_split(app: &App, gw: &DxGateway, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 3, "");
    let mut block = theme::pane_block("DX Gateway", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let header = dx_header_lines(&gw.name, &gw.id, &gw.state, dx_state_color(&gw.state));
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
    render_section_tab_bar(app,
        chunks[2],
        frame,
        &descriptor_tabs(app, &crate::aws::services::direct_connect::DX_GATEWAY_SECTIONS),
    );
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

/// Body lines for a DX gateway. Associations (VGW/TGW) and attachments (VIFs)
/// are lazy per-gateway fetches; both id columns render as key-value rows so
/// the generic classifier makes `tgw-`/`dxvif-` ids Enter-jumpable.
pub fn dx_gateway_section_lines(
    gw: &DxGateway,
    section: DxGatewayDetailSection,
    assoc_state: Option<&Lazy<Vec<DxGwAssociation>>>,
    attach_state: Option<&Lazy<Vec<DxGwAttachment>>>,
) -> Vec<(String, String)> {
    match section {
        DxGatewayDetailSection::Overview => {
            let mut rows = vec![
                ("ID".to_string(), gw.id.clone()),
                ("Name".to_string(), gw.name.clone()),
                ("State".to_string(), gw.state.clone()),
                ("Owner Account".to_string(), gw.owner_account.clone()),
            ];
            if let Some(asn) = gw.amazon_side_asn {
                rows.push(("Amazon-side ASN".to_string(), asn.to_string()));
            }
            if let Some(err) = &gw.state_change_error {
                rows.push((String::new(), String::new()));
                rows.push((format!("  ⚠ {}", err), String::new()));
            }
            rows.push((String::new(), String::new()));
            rows.push((
                "  Traffic flows on the attached VIFs — `m` on a virtual".to_string(),
                String::new(),
            ));
            rows.push((
                "  interface charts its bps/pps in both directions.".to_string(),
                String::new(),
            ));
            rows
        }
        DxGatewayDetailSection::Associations => {
            let mut rows = vec![(String::new(), String::new())];
            match assoc_state {
                None | Some(Lazy::Loading) => {
                    rows.push(("  Loading associations…".to_string(), String::new()));
                }
                Some(Lazy::Error(e)) => return error_rows(e),
                Some(Lazy::Loaded(assocs)) => {
                    if assocs.is_empty() {
                        rows.push((
                            "  No gateway associations — nothing routes through this DX gateway"
                                .to_string(),
                            String::new(),
                        ));
                    } else {
                        for a in assocs {
                            let kind = match a.gateway_type.as_str() {
                                "transitGateway" => "Transit Gateway",
                                "virtualPrivateGateway" => "Virtual Private Gateway",
                                other => other,
                            };
                            rows.push((format!("{} · {}", kind, a.state), String::new()));
                            rows.push(("Gateway".to_string(), a.gateway_id.clone()));
                            if !a.association_id.is_empty() {
                                rows.push(("Association".to_string(), a.association_id.clone()));
                            }
                            if !a.gateway_region.is_empty() {
                                rows.push(("Region".to_string(), a.gateway_region.clone()));
                            }
                            if !a.gateway_owner.is_empty() {
                                rows.push(("Owner".to_string(), a.gateway_owner.clone()));
                            }
                            if !a.allowed_prefixes.is_empty() {
                                rows.push((
                                    "Allowed Prefixes".to_string(),
                                    a.allowed_prefixes.join(", "),
                                ));
                            }
                            if let Some(err) = &a.state_change_error {
                                rows.push((format!("  ⚠ {}", err), String::new()));
                            }
                            rows.push((String::new(), String::new()));
                        }
                    }
                }
            }
            rows
        }
        DxGatewayDetailSection::Attachments => {
            let mut rows = vec![(String::new(), String::new())];
            match attach_state {
                None | Some(Lazy::Loading) => {
                    rows.push(("  Loading attachments…".to_string(), String::new()));
                }
                Some(Lazy::Error(e)) => return error_rows(e),
                Some(Lazy::Loaded(atts)) => {
                    if atts.is_empty() {
                        rows.push((
                            "  No virtual interfaces attached".to_string(),
                            String::new(),
                        ));
                    } else {
                        for a in atts {
                            rows.push((
                                format!("{} · {}", a.attachment_type, a.state),
                                String::new(),
                            ));
                            rows.push(("Virtual Interface".to_string(), a.vif_id.clone()));
                            if !a.vif_region.is_empty() {
                                rows.push(("Region".to_string(), a.vif_region.clone()));
                            }
                            if !a.vif_owner.is_empty() {
                                rows.push(("Owner".to_string(), a.vif_owner.clone()));
                            }
                            if let Some(err) = &a.state_change_error {
                                rows.push((format!("  ⚠ {}", err), String::new()));
                            }
                            rows.push((String::new(), String::new()));
                        }
                    }
                }
            }
            rows
        }
    }
}

pub(super) fn render_dx_lag_split(
    app: &App,
    lag: &crate::aws::services::direct_connect::DxLag,
    area: Rect,
    frame: &mut Frame,
) {
    let name = if lag.name.is_empty() { &lag.id } else { &lag.name };
    let subtitle = format!("{} · {} connections", lag.bandwidth, lag.connections_count);
    render_simple_split(
        app,
        area,
        frame,
        "DX LAG",
        name,
        &subtitle,
        &descriptor_tabs(app, &crate::aws::services::direct_connect::DX_LAG_SECTIONS),
    );
}

pub fn dx_lag_section_lines(
    lag: &crate::aws::services::direct_connect::DxLag,
    section: DxLagDetailSection,
    conns: &[&crate::aws::services::direct_connect::DxConnection],
) -> Vec<(String, String)> {
    match section {
        DxLagDetailSection::Overview => {
            let mut rows = vec![
                (String::new(), String::new()),
                ("LAG ID".to_string(), lag.id.clone()),
                ("Name".to_string(), lag.name.clone()),
                ("State".to_string(), lag.state.clone()),
                ("Bandwidth".to_string(), lag.bandwidth.clone()),
                ("Location".to_string(), lag.location.clone()),
                ("Region".to_string(), lag.region.clone()),
                (
                    "Connections".to_string(),
                    lag.connections_count.to_string(),
                ),
                ("Minimum Links".to_string(), lag.minimum_links.to_string()),
            ];
            if lag.jumbo_capable {
                rows.push(("Jumbo Frames".to_string(), "✓ capable".to_string()));
            }
            rows
        }
        DxLagDetailSection::Connections => {
            let mut rows = vec![(String::new(), String::new())];
            if conns.is_empty() {
                rows.push((
                    "".to_string(),
                    "No connections from this LAG are in the loaded list".to_string(),
                ));
            }
            for c in conns {
                rows.push((
                    c.id.clone(),
                    format!("{} · {} · {}", c.name, c.state, c.bandwidth),
                ));
            }
            rows
        }
        DxLagDetailSection::Tags => tag_rows(&lag.tags),
    }
}
