use super::*;

// ── MSK cluster split pane ───────────────────────────────────────────────────

pub(super) fn render_msk_split(app: &App, c: &MskCluster, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 5, "");
    let mut block = theme::pane_block("MSK Cluster", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let state_color = match c.state.as_str() {
        "ACTIVE" => theme::success(),
        "FAILED" => theme::error(),
        "CREATING" | "DELETING" => theme::text_dim(),
        _ => theme::warning(), // HEALING / MAINTENANCE / REBOOTING_BROKER / UPDATING
    };
    let subtitle = match &c.kafka_version {
        Some(v) => format!("{}  ·  Kafka {}", c.cluster_type, v),
        None => c.cluster_type.clone(),
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
            Span::styled(subtitle, Style::default().fg(theme::text_dim())),
            Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
            Span::styled(c.state.clone(), Style::default().fg(state_color)),
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
        &descriptor_tabs(app, &crate::aws::services::msk::MSK_SECTIONS),
    );
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub fn msk_section_lines(
    c: &MskCluster,
    section: MskDetailSection,
    config: Option<&Lazy<crate::aws::services::msk::MskConfigRevision>>,
) -> Vec<(String, String)> {
    let yn = |b: bool| {
        if b {
            "✓ Yes".to_string()
        } else {
            "✗ No".to_string()
        }
    };
    match section {
        MskDetailSection::Overview => {
            let mut rows = vec![
                ("Name".to_string(), c.name.clone()),
                ("Type".to_string(), c.cluster_type.clone()),
                ("State".to_string(), c.state.clone()),
            ];
            if let Some(m) = &c.state_message {
                rows.push(("  Message".to_string(), m.clone()));
            }
            if let Some(v) = &c.kafka_version {
                rows.push(("Kafka Version".to_string(), v.clone()));
            }
            if let Some(t) = &c.created {
                rows.push(("Created".to_string(), t.clone()));
            }
            rows.push(("ARN".to_string(), c.arn.clone()));

            if c.cluster_type == "PROVISIONED" {
                rows.push((String::new(), String::new()));
                rows.push(("Brokers".to_string(), String::new())); // group header
                if let Some(n) = c.broker_count {
                    rows.push(("  Broker Nodes".to_string(), n.to_string()));
                }
                if let Some(it) = &c.instance_type {
                    rows.push(("  Instance Type".to_string(), it.clone()));
                }
                if let Some(sz) = c.volume_size {
                    rows.push(("  EBS Storage".to_string(), format!("{} GiB / broker", sz)));
                }
                if let Some(sm) = &c.storage_mode {
                    rows.push(("  Storage Mode".to_string(), sm.clone()));
                }
                if let Some(zk) = &c.zookeeper {
                    rows.push(("  ZooKeeper".to_string(), zk.clone()));
                }
            }

            rows.push((String::new(), String::new()));
            rows.push(("Authentication".to_string(), String::new())); // group header
            rows.push(("  IAM".to_string(), yn(c.auth_iam)));
            rows.push(("  SASL/SCRAM".to_string(), yn(c.auth_scram)));
            rows.push(("  TLS (mutual)".to_string(), yn(c.auth_tls)));
            rows.push(("  Unauthenticated".to_string(), yn(c.auth_unauthenticated)));

            rows.push((String::new(), String::new()));
            rows.push(("Encryption".to_string(), String::new())); // group header
            if let Some(cb) = &c.encryption_in_transit_client {
                rows.push(("  In Transit (client)".to_string(), cb.clone()));
            }
            if let Some(ic) = c.encryption_in_cluster {
                rows.push(("  In Cluster".to_string(), yn(ic)));
            }
            rows.push((
                "  At Rest".to_string(),
                match &c.encryption_at_rest_kms {
                    Some(_) => "✓ KMS".to_string(),
                    None => "✗ —".to_string(),
                },
            ));
            if let Some(k) = &c.encryption_at_rest_kms {
                // Jumpable KMS key (ARN or key id).
                rows.push(("  KMS Key".to_string(), k.clone()));
            }
            rows
        }
        MskDetailSection::Networking => {
            let mut rows = Vec::new();
            if let Some(pa) = &c.public_access {
                rows.push(("Public Access".to_string(), pa.clone()));
            }
            if let Some(az) = &c.az_distribution {
                rows.push(("Broker AZ Distribution".to_string(), az.clone()));
            }
            if !rows.is_empty() {
                rows.push((String::new(), String::new()));
            }
            rows.push(("Subnets".to_string(), String::new())); // group header
            if c.subnets.is_empty() {
                rows.push(("  —".to_string(), String::new()));
            } else {
                for s in &c.subnets {
                    rows.push(("Subnet".to_string(), s.clone()));
                }
            }
            rows.push((String::new(), String::new()));
            rows.push(("Security Groups".to_string(), String::new())); // group header
            if c.security_groups.is_empty() {
                rows.push(("  —".to_string(), String::new()));
            } else {
                for sg in &c.security_groups {
                    rows.push(("Security Group".to_string(), sg.clone()));
                }
            }
            if !c.zone_ids.is_empty() {
                rows.push((String::new(), String::new()));
                rows.push(("Zone IDs".to_string(), String::new())); // group header
                for z in &c.zone_ids {
                    rows.push((format!("  {}", z), String::new()));
                }
            }
            rows
        }
        MskDetailSection::Monitoring => {
            let mut rows = vec![(
                "Enhanced Monitoring".to_string(),
                c.enhanced_monitoring
                    .clone()
                    .unwrap_or_else(|| "—".to_string()),
            )];
            rows.push((String::new(), String::new()));
            rows.push(("Open Monitoring (Prometheus)".to_string(), String::new())); // group header
            rows.push(("  JMX Exporter".to_string(), yn(c.prometheus_jmx)));
            rows.push(("  Node Exporter".to_string(), yn(c.prometheus_node)));
            rows.push((String::new(), String::new()));
            rows.push((
                "  Press m for AWS/Kafka metrics (topics, throughput, disk).".to_string(),
                String::new(),
            ));
            rows
        }
        MskDetailSection::Config => {
            let mut rows = vec![(String::new(), String::new())];
            let Some(_) = &c.config_arn else {
                rows.push((
                    "  No custom configuration (using MSK defaults).".to_string(),
                    String::new(),
                ));
                return rows;
            };
            match config {
                None | Some(Lazy::Loading) => {
                    rows.push(("  Loading…".to_string(), String::new()));
                }
                Some(Lazy::Error(e)) => {
                    rows.extend(error_rows(e));
                }
                Some(Lazy::Loaded(rev)) => {
                    rows.pop(); // drop the leading blank; use a real header
                    rows.push(("Configuration".to_string(), String::new())); // group header
                    rows.push(("Revision".to_string(), rev.revision.to_string()));
                    if let Some(t) = &rev.created {
                        rows.push(("Created".to_string(), t.clone()));
                    }
                    if let Some(d) = &rev.description {
                        rows.push(("Description".to_string(), d.clone()));
                    }
                    rows.push((String::new(), String::new()));
                    rows.push(("server.properties".to_string(), String::new())); // group header
                    if rev.properties.is_empty() {
                        rows.push(("  (empty)".to_string(), String::new()));
                    } else {
                        for line in &rev.properties {
                            rows.push((format!("  {}", line), String::new()));
                        }
                    }
                }
            }
            rows
        }
        MskDetailSection::Tags => tag_rows(&c.tags),
    }
}
