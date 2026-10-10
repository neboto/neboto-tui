use super::*;

// ── CloudFront Distribution split pane ─────────────────────────────────────────

pub(super) fn render_cf_distribution_split(
    app: &App,
    dist: &CfDistribution,
    area: Rect,
    frame: &mut Frame,
) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 7, "");

    let mut block = theme::pane_block("CloudFront Distribution", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let header_lines = build_cf_distribution_header_lines(dist);
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
    render_cf_section_tabs(app, chunks[2], frame);
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub(super) fn build_cf_distribution_header_lines(dist: &CfDistribution) -> Vec<Line<'static>> {
    let mut lines = vec![Line::raw("")];

    // Name (first alias or domain)
    let display_name = if let Some(alias) = dist.aliases.first() {
        alias.clone()
    } else {
        dist.domain_name.clone()
    };
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            display_name,
            Style::default()
                .fg(crate::ui::theme::text_primary())
                .add_modifier(Modifier::BOLD),
        ),
    ]));

    // Status + enabled + ID
    let status_color = match (dist.status.as_str(), dist.enabled) {
        ("InProgress", _) => theme::warning(),
        (_, false) => theme::text_dim(),
        _ => theme::success(),
    };
    let enabled_text = if dist.enabled { "Enabled" } else { "Disabled" };
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(dist.id.clone(), Style::default().fg(theme::text_dim())),
        Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
        Span::styled(dist.status.clone(), Style::default().fg(status_color)),
        Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
        Span::styled(enabled_text.to_string(), Style::default().fg(status_color)),
    ]));

    // Domain
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            dist.domain_name.clone(),
            Style::default().fg(theme::text_dim()),
        ),
    ]));

    lines.push(Line::raw(""));
    lines
}

pub(super) fn render_cf_section_tabs(app: &App, area: Rect, frame: &mut Frame) {
    render_section_tab_bar(
        app,
        area,
        frame,
        &descriptor_tabs(app, &crate::aws::services::cloudfront::CF_DISTRIBUTION_SECTIONS),
    );
}

pub fn cf_distribution_section_lines(
    dist: &CfDistribution,
    section: CfDistributionDetailSection,
    tags_state: Option<&crate::lazy::Lazy<std::collections::HashMap<String, String>>>,
    invalidations_state: Option<&Lazy<CfInvalidationsData>>,
) -> Vec<(String, String)> {
    match section {
        CfDistributionDetailSection::Details => cf_details_lines(dist),
        CfDistributionDetailSection::Origins => cf_origins_lines(dist),
        CfDistributionDetailSection::Behaviors => cf_behaviors_lines(dist),
        CfDistributionDetailSection::Errors => cf_errors_lines(dist),
        CfDistributionDetailSection::Restrictions => cf_restrictions_lines(dist),
        CfDistributionDetailSection::Invalidations => {
            cf_invalidations_lines(invalidations_state)
        }
        CfDistributionDetailSection::Tags => cf_tags_lines(tags_state),
    }
}

pub(super) fn cf_invalidations_lines(
    state: Option<&Lazy<CfInvalidationsData>>,
) -> Vec<(String, String)> {

    match state {
        None | Some(Lazy::Loading) => {
            vec![("".to_string(), "Loading invalidations…".to_string())]
        }
        Some(Lazy::Error(e)) => error_rows(e),
        Some(Lazy::Loaded(data)) => {
            let mut rows = vec![];
            if data.items.is_empty() {
                rows.push(("".to_string(), "No invalidations".to_string()));
                return rows;
            }
            let header = if data.total > data.items.len() {
                format!("Invalidations (latest {} of {})", data.items.len(), data.total)
            } else {
                format!("Invalidations ({})", data.items.len())
            };
            rows.push((header, "".to_string()));
            rows.push(("".to_string(), "".to_string()));
            for inv in &data.items {
                let status = if inv.status.eq_ignore_ascii_case("InProgress") {
                    format!("⚠ {}", inv.status)
                } else {
                    inv.status.clone()
                };
                rows.push((format!("  {}", inv.created), format!("{} — {}", inv.id, status)));
                for p in &inv.paths {
                    rows.push((format!("    {}", p), "".to_string()));
                }
                if inv.paths_total > inv.paths.len() {
                    rows.push((
                        format!("    … +{} more paths", inv.paths_total - inv.paths.len()),
                        "".to_string(),
                    ));
                }
                rows.push(("".to_string(), "".to_string()));
            }
            rows
        }
    }
}

