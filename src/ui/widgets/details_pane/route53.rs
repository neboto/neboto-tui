use super::*;

// ── Route53 Hosted Zone split pane ────────────────────────────────────────────

pub(super) fn render_r53_zone_split(app: &App, zone: &R53HostedZone, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;

    let footer = detail_footer(app, focused, 3, "");

    let mut block = theme::pane_block("R53 Hosted Zone", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let header_lines = build_r53_zone_header_lines(zone);
    let header_h = header_lines.len() as u16;

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(header_h),
            Constraint::Length(1), // hr
            Constraint::Length(1), // tab bar
            Constraint::Length(1), // hr
            Constraint::Min(0),    // body
        ])
        .split(inner);

    frame.render_widget(Paragraph::new(header_lines), chunks[0]);
    render_hr(chunks[1], frame);
    render_r53_section_tabs(app, chunks[2], frame);
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub(super) fn build_r53_zone_header_lines(zone: &R53HostedZone) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    // Zone name — bold primary identifier
    lines.push(Line::from(vec![
        Span::raw(" "),
        Span::styled(
            zone.name.clone(),
            Style::default()
                .fg(theme::aws_orange())
                .add_modifier(Modifier::BOLD),
        ),
    ]));

    // Type + record count
    let type_color = if zone.private_zone {
        theme::warning()
    } else {
        theme::success()
    };
    lines.push(Line::from(vec![
        Span::raw(" "),
        Span::styled(
            zone.zone_type().to_string(),
            Style::default().fg(type_color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  {} records", zone.record_count),
            Style::default().fg(theme::text_dim()),
        ),
    ]));

    // Zone ID
    lines.push(header_kv("Zone ID", zone.bare_id()));

    // Comment if present
    if !zone.comment.is_empty() {
        lines.push(Line::from(vec![
            Span::raw(" "),
            Span::styled(zone.comment.clone(), Style::default().fg(theme::text_dim())),
        ]));
    }

    lines
}

pub(super) fn render_r53_section_tabs(app: &App, area: Rect, frame: &mut Frame) {
    render_section_tab_bar(
        app,
        area,
        frame,
        &descriptor_tabs(app, &crate::aws::services::route53::R53_ZONE_SECTIONS),
    );
}

pub fn r53_zone_section_lines(
    zone: &R53HostedZone,
    section: R53ZoneDetailSection,
    records_state: Option<&crate::lazy::Lazy<Vec<crate::aws::services::route53::R53Record>>>,
    sharing_state: Option<
        &crate::lazy::Lazy<Box<crate::aws::services::route53::R53ZoneDetail>>,
    >,
) -> Vec<(String, String)> {
    match section {
        R53ZoneDetailSection::Records => r53_records_lines(records_state),
        R53ZoneDetailSection::Sharing => r53_sharing_lines(zone, sharing_state),
        R53ZoneDetailSection::Info => r53_info_lines(zone, sharing_state),
        R53ZoneDetailSection::Tags => r53_tags_lines(sharing_state),
    }
}

pub(super) fn r53_sharing_lines(
    zone: &R53HostedZone,
    state: Option<&crate::lazy::Lazy<Box<crate::aws::services::route53::R53ZoneDetail>>>,
) -> Vec<(String, String)> {
    let mut rows = vec![("".to_string(), "".to_string())];

    if !zone.private_zone {
        rows.push(("  — (public zone, not shared)".to_string(), "".to_string()));
        return rows;
    }

    match state {
        None | Some(crate::lazy::Lazy::Loading) => {
            rows.push(("  Loading…".to_string(), "".to_string()));
        }
        Some(crate::lazy::Lazy::Error(err)) => rows.extend(error_rows(err)),
        Some(crate::lazy::Lazy::Loaded(d)) => {
            let authorized_ids: std::collections::HashSet<&str> =
                d.authorized.iter().map(|v| v.vpc_id.as_str()).collect();

            rows.push((format!("Associated ({})", d.associated.len()), "".to_string()));
            rows.push(("".to_string(), "".to_string()));
            if d.associated.is_empty() {
                rows.push(("  (no associations)".to_string(), "".to_string()));
            } else {
                for v in &d.associated {
                    // vpc id carries the raw vpc- token (jumpable to VPC when
                    // it's in the current region — cross-region ids are just
                    // shown, no jump attempted).
                    rows.push(("VPC".to_string(), v.vpc_id.clone()));
                    rows.push(("Region".to_string(), v.vpc_region.clone()));
                    if authorized_ids.contains(v.vpc_id.as_str()) {
                        rows.push((
                            "  ⚠ authorization still active after association".to_string(),
                            "".to_string(),
                        ));
                    }
                    rows.push(("".to_string(), "".to_string()));
                }
            }

            rows.push((
                format!("Authorized, not yet associated ({})", d.authorized.len()),
                "".to_string(),
            ));
            rows.push(("".to_string(), "".to_string()));
            if d.authorized.is_empty() {
                rows.push(("  (no pending authorizations)".to_string(), "".to_string()));
            } else {
                for v in &d.authorized {
                    rows.push(("VPC".to_string(), v.vpc_id.clone()));
                    rows.push(("Region".to_string(), v.vpc_region.clone()));
                    rows.push(("".to_string(), "".to_string()));
                }
            }
        }
    }
    rows
}

