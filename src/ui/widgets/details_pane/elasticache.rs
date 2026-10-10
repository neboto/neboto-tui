use super::*;

// ── ElastiCache cluster split pane ─────────────────────────────────────────────

pub(super) fn render_elasticache_split(
    app: &App,
    c: &ElastiCacheCluster,
    area: Rect,
    frame: &mut Frame,
) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 4, "");
    let mut block = theme::pane_block("ElastiCache", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let state_color = match c.status.as_str() {
        "available" => theme::success(),
        "creating" | "modifying" | "snapshotting" => theme::warning(),
        "create-failed" | "incompatible-network" | "restore-failed" => theme::error(),
        _ => theme::text_dim(),
    };
    let header = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                c.name.clone(),
                Style::default().fg(crate::ui::theme::text_primary()).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(c.engine_summary(), Style::default().fg(theme::text_dim())),
            Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
            Span::styled(c.status.clone(), Style::default().fg(state_color)),
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
        &descriptor_tabs(app, &crate::aws::services::elasticache::ELASTICACHE_SECTIONS),
    );
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub fn elasticache_section_lines(
    c: &ElastiCacheCluster,
    section: ElastiCacheDetailSection,
    tags: Option<&Lazy<Vec<(String, String)>>>,
) -> Vec<(String, String)> {
    match section {
        ElastiCacheDetailSection::Details => {
            let mut rows = vec![
                ("Name".to_string(), c.name.clone()),
                ("Cache ID".to_string(), c.id.clone()),
                (
                    "Type".to_string(),
                    match c.kind {
                        CacheKind::ReplicationGroup => "Replication Group".to_string(),
                        CacheKind::CacheCluster => "Cache Cluster".to_string(),
                    },
                ),
                ("Engine".to_string(), c.engine_summary()),
                ("Status".to_string(), c.status.clone()),
                ("Node Type".to_string(), c.node_type.clone()),
                ("Nodes".to_string(), c.num_nodes.to_string()),
            ];
            if let Some(d) = &c.description {
                rows.push(("Description".to_string(), d.clone()));
            }

            rows.push((String::new(), String::new()));
            rows.push(("Endpoints".to_string(), String::new())); // group header
            rows.push((
                "  Primary".to_string(),
                c.endpoint.clone().unwrap_or_else(|| "—".to_string()),
            ));
            if let Some(r) = &c.reader_endpoint {
                rows.push(("  Reader".to_string(), r.clone()));
            }

            if c.kind == CacheKind::ReplicationGroup {
                rows.push((String::new(), String::new()));
                rows.push(("Availability".to_string(), String::new())); // group header
                rows.push((
                    "  Cluster Mode".to_string(),
                    if c.cluster_mode_enabled {
                        "enabled".to_string()
                    } else {
                        "disabled".to_string()
                    },
                ));
                if let Some(m) = &c.multi_az {
                    rows.push(("  Multi-AZ".to_string(), m.clone()));
                }
                if let Some(f) = &c.automatic_failover {
                    rows.push(("  Automatic Failover".to_string(), f.clone()));
                }
            }

            rows.push((String::new(), String::new()));
            rows.push(("Security".to_string(), String::new())); // group header
            rows.push((
                "  Encryption at Rest".to_string(),
                if c.at_rest_encryption {
                    "✓ Enabled".to_string()
                } else {
                    "✗ Disabled".to_string()
                },
            ));
            rows.push((
                "  Encryption in Transit".to_string(),
                if c.transit_encryption {
                    "✓ Enabled".to_string()
                } else {
                    "✗ Disabled".to_string()
                },
            ));
            rows.push((
                "  Auth Token (AUTH)".to_string(),
                if c.auth_token_enabled {
                    "✓ Enabled".to_string()
                } else {
                    "✗ Disabled".to_string()
                },
            ));

            rows.push((String::new(), String::new()));
            rows.push(("Maintenance".to_string(), String::new())); // group header
            rows.push((
                "  Snapshot Retention".to_string(),
                match c.snapshot_retention {
                    Some(0) | None => "disabled".to_string(),
                    Some(d) => format!("{} day(s)", d),
                },
            ));
            if let Some(w) = &c.maintenance_window {
                rows.push(("  Window".to_string(), w.clone()));
            }
            if let Some(created) = &c.created {
                rows.push(("  Created".to_string(), created.clone()));
            }
            if !c.arn.is_empty() {
                rows.push(("  ARN".to_string(), c.arn.clone()));
            }
            rows
        }
        ElastiCacheDetailSection::Nodes => {
            if c.nodes.is_empty() {
                return vec![("".to_string(), "No node detail available".to_string())];
            }
            let mut rows = Vec::new();
            for n in &c.nodes {
                let label = match &n.role {
                    Some(role) if !role.is_empty() => format!("{} ({})", n.cache_cluster_id, role),
                    _ => n.cache_cluster_id.clone(),
                };
                rows.push((label, String::new())); // group header
                if !n.node_id.is_empty() {
                    rows.push(("  Node ID".to_string(), n.node_id.clone()));
                }
                if !n.status.is_empty() {
                    rows.push(("  Status".to_string(), n.status.clone()));
                }
                if let Some(az) = &n.az {
                    rows.push(("  Availability Zone".to_string(), az.clone()));
                }
                if let Some(ep) = &n.endpoint {
                    rows.push(("  Endpoint".to_string(), ep.clone()));
                }
                rows.push((String::new(), String::new()));
            }
            rows
        }
        ElastiCacheDetailSection::Network => {
            let mut rows = Vec::new();
            rows.push((
                "Subnet Group".to_string(),
                c.subnet_group.clone().unwrap_or_else(|| "—".to_string()),
            ));
            rows.push((
                "Parameter Group".to_string(),
                c.parameter_group.clone().unwrap_or_else(|| "—".to_string()),
            ));
            if let Some(kms) = &c.kms_key_id {
                rows.push(("KMS Key".to_string(), kms.clone())); // arn → jumpable
            }
            rows.push((String::new(), String::new()));
            rows.push(("Security Groups".to_string(), String::new())); // group header
            if c.security_group_ids.is_empty() {
                rows.push(("  ".to_string(), "None".to_string()));
            } else {
                for sg in &c.security_group_ids {
                    rows.push(("  Security Group".to_string(), sg.clone())); // sg- → jumpable
                }
            }
            rows
        }
        ElastiCacheDetailSection::Tags => match tags {
            None | Some(Lazy::Loading) => {
                vec![("".to_string(), "Loading tags…".to_string())]
            }
            Some(Lazy::Error(e)) => error_rows(e),
            Some(Lazy::Loaded(list)) => {
                if list.is_empty() {
                    return vec![("".to_string(), "No tags".to_string())];
                }
                list.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
            }
        },
    }
}