pub(super) fn cf_details_lines(dist: &CfDistribution) -> Vec<(String, String)> {
    let mut rows = vec![
        ("Domain Name".to_string(), dist.domain_name.clone()),
        ("ARN".to_string(), dist.arn.clone()),
        ("Status".to_string(), dist.status.clone()),
        (
            "Enabled".to_string(),
            if dist.enabled {
                "Yes".to_string()
            } else {
                "No".to_string()
            },
        ),
        ("".to_string(), "".to_string()),
    ];

    if !dist.aliases.is_empty() {
        rows.push(("Aliases".to_string(), "".to_string()));
        for alias in &dist.aliases {
            rows.push((format!("  {}", alias), "".to_string()));
        }
        rows.push(("".to_string(), "".to_string()));
    }

    rows.push(("Price Class".to_string(), dist.price_class.clone()));
    rows.push(("HTTP Version".to_string(), dist.http_version.clone()));
    rows.push((
        "IPv6 Enabled".to_string(),
        if dist.is_ipv6_enabled {
            "Yes".to_string()
        } else {
            "No".to_string()
        },
    ));
    rows.push(("".to_string(), "".to_string()));

    rows.push(("Viewer Certificate".to_string(), dist.viewer_cert.clone()));
    if !dist.min_tls.is_empty() {
        rows.push(("TLS Min Protocol".to_string(), dist.min_tls.clone()));
    }
    if !dist.ssl_method.is_empty() {
        rows.push(("SSL Method".to_string(), dist.ssl_method.clone()));
    }

    if dist.staging {
        rows.push((
            "Staging".to_string(),
            "⚠ Yes — staging distribution (continuous deployment)".to_string(),
        ));
    }
    if !dist.anycast_ip_list_id.is_empty() {
        rows.push(("Anycast IP List".to_string(), dist.anycast_ip_list_id.clone()));
    }

    if !dist.web_acl_id.is_empty() {
        // For WAFv2 this is the full ACL ARN → Enter jumps to WAF (CLOUDFRONT
        // scope) via arn_jump_target. WAF Classic ids (bare uuid) aren't jumpable.
        rows.push(("WAF Web ACL".to_string(), dist.web_acl_id.clone()));
    }

    if !dist.comment.is_empty() {
        rows.push(("Comment".to_string(), dist.comment.clone()));
    }

    if let Some(lm) = &dist.last_modified {
        rows.push(("Last Modified".to_string(), lm.clone()));
    }

    rows.push(("".to_string(), "".to_string()));
    rows
}

pub(super) fn cf_origins_lines(dist: &CfDistribution) -> Vec<(String, String)> {
    let mut rows = vec![];
    if dist.origins.is_empty() && dist.origin_groups.is_empty() {
        rows.push(("".to_string(), "No origins".to_string()));
        return rows;
    }
    for origin in &dist.origins {
        rows.push((origin.id.clone(), "".to_string()));
        // S3 origin domains (bucket.s3.region.amazonaws.com) jump to the bucket.
        rows.push(("  Domain".to_string(), origin.domain_name.clone()));
        if !origin.origin_path.is_empty() {
            rows.push(("  Path".to_string(), origin.origin_path.clone()));
        }
        rows.push(("  Type".to_string(), origin.kind.clone()));
        if !origin.oac_id.is_empty() {
            rows.push(("  Origin Access Control".to_string(), origin.oac_id.clone()));
        }
        if !origin.protocol_policy.is_empty() {
            rows.push(("  Protocol Policy".to_string(), origin.protocol_policy.clone()));
        }
        if !origin.custom_timeouts.is_empty() {
            rows.push(("  Timeouts / TLS".to_string(), origin.custom_timeouts.clone()));
        }
        if !origin.connection.is_empty() {
            rows.push(("  Connection".to_string(), origin.connection.clone()));
        }
        rows.push((
            "  Origin Shield".to_string(),
            if origin.shield_region.is_empty() {
                "Disabled".to_string()
            } else {
                format!("Enabled ({})", origin.shield_region)
            },
        ));
        if origin.custom_headers > 0 {
            rows.push((
                "  Custom Headers".to_string(),
                origin.custom_headers.to_string(),
            ));
        }
        rows.push(("".to_string(), "".to_string()));
    }

    if !dist.origin_groups.is_empty() {
        rows.push(("Origin Groups (failover)".to_string(), "".to_string()));
        for g in &dist.origin_groups {
            rows.push((format!("  {}", g.id), "".to_string()));
            rows.push(("  Members".to_string(), g.members.join(" → ")));
            if !g.status_codes.is_empty() {
                rows.push((
                    "  Failover On".to_string(),
                    g.status_codes
                        .iter()
                        .map(|c| c.to_string())
                        .collect::<Vec<_>>()
                        .join(", "),
                ));
            }
            rows.push(("".to_string(), "".to_string()));
        }
    }
    rows
}