pub(super) fn r53_records_lines(records_state: Option<&crate::lazy::Lazy<Vec<crate::aws::services::route53::R53Record>>>) -> Vec<(String, String)> {
    let mut rows = vec![("".to_string(), "".to_string())];

    match records_state {
        None => {
            rows.push(("  Loading…".to_string(), "".to_string()));
        }
        Some(crate::lazy::Lazy::Loading) => {
            rows.push(("  Loading records…".to_string(), "".to_string()));
        }
        Some(crate::lazy::Lazy::Error(e)) => {
            rows.extend(error_rows(e));
        }
        Some(crate::lazy::Lazy::Loaded(records)) => {
            if records.is_empty() {
                rows.push(("  No records found".to_string(), "".to_string()));
            } else {
                // Key-value rows, laid out for the adaptive key column
                // (`key_col_widths`): the key is `TYPE name` — type first so
                // that when a long name overruns the column cap the ellipsis
                // eats the name's tail, never the type — and the value is
                // `first-value · ttl`, TTL last so a narrow pane clips the
                // least important column. The old `name|type|ttl` 70-column
                // key put the type past KEY_COL_MAX, so it was clipped in
                // every pane width (#17). The name is **zone-relative**
                // (`@`, `www`): the FQDN repeated the zone on every row and
                // one long name dragged the key column to its cap, leaving
                // the values ~18 columns in a split pane. Values stay in the
                // value slot so an S3-website alias remains ⏎-jumpable, and
                // ⏎ on any row opens the record's own pane on the Records
                // tab (`r53_row_jump_target`), where nothing is truncated.
                rows.push((
                    format!("  {:<6} {}", "TYPE", "NAME"),
                    "VALUE · TTL".to_string(),
                ));

                for record in records {
                    let ttl = record.ttl
                        .map(|t| t.to_string())
                        .unwrap_or_else(|| "alias".to_string());

                    // Collect all values (alias counts as one value)
                    let all_values: Vec<&str> = if let Some(alias) = &record.alias_target {
                        vec![alias.as_str()]
                    } else {
                        record.values.iter().map(|s| s.as_str()).collect()
                    };

                    // Char-based truncation: a byte slice panics on a
                    // multi-byte character at the boundary (TXT records).
                    let first = cost_trunc(all_values.first().copied().unwrap_or("—"), 40);
                    rows.push((
                        format!("  {:<6} {}", record.record_type, record.relative_name()),
                        format!("{first} · {ttl}"),
                    ));

                    // Continuation rows for multi-value records (NS, MX, TXT, …):
                    // a blank (non-empty) key keeps the key-value shape so the
                    // value aligns under the first row's.
                    for val in all_values.iter().skip(1) {
                        rows.push(("        ".to_string(), cost_trunc(val, 40)));
                    }

                    // Without these, every record in a weighted/latency/failover
                    // set renders as an identical row — the set identifier is the
                    // only thing that tells them apart.
                    if let Some(summary) = record.routing_summary() {
                        rows.push((format!("      ↳ {summary}"), String::new()));
                    }
                    if record.alias_target.is_some() && record.evaluate_target_health {
                        rows.push((
                            "      ↳ evaluates target health".to_string(),
                            String::new(),
                        ));
                    }
                    if let Some(tp) = &record.traffic_policy_instance_id {
                        rows.push((
                            format!("      ↳ managed by traffic policy {tp}"),
                            String::new(),
                        ));
                    }
                    // A labelled key-value row, not a `↳` annotation, so
                    // `r53_row_jump_target` can route ⏎ to the Health Checks tab.
                    if let Some(hc) = &record.health_check_id {
                        rows.push(("      Health Check".to_string(), hc.clone()));
                    }
                }
            }
        }
    }

    rows.push(("".to_string(), "".to_string()));
    rows
}

