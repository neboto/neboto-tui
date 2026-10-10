use super::*;

// ── Kinesis stream split pane ──────────────────────────────────────────────────

pub(super) fn render_kinesis_split(app: &App, st: &KinesisStream, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 3, "");
    let mut block = theme::pane_block("Kinesis Stream", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let state_color = match st.status.as_str() {
        "ACTIVE" => theme::success(),
        "CREATING" | "UPDATING" => theme::warning(),
        "DELETING" => theme::text_dim(),
        _ => theme::text_dim(),
    };
    let header = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                st.name.clone(),
                Style::default().fg(crate::ui::theme::text_primary()).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                format!("{} · {} shard(s)", st.mode, st.open_shard_count),
                Style::default().fg(theme::text_dim()),
            ),
            Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
            Span::styled(st.status.clone(), Style::default().fg(state_color)),
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
        &descriptor_tabs(app, &crate::aws::services::kinesis::KINESIS_SECTIONS),
    );
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub fn kinesis_section_lines(
    st: &KinesisStream,
    section: KinesisDetailSection,
    consumers: Option<&Lazy<Vec<crate::aws::services::kinesis::KinesisConsumer>>>,
    tags: Option<&Lazy<Vec<(String, String)>>>,
) -> Vec<(String, String)> {
    match section {
        KinesisDetailSection::Details => {
            let mut rows = vec![
                ("Name".to_string(), st.name.clone()),
                ("Status".to_string(), st.status.clone()),
                ("Capacity Mode".to_string(), st.mode.clone()),
                ("Open Shards".to_string(), st.open_shard_count.to_string()),
                ("Retention".to_string(), st.retention_label()),
            ];
            if let Some(n) = st.consumer_count {
                rows.push(("Fan-Out Consumers".to_string(), n.to_string()));
            }

            rows.push((String::new(), String::new()));
            rows.push(("Encryption".to_string(), String::new())); // group header
            rows.push((
                "  At Rest".to_string(),
                if st.is_encrypted() {
                    "✓ KMS".to_string()
                } else {
                    "✗ None".to_string()
                },
            ));
            if let Some(kms) = &st.kms_key_id {
                rows.push(("  KMS Key".to_string(), kms.clone())); // jumpable
            }

            if !st.enhanced_metrics.is_empty() {
                rows.push((String::new(), String::new()));
                rows.push(("Enhanced (Shard-Level) Metrics".to_string(), String::new())); // group header
                for m in &st.enhanced_metrics {
                    rows.push(("  ".to_string(), m.clone()));
                }
            }

            rows.push((String::new(), String::new()));
            rows.push(("Other".to_string(), String::new())); // group header
            if let Some(c) = &st.created {
                rows.push(("  Created".to_string(), c.clone()));
            }
            if !st.arn.is_empty() {
                rows.push(("  ARN".to_string(), st.arn.clone()));
            }
            rows
        }
        KinesisDetailSection::Consumers => match consumers {
            None | Some(Lazy::Loading) => {
                vec![("".to_string(), "Loading consumers…".to_string())]
            }
            Some(Lazy::Error(e)) => error_rows(e),
            Some(Lazy::Loaded(list)) => {
                if list.is_empty() {
                    return vec![(
                        "".to_string(),
                        "No enhanced fan-out consumers registered".to_string(),
                    )];
                }
                let mut rows = Vec::new();
                for c in list {
                    rows.push((c.name.clone(), String::new())); // group header
                    rows.push(("  Status".to_string(), c.status.clone()));
                    if let Some(created) = &c.created {
                        rows.push(("  Created".to_string(), created.clone()));
                    }
                    rows.push(("  ARN".to_string(), c.arn.clone()));
                    rows.push((String::new(), String::new()));
                }
                rows
            }
        },
        KinesisDetailSection::Tags => match tags {
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

// ── Firehose delivery stream split pane ────────────────────────────────────────

pub(super) fn render_firehose_split(app: &App, st: &FirehoseStream, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 4, "");
    let mut block = theme::pane_block("Firehose Delivery Stream", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let state_color = match st.status.as_str() {
        "ACTIVE" => theme::success(),
        "CREATING" => theme::warning(),
        s if s.ends_with("_FAILED") => theme::error(),
        _ => theme::text_dim(),
    };
    let header = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                st.name.clone(),
                Style::default().fg(crate::ui::theme::text_primary()).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                format!("→ {}", st.dest.kind),
                Style::default().fg(theme::text_dim()),
            ),
            Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
            Span::styled(st.status.clone(), Style::default().fg(state_color)),
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
        &descriptor_tabs(app, &crate::aws::services::kinesis::FIREHOSE_SECTIONS),
    );
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub fn firehose_section_lines(
    st: &FirehoseStream,
    section: FirehoseDetailSection,
    tags: Option<&Lazy<Vec<(String, String)>>>,
) -> Vec<(String, String)> {
    match section {
        FirehoseDetailSection::Overview => {
            let mut rows = vec![
                ("Name".to_string(), st.name.clone()),
                ("Status".to_string(), st.status.clone()),
                ("Type".to_string(), st.stream_type.clone()),
            ];
            if let Some(src) = &st.source {
                rows.push(("Source".to_string(), src.clone())); // jumpable (Kinesis/MSK ARN)
            }

            if let Some((kind, details)) = &st.failure {
                rows.push((String::new(), String::new()));
                rows.push(("Failure".to_string(), String::new())); // group header
                rows.push(("  Type".to_string(), format!("✗ {}", kind)));
                rows.push(("  Details".to_string(), details.clone()));
            }

            rows.push((String::new(), String::new()));
            rows.push(("Encryption".to_string(), String::new())); // group header
            rows.push((
                "  At Rest".to_string(),
                match &st.encryption {
                    Some(kt) => format!("✓ {}", kt),
                    None => "✗ Not enabled".to_string(),
                },
            ));
            if let Some(k) = &st.kms_key_arn {
                rows.push(("  KMS Key".to_string(), k.clone())); // jumpable
            }

            rows.push((String::new(), String::new()));
            rows.push(("Error Logging".to_string(), String::new())); // group header
            match st.error_log_group() {
                Some(g) => {
                    rows.push(("  CloudWatch Group".to_string(), g.to_string()));
                    rows.push(("  Tail".to_string(), "t  live-tail error logs".to_string()));
                }
                None => rows.push(("  CloudWatch".to_string(), "✗ Not enabled".to_string())),
            }

            rows.push((String::new(), String::new()));
            rows.push(("Other".to_string(), String::new())); // group header
            if let Some(c) = &st.created {
                rows.push(("  Created".to_string(), c.clone()));
            }
            rows.push(("  Version".to_string(), st.version_id.clone()));
            if !st.arn.is_empty() {
                rows.push(("  ARN".to_string(), st.arn.clone()));
            }
            rows
        }
        FirehoseDetailSection::Destination => {
            let d = &st.dest;
            let mut rows = vec![("Destination".to_string(), d.kind.clone())];
            if let Some(arn) = &d.arn {
                rows.push(("Target ARN".to_string(), arn.clone())); // jumpable (bucket/domain)
            }
            for (k, v) in &d.extra {
                rows.push((k.clone(), v.clone()));
            }

            rows.push((String::new(), String::new()));
            rows.push(("Delivery".to_string(), String::new())); // group header
            rows.push((
                "  Buffering".to_string(),
                st.buffering_label().unwrap_or_else(|| "—".to_string()),
            ));
            if let Some(c) = &d.compression {
                rows.push(("  Compression".to_string(), c.clone()));
            }
            if let Some(p) = &d.prefix {
                rows.push(("  Prefix".to_string(), p.clone()));
            }
            if let Some(e) = &d.error_prefix {
                rows.push(("  Error Output Prefix".to_string(), e.clone()));
            }

            // Extended-S3 features (only shown when present).
            if d.format_conversion.is_some() || d.dynamic_partitioning || d.s3_backup.is_some() {
                rows.push((String::new(), String::new()));
                rows.push(("S3 Features".to_string(), String::new())); // group header
                if let Some(fc) = &d.format_conversion {
                    rows.push(("  Format Conversion".to_string(), format!("✓ {}", fc)));
                }
                if d.dynamic_partitioning {
                    rows.push(("  Dynamic Partitioning".to_string(), "✓ Enabled".to_string()));
                }
                if let Some(b) = &d.s3_backup {
                    rows.push(("  Source-Record Backup".to_string(), format!("✓ {}", b)));
                }
            }
            rows
        }
        FirehoseDetailSection::Processing => {
            let procs = &st.dest.processors;
            if procs.is_empty() {
                return vec![(
                    "".to_string(),
                    "No record processing configured".to_string(),
                )];
            }
            let mut rows = Vec::new();
            for p in procs {
                rows.push((p.kind.clone(), String::new())); // group header
                if let Some(arn) = &p.lambda_arn {
                    rows.push(("  Lambda".to_string(), arn.clone())); // jumpable
                }
                for (k, v) in &p.params {
                    rows.push((format!("  {}", k), v.clone()));
                }
                rows.push((String::new(), String::new()));
            }
            rows
        }
        FirehoseDetailSection::Tags => match tags {
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