pub(super) fn cf_behaviors_lines(dist: &CfDistribution) -> Vec<(String, String)> {
    let mut rows = vec![];
    if dist.behaviors.is_empty() {
        rows.push(("".to_string(), "No behaviors".to_string()));
        return rows;
    }

    for b in &dist.behaviors {
        rows.push((b.path_pattern.clone(), "".to_string()));
        rows.push(("  Origin".to_string(), b.target_origin_id.clone()));
        rows.push((
            "  Viewer Protocol".to_string(),
            b.viewer_protocol_policy.clone(),
        ));
        if !b.allowed_methods.is_empty() {
            rows.push(("  Methods".to_string(), b.allowed_methods.clone()));
        }
        if !b.cache_policy.is_empty() {
            rows.push(("  Cache Policy".to_string(), b.cache_policy.clone()));
        }
        if !b.origin_request_policy.is_empty() {
            rows.push((
                "  Origin Req Policy".to_string(),
                b.origin_request_policy.clone(),
            ));
        }
        if !b.response_headers_policy.is_empty() {
            rows.push((
                "  Resp Headers Policy".to_string(),
                b.response_headers_policy.clone(),
            ));
        }
        if let Some((min, def, max)) = b.legacy_ttls {
            rows.push((
                "  TTL (legacy)".to_string(),
                format!("min {}s · default {}s · max {}s", min, def, max),
            ));
        }
        if !b.legacy_forwarded.is_empty() {
            rows.push(("  Forwarded (legacy)".to_string(), b.legacy_forwarded.clone()));
        }
        rows.push((
            "  Compress".to_string(),
            if b.compress { "Yes" } else { "No" }.to_string(),
        ));
        if b.smooth_streaming {
            rows.push(("  Smooth Streaming".to_string(), "Yes".to_string()));
        }
        // Edge code: Lambda@Edge ARNs jump to the Lambda function.
        for (event, arn) in &b.lambda_edge {
            rows.push((format!("  Lambda@Edge · {}", event), arn.clone()));
        }
        for (event, arn) in &b.functions {
            rows.push((format!("  Function · {}", event), arn.clone()));
        }
        if !b.realtime_log_arn.is_empty() {
            rows.push(("  Realtime Logs".to_string(), b.realtime_log_arn.clone()));
        }
        if !b.field_level_encryption.is_empty() {
            rows.push((
                "  Field-level Encryption".to_string(),
                b.field_level_encryption.clone(),
            ));
        }
        if !b.trusted_key_groups.is_empty() {
            rows.push((
                "  Trusted Key Groups".to_string(),
                format!("⚠ signed URLs required — {}", b.trusted_key_groups.join(", ")),
            ));
        }
        if !b.trusted_signers.is_empty() {
            rows.push((
                "  Trusted Signers".to_string(),
                format!("⚠ signed URLs required — {}", b.trusted_signers.join(", ")),
            ));
        }
        rows.push(("".to_string(), "".to_string()));
    }
    rows
}

pub(super) fn cf_errors_lines(dist: &CfDistribution) -> Vec<(String, String)> {
    let mut rows = vec![];
    if dist.error_responses.is_empty() {
        rows.push(("".to_string(), "No custom error responses".to_string()));
        return rows;
    }
    rows.push(("Custom Error Responses".to_string(), "".to_string()));
    for er in &dist.error_responses {
        let mut val = String::new();
        if !er.response_page.is_empty() {
            val.push_str(&format!("→ {}", er.response_page));
            if !er.response_code.is_empty() {
                val.push_str(&format!(" ({})", er.response_code));
            }
        } else if !er.response_code.is_empty() {
            val.push_str(&format!("→ {}", er.response_code));
        } else {
            val.push_str("(no response override)");
        }
        if let Some(ttl) = er.min_ttl {
            val.push_str(&format!(" · TTL {}s", ttl));
        }
        rows.push((format!("  HTTP {}", er.error_code), val));
    }
    rows.push(("".to_string(), "".to_string()));
    rows
}