pub(super) fn r53_info_lines(
    zone: &R53HostedZone,
    state: Option<&crate::lazy::Lazy<Box<crate::aws::services::route53::R53ZoneDetail>>>,
) -> Vec<(String, String)> {
    let mut rows = vec![("".to_string(), "".to_string())];

    rows.push(("Zone Name".to_string(), zone.name.clone()));
    rows.push(("Zone ID".to_string(), zone.bare_id().to_string()));
    rows.push(("Type".to_string(), zone.zone_type().to_string()));

    let detail = match state {
        Some(crate::lazy::Lazy::Loaded(d)) => Some(d),
        _ => None,
    };

    // Record count against the zone's ceiling once the limit is known.
    match detail.and_then(|d| d.record_limit) {
        Some(limit) if limit > 0 => {
            let pct = zone.record_count as f64 * 100.0 / limit as f64;
            let flag = if pct >= 90.0 { " ⚠" } else { "" };
            rows.push((
                "Records".to_string(),
                format!("{} / {} ({:.0}%){}", zone.record_count, limit, pct, flag),
            ));
        }
        _ => rows.push(("Records".to_string(), zone.record_count.to_string())),
    }

    if !zone.comment.is_empty() {
        rows.push(("Comment".to_string(), zone.comment.clone()));
    }

    match state {
        None | Some(crate::lazy::Lazy::Loading) => {
            rows.push(("".to_string(), "".to_string()));
            rows.push(("  Loading…".to_string(), "".to_string()));
            return rows;
        }
        Some(crate::lazy::Lazy::Error(err)) => {
            rows.extend(error_rows(err));
            return rows;
        }
        Some(crate::lazy::Lazy::Loaded(_)) => {}
    }
    let d = detail.expect("loaded");

    // Query logging: the log group is rendered as its ARN-less name, jumpable
    // to CloudWatch via the `t`-tail hint rather than a link (log group rows
    // jump by ARN, and Route 53 only hands back the ARN we already parsed).
    rows.push((
        "Query Logging".to_string(),
        match &d.query_log_group {
            Some((group, region)) => format!("{} ({}) · t tail", group, region),
            None => "✗ not enabled".to_string(),
        },
    ));

    if zone.private_zone {
        rows.push(("DNSSEC".to_string(), "— (private zone)".to_string()));
        rows.push(("Name Servers".to_string(), "— (private zone)".to_string()));
        rows.push(("".to_string(), "".to_string()));
        return rows;
    }

    match &d.dnssec {
        Some(ds) => {
            let v = match ds.status.as_str() {
                "SIGNING" => "✓ SIGNING".to_string(),
                "NOT_SIGNING" => "NOT_SIGNING".to_string(),
                other => format!("⚠ {}", other),
            };
            let v = match &ds.status_message {
                Some(m) if !m.is_empty() => format!("{} — {}", v, m),
                _ => v,
            };
            rows.push(("DNSSEC".to_string(), v));
            for (name, status, algo) in &ds.keys {
                rows.push((format!("  KSK {}", name), format!("{} · {}", status, algo)));
            }
        }
        None => rows.push(("DNSSEC".to_string(), "· status unavailable".to_string())),
    }

    rows.push(("".to_string(), "".to_string()));
    rows.push((format!("Name Servers ({})", d.name_servers.len()), "".to_string()));
    if d.name_servers.is_empty() {
        rows.push(("  · none reported".to_string(), "".to_string()));
    }
    for ns in &d.name_servers {
        rows.push((format!("  {}", ns), "".to_string()));
    }

    rows.push(("".to_string(), "".to_string()));
    rows
}

