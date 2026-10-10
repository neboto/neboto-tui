use super::*;

// ── Transfer Family server split pane ──────────────────────────────────────────

pub(super) fn render_transfer_server_split(
    app: &App,
    srv: &TransferServer,
    area: Rect,
    frame: &mut Frame,
) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 3, "");
    let mut block = theme::pane_block("Transfer Server", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let state_color = match srv.state.as_str() {
        "ONLINE" => theme::success(),
        "OFFLINE" => theme::text_dim(),
        "STARTING" | "STOPPING" => theme::warning(),
        "START_FAILED" | "STOP_FAILED" => theme::error(),
        _ => theme::text_dim(),
    };
    let header = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                srv.display_name().to_string(),
                Style::default().fg(crate::ui::theme::text_primary()).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(srv.server_id.clone(), Style::default().fg(theme::text_dim())),
            Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
            Span::styled(srv.protocols.join("/"), Style::default().fg(theme::text_dim())),
            Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
            Span::styled(srv.state.clone(), Style::default().fg(state_color)),
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
        &descriptor_tabs(app, &crate::aws::services::transfer::TRANSFER_SERVER_SECTIONS),
    );
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub fn transfer_server_section_lines(
    srv: &TransferServer,
    section: TransferServerDetailSection,
    users: Option<&Lazy<Vec<crate::aws::services::transfer::TransferUser>>>,
) -> Vec<(String, String)> {
    match section {
        TransferServerDetailSection::Details => {
            let mut rows = vec![
                ("Server ID".to_string(), srv.server_id.clone()),
                ("State".to_string(), srv.state.clone()),
                ("Protocols".to_string(), srv.protocols.join(", ")),
                ("Domain".to_string(), srv.domain.clone()),
                (
                    "Identity Provider".to_string(),
                    srv.identity_provider_type.clone(),
                ),
            ];
            rows.push((String::new(), String::new()));
            rows.push(("Endpoint".to_string(), String::new())); // group header
            rows.push(("  Type".to_string(), srv.endpoint_type.clone()));
            if srv.endpoint_type == "VPC" {
                // VPC-hosted endpoint carries vpc / subnets / EIP allocations.
                if let Some(vpc) = &srv.endpoint_vpc_id {
                    rows.push(("  VPC".to_string(), vpc.clone())); // jumpable (vpc-)
                }
                for sn in &srv.endpoint_subnet_ids {
                    rows.push(("  Subnet".to_string(), sn.clone())); // jumpable (subnet-)
                }
                for a in &srv.endpoint_address_allocation_ids {
                    rows.push(("  Address Allocation".to_string(), a.clone()));
                }
            } else if srv.endpoint_type == "VPC_ENDPOINT" {
                if let Some(ve) = &srv.endpoint_vpc_endpoint_id {
                    rows.push(("  VPC Endpoint".to_string(), ve.clone()));
                }
            }
            rows.push((String::new(), String::new()));
            rows.push(("Security".to_string(), String::new())); // group header
            rows.push((
                "  Security Policy".to_string(),
                srv.security_policy_name
                    .clone()
                    .unwrap_or_else(|| "Default".to_string()),
            ));
            if let Some(role) = &srv.logging_role {
                rows.push(("  Logging Role".to_string(), role.clone())); // jumpable (IAM ARN)
            }
            if let Some(fp) = &srv.host_key_fingerprint {
                rows.push(("  Host Key Fingerprint".to_string(), fp.clone()));
            }
            if !srv.structured_log_destinations.is_empty() {
                rows.push((String::new(), String::new()));
                rows.push(("Logging".to_string(), String::new())); // group header
                for dest in &srv.structured_log_destinations {
                    // CloudWatch log group ARN — jumpable to Logs.
                    rows.push(("  Log Destination".to_string(), dest.clone()));
                }
            }
            rows.push((String::new(), String::new()));
            rows.push(("Other".to_string(), String::new())); // group header
            rows.push(("  Users".to_string(), srv.user_count.to_string()));
            rows.push(("  ARN".to_string(), srv.arn.clone()));
            rows
        }
        TransferServerDetailSection::Users => match users {
            None | Some(Lazy::Loading) => {
                vec![("".to_string(), "Loading users…".to_string())]
            }
            Some(Lazy::Error(e)) => error_rows(e),
            Some(Lazy::Loaded(list)) => {
                if list.is_empty() {
                    return vec![(
                        "".to_string(),
                        "No users — AS2 servers use agreements/connectors instead".to_string(),
                    )];
                }
                let mut rows = Vec::new();
                for u in list {
                    rows.push((u.user_name.clone(), String::new())); // group header
                    if let Some(role) = &u.role {
                        rows.push(("  Role".to_string(), role.clone())); // jumpable (IAM ARN)
                    }
                    if let Some(t) = &u.home_directory_type {
                        rows.push(("  Home Dir Type".to_string(), t.clone()));
                    }
                    if let Some(h) = &u.home_directory {
                        if !h.is_empty() {
                            rows.push(("  Home Directory".to_string(), h.clone()));
                        }
                    }
                    if u.home_directory_mappings > 0 {
                        rows.push((
                            "  Logical Mappings".to_string(),
                            u.home_directory_mappings.to_string(),
                        ));
                    }
                    rows.push((
                        "  SSH Public Keys".to_string(),
                        u.ssh_public_key_count.to_string(),
                    ));
                    rows.push((
                        "  Session Policy".to_string(),
                        if u.policy.is_some() {
                            "✓ Set".to_string()
                        } else {
                            "—".to_string()
                        },
                    ));
                    rows.push((String::new(), String::new()));
                }
                rows
            }
        },
        TransferServerDetailSection::Tags => tag_rows(&srv.tags),
    }
}
