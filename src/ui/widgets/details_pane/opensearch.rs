use super::*;

// ── OpenSearch domain split pane ─────────────────────────────────────────────

pub(super) fn render_opensearch_split(app: &App, d: &OpenSearchDomain, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 5, "");
    let mut block = theme::pane_block("OpenSearch Domain", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let status = d.status_label();
    let state_color = match status.as_str() {
        "Active" => theme::success(),
        "Isolated" => theme::error(),
        "Creating" | "Deleting" => theme::text_dim(),
        _ => theme::warning(), // Modifying / Upgrading / Updating / Processing
    };
    let header = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                d.domain_name.clone(),
                Style::default().fg(crate::ui::theme::text_primary()).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(d.engine_version.clone(), Style::default().fg(theme::text_dim())),
            Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
            Span::styled(status, Style::default().fg(state_color)),
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
        &descriptor_tabs(app, &crate::aws::services::opensearch::OPENSEARCH_SECTIONS),
    );
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub fn opensearch_section_lines(
    d: &OpenSearchDomain,
    section: OpenSearchDetailSection,
    tags: Option<&Lazy<Vec<(String, String)>>>,
) -> Vec<(String, String)> {
    let yn = |b: bool| {
        if b {
            "✓ Enabled".to_string()
        } else {
            "✗ Disabled".to_string()
        }
    };
    match section {
        OpenSearchDetailSection::Details => {
            let mut rows = vec![
                ("Name".to_string(), d.domain_name.clone()),
                ("Domain ID".to_string(), d.domain_id.clone()),
                ("Engine".to_string(), d.engine_version.clone()),
                ("Status".to_string(), d.status_label()),
            ];
            if let Some(ep) = &d.endpoint {
                rows.push(("Endpoint".to_string(), ep.clone()));
            }
            rows.push((String::new(), String::new()));
            rows.push(("Operations".to_string(), String::new())); // group header
            if let Some(at) = &d.auto_tune {
                rows.push(("  Auto-Tune".to_string(), at.clone()));
            }
            if let Some(h) = d.snapshot_hour {
                rows.push(("  Auto-Snapshot Hour".to_string(), format!("{:02}:00 UTC", h)));
            }
            rows.push((
                "  Change In Progress".to_string(),
                if d.processing { "yes".to_string() } else { "no".to_string() },
            ));
            if !d.arn.is_empty() {
                rows.push(("  ARN".to_string(), d.arn.clone()));
            }
            rows
        }
        OpenSearchDetailSection::Cluster => {
            let mut rows = vec![
                ("Data Nodes".to_string(), String::new()), // group header
                ("  Instance Type".to_string(), d.instance_type.clone()),
                ("  Instance Count".to_string(), d.instance_count.to_string()),
            ];
            rows.push((
                "  Zone Awareness".to_string(),
                if d.zone_awareness_enabled {
                    match d.availability_zone_count {
                        Some(n) => format!("enabled ({} AZ)", n),
                        None => "enabled".to_string(),
                    }
                } else {
                    "disabled".to_string()
                },
            ));
            if d.multi_az_standby {
                rows.push(("  Multi-AZ w/ Standby".to_string(), "✓ Enabled".to_string()));
            }

            rows.push((String::new(), String::new()));
            rows.push(("Dedicated Master".to_string(), String::new())); // group header
            if d.dedicated_master_enabled {
                if let Some(t) = &d.dedicated_master_type {
                    rows.push(("  Type".to_string(), t.clone()));
                }
                if let Some(c) = d.dedicated_master_count {
                    rows.push(("  Count".to_string(), c.to_string()));
                }
            } else {
                rows.push(("  Enabled".to_string(), "✗ Disabled".to_string()));
            }

            if d.warm_enabled || d.cold_storage_enabled {
                rows.push((String::new(), String::new()));
                rows.push(("Tiered Storage".to_string(), String::new())); // group header
                if d.warm_enabled {
                    let wt = d.warm_type.clone().unwrap_or_default();
                    let wc = d.warm_count.map(|c| c.to_string()).unwrap_or_default();
                    rows.push(("  UltraWarm".to_string(), format!("{} × {}", wc, wt)));
                }
                if d.cold_storage_enabled {
                    rows.push(("  Cold Storage".to_string(), "✓ Enabled".to_string()));
                }
            }

            rows.push((String::new(), String::new()));
            rows.push(("Storage (EBS)".to_string(), String::new())); // group header
            if d.ebs_enabled {
                if let Some(t) = &d.volume_type {
                    rows.push(("  Volume Type".to_string(), t.clone()));
                }
                if let Some(s) = d.volume_size {
                    rows.push(("  Volume Size".to_string(), format!("{} GiB", s)));
                }
                if let Some(i) = d.volume_iops {
                    rows.push(("  IOPS".to_string(), i.to_string()));
                }
                if let Some(t) = d.volume_throughput {
                    rows.push(("  Throughput".to_string(), format!("{} MiB/s", t)));
                }
            } else {
                rows.push(("  EBS".to_string(), "instance storage".to_string()));
            }
            rows
        }
        OpenSearchDetailSection::Network => {
            let mut rows = Vec::new();
            if let Some(ep) = &d.endpoint {
                rows.push(("Endpoint".to_string(), ep.clone()));
            }
            for ve in &d.vpc_endpoints {
                rows.push(("VPC Endpoint".to_string(), ve.clone()));
            }
            if let Some(ce) = &d.custom_endpoint {
                rows.push(("Custom Endpoint".to_string(), ce.clone()));
            }
            if let Some(e) = d.enforce_https {
                rows.push(("Enforce HTTPS".to_string(), yn(e)));
            }
            if let Some(t) = &d.tls_policy {
                rows.push(("TLS Policy".to_string(), t.clone()));
            }

            if d.vpc_id.is_some() || !d.subnet_ids.is_empty() {
                rows.push((String::new(), String::new()));
                rows.push(("VPC".to_string(), String::new())); // group header
                if let Some(v) = &d.vpc_id {
                    rows.push(("  VPC".to_string(), v.clone())); // vpc- jumpable
                }
                for sn in &d.subnet_ids {
                    rows.push(("  Subnet".to_string(), sn.clone())); // subnet- jumpable
                }
                for sg in &d.security_group_ids {
                    rows.push(("  Security Group".to_string(), sg.clone())); // sg- jumpable
                }
                for az in &d.availability_zones {
                    rows.push(("  Availability Zone".to_string(), az.clone()));
                }
            } else {
                rows.push((String::new(), String::new()));
                rows.push(("Access".to_string(), "Public endpoint".to_string()));
            }
            rows
        }
        OpenSearchDetailSection::Security => {
            let mut rows = vec![
                ("Encryption".to_string(), String::new()), // group header
                ("  At Rest".to_string(), yn(d.encryption_at_rest)),
            ];
            if let Some(kms) = &d.kms_key_id {
                rows.push(("  KMS Key".to_string(), kms.clone())); // arn jumpable
            }
            rows.push(("  Node-to-Node".to_string(), yn(d.node_to_node_encryption)));

            rows.push((String::new(), String::new()));
            rows.push(("Access Control".to_string(), String::new())); // group header
            rows.push(("  Fine-Grained Access".to_string(), yn(d.fine_grained_access)));
            if d.fine_grained_access {
                rows.push((
                    "  Internal User DB".to_string(),
                    yn(d.internal_user_db),
                ));
            }
            rows.push(("  Cognito Auth".to_string(), yn(d.cognito_enabled)));
            if let Some(up) = &d.cognito_user_pool_id {
                rows.push(("  Cognito User Pool".to_string(), up.clone()));
            }

            rows.push((String::new(), String::new()));
            rows.push(("Resource Policy".to_string(), String::new())); // group header
            if d.access_policies.is_some() {
                rows.push((
                    "  Access Policy".to_string(),
                    "set — press e to view".to_string(),
                ));
            } else {
                rows.push(("  Access Policy".to_string(), "none".to_string()));
            }
            rows
        }
        OpenSearchDetailSection::Tags => match tags {
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