pub(super) fn r53_tags_lines(
    state: Option<&crate::lazy::Lazy<Box<crate::aws::services::route53::R53ZoneDetail>>>,
) -> Vec<(String, String)> {
    let mut rows = vec![("".to_string(), "".to_string())];

    match state {
        None | Some(crate::lazy::Lazy::Loading) => {
            rows.push(("  Loading…".to_string(), "".to_string()));
        }
        Some(crate::lazy::Lazy::Error(err)) => rows.extend(error_rows(err)),
        // The bundle is best-effort on tags: a failed `ListTagsForResource`
        // must not read as "No tags" (#26).
        Some(crate::lazy::Lazy::Loaded(d)) if d.tags_error.is_some() => {
            rows.extend(error_rows(d.tags_error.as_deref().unwrap_or_default()));
        }
        Some(crate::lazy::Lazy::Loaded(d)) if d.tags.is_empty() => {
            rows.push(("  No tags".to_string(), "".to_string()));
        }
        Some(crate::lazy::Lazy::Loaded(d)) => {
            let mut sorted = d.tags.clone();
            sorted.sort_by(|(a, _), (b, _)| a.cmp(b));
            for (key, value) in sorted {
                rows.push((format!("  {}", key), value));
            }
        }
    }

    rows.push(("".to_string(), "".to_string()));
    rows
}

// ── R53 Health Check Split Pane ───────────────────────────────────────────────

pub(super) fn render_r53_health_check_split(
    app: &App,
    hc: &R53HealthCheck,
    area: Rect,
    frame: &mut Frame,
) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 3, "");

    let mut block = theme::pane_block("R53 Health Check", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let header_lines = build_r53_health_check_header_lines(hc);
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
    render_r53_health_check_section_tabs(app, chunks[2], frame);
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub(super) fn build_r53_health_check_header_lines(hc: &R53HealthCheck) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    lines.push(Line::from(vec![
        Span::raw(" "),
        Span::styled(
            hc.name().to_string(),
            Style::default().fg(theme::aws_orange()).add_modifier(Modifier::BOLD),
        ),
    ]));
    let (state_txt, state_color) = if hc.disabled {
        ("Disabled", theme::text_dim())
    } else {
        ("Enabled", theme::success())
    };
    lines.push(Line::from(vec![
        Span::raw(" "),
        Span::styled(hc.check_type.clone(), Style::default().fg(theme::accent()).add_modifier(Modifier::BOLD)),
        Span::styled(format!("  {}", state_txt), Style::default().fg(state_color)),
    ]));
    lines.push(Line::from(vec![
        Span::raw(" "),
        Span::styled(hc.target(), Style::default().fg(theme::text_dim())),
    ]));
    lines.push(header_kv("Health Check ID", &hc.id));
    lines
}

pub(super) fn render_r53_health_check_section_tabs(app: &App, area: Rect, frame: &mut Frame) {
    render_section_tab_bar(
        app,
        area,
        frame,
        &descriptor_tabs(app, &crate::aws::services::route53::R53_HEALTH_CHECK_SECTIONS),
    );
}

pub fn r53_health_check_section_lines(
    hc: &R53HealthCheck,
    section: R53HealthCheckDetailSection,
    status_state: Option<&crate::lazy::Lazy<crate::aws::services::route53::R53HealthStatus>>,
) -> Vec<(String, String)> {
    match section {
        R53HealthCheckDetailSection::Overview => r53_health_overview_lines(hc),
        R53HealthCheckDetailSection::Status => r53_health_status_lines(status_state),
        R53HealthCheckDetailSection::Tags => r53_health_tags_lines(hc),
    }
}

pub fn r53_record_section_lines(
    rec: &crate::aws::services::route53::R53Record,
    section: crate::aws::services::route53::R53RecordDetailSection,
    answer: Option<&crate::lazy::Lazy<crate::aws::services::route53::R53TestAnswer>>,
) -> Vec<(String, String)> {
    use crate::aws::services::route53::R53RecordDetailSection as S;
    match section {
        // The rows the flat pane always showed — `r53_row_jump_target` keys
        // on their labels (Zone ID, Health Check, Alias Target, Target).
        S::Details => {
            let mut rows = vec![("".to_string(), "".to_string())];
            rows.extend(rec.details());
            rows
        }
        S::TestAnswer => r53_test_answer_lines(rec, answer),
    }
}