pub(super) fn cf_restrictions_lines(dist: &CfDistribution) -> Vec<(String, String)> {
    let mut rows = vec![];
    rows.push((
        "Geo Restriction".to_string(),
        dist.geo_restriction.kind.clone(),
    ));
    if !dist.geo_restriction.locations.is_empty() {
        rows.push(("".to_string(), "".to_string()));
        rows.push(("Countries".to_string(), "".to_string()));
        for loc in &dist.geo_restriction.locations {
            rows.push((format!("  {}", loc), "".to_string()));
        }
    }
    rows.push(("".to_string(), "".to_string()));
    rows
}

pub(super) fn cf_tags_lines(tags_state: Option<&crate::lazy::Lazy<std::collections::HashMap<String, String>>>) -> Vec<(String, String)> {
    match tags_state {
        None | Some(crate::lazy::Lazy::Loading) => {
            vec![("".to_string(), "Loading tags…".to_string())]
        }
        Some(crate::lazy::Lazy::Error(e)) => error_rows(e),
        Some(crate::lazy::Lazy::Loaded(tags)) => {
            let mut rows = vec![];
            if tags.is_empty() {
                rows.push(("".to_string(), "No tags".to_string()));
            } else {
                let mut sorted: Vec<(&String, &String)> = tags.iter().collect();
                sorted.sort_by_key(|(k, _)| k.as_str());
                for (key, value) in sorted {
                    rows.push((key.clone(), value.clone()));
                }
            }
            rows.push(("".to_string(), "".to_string()));
            rows
        }
    }
}

// ── CloudFront function split pane ────────────────────────────────────────────

pub(super) fn render_cf_function_split(
    app: &App,
    f: &crate::aws::services::cloudfront::CfFunction,
    area: Rect,
    frame: &mut Frame,
) {
    

    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 2, "");
    let mut block = theme::pane_block("CloudFront Function", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let stage_color = if f.published {
        theme::success()
    } else {
        theme::warning()
    };
    let stage_text = if f.published {
        "LIVE"
    } else {
        "DEVELOPMENT only"
    };
    let header_lines = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                f.name.clone(),
                Style::default()
                    .fg(crate::ui::theme::text_primary())
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(f.runtime.clone(), Style::default().fg(theme::text_dim())),
            Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
            Span::styled(stage_text.to_string(), Style::default().fg(stage_color)),
        ]),
        Line::raw(""),
    ];
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
    render_section_tab_bar(
        app,
        chunks[2],
        frame,
        &descriptor_tabs(app, &crate::aws::services::cloudfront::CF_FUNCTION_SECTIONS),
    );
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub fn cf_function_section_lines(
    f: &crate::aws::services::cloudfront::CfFunction,
    section: crate::aws::services::cloudfront::CfFunctionDetailSection,
    code_state: Option<&Lazy<String>>,
) -> Vec<(String, String)> {
    

    match section {
        CfFunctionDetailSection::Overview => {
            let mut rows = vec![
                ("Name".to_string(), f.name.clone()),
                ("ARN".to_string(), f.arn.clone()),
                ("Status".to_string(), f.status.clone()),
                ("Runtime".to_string(), f.runtime.clone()),
                (
                    "Stage".to_string(),
                    if f.published {
                        "LIVE (published)".to_string()
                    } else {
                        "⚠ DEVELOPMENT only — never published".to_string()
                    },
                ),
            ];
            if !f.comment.is_empty() {
                rows.push(("Comment".to_string(), f.comment.clone()));
            }
            if !f.created.is_empty() {
                rows.push(("Created".to_string(), f.created.clone()));
            }
            if !f.last_modified.is_empty() {
                rows.push(("Last Modified".to_string(), f.last_modified.clone()));
            }
            if !f.kv_stores.is_empty() {
                rows.push(("".to_string(), "".to_string()));
                rows.push(("Key Value Stores".to_string(), "".to_string()));
                for arn in &f.kv_stores {
                    rows.push((format!("  {}", arn), "".to_string()));
                }
            }
            rows.push(("".to_string(), "".to_string()));
            rows
        }
        CfFunctionDetailSection::Code => match code_state {
            None | Some(Lazy::Loading) => {
                vec![("".to_string(), "Loading code…".to_string())]
            }
            Some(Lazy::Error(e)) => error_rows(e),
            Some(Lazy::Loaded(code)) => {
                let mut rows = vec![];
                if code.is_empty() {
                    rows.push(("".to_string(), "(empty function)".to_string()));
                    return rows;
                }
                for line in code.lines() {
                    rows.push((format!("  {}", line), "".to_string()));
                }
                rows.push(("".to_string(), "".to_string()));
                rows
            }
        },
    }
}