pub(super) fn r53_test_answer_lines(
    rec: &crate::aws::services::route53::R53Record,
    answer: Option<&crate::lazy::Lazy<crate::aws::services::route53::R53TestAnswer>>,
) -> Vec<(String, String)> {
    let mut rows = vec![("".to_string(), "".to_string())];
    let a = match answer {
        None => {
            rows.push((
                "".to_string(),
                "What Route 53 itself answers for this name and type, after routing policy and health checks · press x to ask".to_string(),
            ));
            return rows;
        }
        Some(crate::lazy::Lazy::Loading) => {
            rows.push(("".to_string(), "Loading…".to_string()));
            return rows;
        }
        Some(crate::lazy::Lazy::Error(e)) => {
            rows.extend(error_rows(e));
            return rows;
        }
        Some(crate::lazy::Lazy::Loaded(a)) => a,
    };
    rows.push((format!("Answer at {}", a.asked_at), "".to_string())); // group header
    rows.push(("Response Code".to_string(), a.response_code.clone()));
    rows.push(("Protocol".to_string(), a.protocol.clone()));
    rows.push(("Nameserver".to_string(), a.nameserver.clone()));
    rows.push(("".to_string(), "".to_string()));
    if a.record_data.is_empty() {
        rows.push(("".to_string(), "No records in the answer".to_string()));
    } else {
        rows.push((format!("Record Data ({})", a.record_data.len()), "".to_string()));
        for d in &a.record_data {
            rows.push((format!("  {}", d), "".to_string()));
        }
    }
    if let Some(same) = r53_answer_matches_record(rec, &a.record_data) {
        rows.push(("".to_string(), "".to_string()));
        rows.push((
            "".to_string(),
            if same {
                "✓ matches this record's values".to_string()
            } else if rec.routing_policy() == "simple" {
                "· differs from this record's values".to_string()
            } else {
                "· another answer than this record's values — routing picked a different record in the set, or a health check failed this one over".to_string()
            },
        ));
    }
    rows.push(("".to_string(), "".to_string()));
    rows.push(("".to_string(), "· press x to ask again (weighted sets resample)".to_string()));
    rows
}

/// Whether a test answer is exactly this record's configured values
/// (order-, case- and trailing-dot-insensitive). `None` for alias records,
/// whose answer is the target's addresses and has nothing to compare with.
pub(super) fn r53_answer_matches_record(
    rec: &crate::aws::services::route53::R53Record,
    data: &[String],
) -> Option<bool> {
    if rec.values.is_empty() || data.is_empty() {
        return None;
    }
    let norm = |v: &[String]| {
        let mut v: Vec<String> = v
            .iter()
            .map(|s| s.trim().trim_end_matches('.').to_ascii_lowercase())
            .collect();
        v.sort();
        v
    };
    Some(norm(&rec.values) == norm(data))
}

pub(super) fn r53_health_overview_lines(hc: &R53HealthCheck) -> Vec<(String, String)> {
    let mut rows = vec![("".to_string(), "".to_string())];
    rows.push(("Configuration".to_string(), "".to_string())); // group header
    rows.push(("Type".to_string(), hc.check_type.clone()));
    rows.push(("Target".to_string(), hc.target()));
    if let Some(fqdn) = &hc.fqdn {
        rows.push(("FQDN".to_string(), fqdn.clone()));
    }
    if let Some(ip) = &hc.ip_address {
        rows.push(("IP Address".to_string(), ip.clone()));
    }
    if let Some(p) = hc.port {
        rows.push(("Port".to_string(), p.to_string()));
    }
    if let Some(path) = &hc.resource_path {
        rows.push(("Resource Path".to_string(), path.clone()));
    }
    if let Some(s) = &hc.search_string {
        rows.push(("Search String".to_string(), s.clone()));
    }

    rows.push(("".to_string(), "".to_string()));
    rows.push(("Evaluation".to_string(), "".to_string())); // group header
    if let Some(i) = hc.request_interval {
        rows.push(("Request Interval".to_string(), format!("{}s", i)));
    }
    if let Some(f) = hc.failure_threshold {
        rows.push(("Failure Threshold".to_string(), f.to_string()));
    }
    rows.push((
        "Measure Latency".to_string(),
        if hc.measure_latency { "✓ yes".to_string() } else { "no".to_string() },
    ));
    rows.push((
        "Inverted".to_string(),
        if hc.inverted { "⚠ yes".to_string() } else { "no".to_string() },
    ));
    rows.push((
        "Disabled".to_string(),
        if hc.disabled { "⚠ yes".to_string() } else { "no".to_string() },
    ));
    if hc.check_type.contains("HTTPS") {
        rows.push((
            "Enable SNI".to_string(),
            if hc.enable_sni { "✓ yes".to_string() } else { "no".to_string() },
        ));
    }

    if !hc.child_health_checks.is_empty() {
        rows.push(("".to_string(), "".to_string()));
        rows.push(("Child Health Checks".to_string(), "".to_string())); // group header
        for c in &hc.child_health_checks {
            rows.push((format!("  {}", c), "".to_string()));
        }
    }
    if let Some(alarm) = &hc.alarm_name {
        rows.push(("".to_string(), "".to_string()));
        rows.push(("CloudWatch Alarm".to_string(), alarm.clone()));
    }

    rows.push(("".to_string(), "".to_string()));
    rows.push(("Checker Regions".to_string(), "".to_string())); // group header
    if hc.regions.is_empty() {
        rows.push(("  (default global set)".to_string(), "".to_string()));
    } else {
        for r in &hc.regions {
            rows.push((format!("  {}", r), "".to_string()));
        }
    }

    rows.push(("".to_string(), "".to_string()));
    rows
}

pub(super) fn r53_health_status_lines(state: Option<&crate::lazy::Lazy<crate::aws::services::route53::R53HealthStatus>>) -> Vec<(String, String)> {
    let mut rows = vec![("".to_string(), "".to_string())];
    match state {
        None => {
            rows.push(("  Not loaded".to_string(), "".to_string()));
            rows.push(("  Press 2 to load live status".to_string(), "".to_string()));
        }
        Some(crate::lazy::Lazy::Loading) => {
            rows.push(("  Loading live status…".to_string(), "".to_string()));
        }
        Some(crate::lazy::Lazy::Error(e)) => {
            rows.extend(error_rows(e));
        }
        Some(crate::lazy::Lazy::Loaded(st)) => {
            let obs = &st.observations;
            if obs.is_empty() {
                rows.push((
                    "  No observations (calculated/alarm checks report no per-region status)".to_string(),
                    "".to_string(),
                ));
            } else {
                let healthy = obs.iter().filter(|o| o.status.starts_with("Success")).count();
                rows.push((
                    format!("Health checkers reporting healthy: {}/{}", healthy, obs.len()),
                    "".to_string(),
                )); // group header
                rows.push(("".to_string(), "".to_string()));
                for o in obs {
                    rows.push((format!("  {}", o.region), o.status.clone()));
                    if let Some(t) = &o.checked_time {
                        rows.push((format!("  {:<20}", "  checked"), t.clone()));
                    }
                }
            }
            rows.extend(r53_last_failure_lines(st));
        }
    }
    rows.push(("".to_string(), "".to_string()));
    rows
}

/// "Last failure" group under the live observations: per checker, the most
/// recent failure reason and when it was seen — so a check that is healthy
/// now but flapped an hour ago still says so.
pub(super) fn r53_last_failure_lines(st: &crate::aws::services::route53::R53HealthStatus) -> Vec<(String, String)> {
    let mut rows = vec![
        ("".to_string(), "".to_string()),
        ("Last failure per checker".to_string(), "".to_string()), // group header
    ];
    if let Some(e) = &st.last_failure_error {
        rows.extend(error_rows(e));
        return rows;
    }
    if st.last_failures.is_empty() {
        rows.push(("".to_string(), "No failures on record".to_string()));
        return rows;
    }
    // Newest first: that's the flap you came to find.
    let mut failures: Vec<_> = st.last_failures.iter().collect();
    failures.sort_by(|a, b| b.checked_time.cmp(&a.checked_time));
    for f in failures {
        rows.push((format!("  {}", f.region), f.status.clone()));
        if let Some(t) = &f.checked_time {
            rows.push((format!("  {:<20}", "  at"), t.clone()));
        }
    }
    rows
}

pub(super) fn r53_health_tags_lines(hc: &R53HealthCheck) -> Vec<(String, String)> {
    let mut rows = vec![("".to_string(), "".to_string())];
    if hc.tags.is_empty() {
        rows.push(("  No tags".to_string(), "".to_string()));
    } else {
        let mut sorted: Vec<(&String, &String)> = hc.tags.iter().collect();
        sorted.sort_by_key(|(k, _)| k.as_str());
        for (key, value) in sorted {
            rows.push((format!("  {}", key), value.clone()));
        }
    }
    rows.push(("".to_string(), "".to_string()));
    rows
}
