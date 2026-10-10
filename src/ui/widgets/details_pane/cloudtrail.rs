use super::*;

// ── CloudTrail event split pane ─────────────────────────────────────────────

pub(super) fn render_ct_event_split(
    app: &App,
    ev: &crate::aws::services::cloudtrail::CloudTrailEvent,
    area: Rect,
    frame: &mut Frame,
) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 5, "");
    let mut block = theme::pane_block("CloudTrail Event", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Header: event name + outcome dot + who/where one-liner.
    let (dot_color, outcome) = if !ev.error_code.is_empty() {
        (theme::error(), ev.error_code.clone())
    } else if ev.read_only {
        (theme::text_dim(), "read".to_string())
    } else {
        (theme::success(), "write".to_string())
    };
    let header = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                ev.event_name.clone(),
                Style::default().fg(crate::ui::theme::text_primary()).add_modifier(Modifier::BOLD),
            ),
            Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
            Span::styled("● ", Style::default().fg(dot_color)),
            Span::styled(outcome, Style::default().fg(dot_color)),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                format!("{}  ·  {}", ev.event_source, ev.event_time),
                Style::default().fg(theme::text_dim()),
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
    render_section_tab_bar(app, 
        chunks[2],
        frame,
        &descriptor_tabs(app, &crate::aws::services::cloudtrail::CT_EVENT_SECTIONS),
    );
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub fn ct_event_section_lines(
    ev: &crate::aws::services::cloudtrail::CloudTrailEvent,
    section: CtEventDetailSection,
) -> Vec<(String, String)> {
    let push_if = |rows: &mut Vec<(String, String)>, k: &str, v: &str| {
        if !v.is_empty() {
            rows.push((k.to_string(), v.to_string()));
        }
    };
    match section {
        CtEventDetailSection::Overview => {
            let mut rows = vec![
                ("Event Name".to_string(), ev.event_name.clone()),
                ("Event Time".to_string(), ev.event_time.clone()),
                ("Event Source".to_string(), ev.event_source.clone()),
            ];
            push_if(&mut rows, "Region", &ev.aws_region);
            push_if(&mut rows, "Event Type", &ev.event_type);
            push_if(&mut rows, "Category", &ev.event_category);
            rows.push((
                "Read Only".to_string(),
                if ev.read_only { "✓ yes" } else { "✗ no (mutation)" }.to_string(),
            ));
            if !ev.error_code.is_empty() {
                rows.push((String::new(), String::new()));
                rows.push(("Error".to_string(), String::new())); // group header
                rows.push((format!("  {}", ev.error_code), String::new()));
                if !ev.error_message.is_empty() {
                    rows.push((format!("  {}", ev.error_message), String::new()));
                }
            }
            rows.push((String::new(), String::new()));
            push_if(&mut rows, "Source IP", &ev.source_ip);
            push_if(&mut rows, "Recipient Acct", &ev.recipient_account_id);
            push_if(&mut rows, "Request ID", &ev.request_id);
            rows.push(("Event ID".to_string(), ev.event_id.clone()));
            if !ev.user_agent.is_empty() {
                rows.push((String::new(), String::new()));
                rows.push(("User Agent".to_string(), String::new())); // group header
                rows.push((format!("  {}", ev.user_agent), String::new()));
            }
            rows
        }
        CtEventDetailSection::Identity => {
            let mut rows = vec![];
            push_if(&mut rows, "Type", &ev.identity_type);
            // ARN rows are jump anchors (resource_jump_target handles IAM ARNs).
            push_if(&mut rows, "ARN", &ev.identity_arn);
            push_if(&mut rows, "User Name", &ev.identity_user_name);
            push_if(&mut rows, "Principal", &ev.identity_principal);
            push_if(&mut rows, "Account", &ev.identity_account);
            push_if(&mut rows, "Access Key", &ev.access_key_id);
            if !ev.mfa_authenticated.is_empty()
                || !ev.session_creation.is_empty()
                || !ev.session_issuer_arn.is_empty()
            {
                rows.push((String::new(), String::new()));
                rows.push(("Session".to_string(), String::new())); // group header
                push_if(&mut rows, "  MFA", &ev.mfa_authenticated);
                push_if(&mut rows, "  Created", &ev.session_creation);
                // Issuer ARN is also a jump anchor (the assumed role).
                push_if(&mut rows, "Assumed Role", &ev.session_issuer_arn);
            }
            if rows.is_empty() {
                rows.push(("  (no identity detail)".to_string(), String::new()));
            }
            rows
        }
        CtEventDetailSection::Request => json_block_rows(&ev.request_parameters),
        CtEventDetailSection::Response => json_block_rows(&ev.response_elements),
        CtEventDetailSection::Resources => {
            if ev.resources.is_empty() {
                return vec![("  (no resources recorded)".to_string(), String::new())];
            }
            let mut rows = vec![(format!("Resources ({})", ev.resources.len()), String::new())];
            rows.push((String::new(), String::new()));
            for (ty, arn) in &ev.resources {
                // ARN/name in the value → resource_jump_target makes it a hop (⏎).
                let key = if ty.is_empty() { "Resource".to_string() } else { ty.clone() };
                rows.push((key, arn.clone()));
            }
            rows
        }
    }
}

// ── CloudTrail trail split pane ─────────────────────────────────────────────

pub(super) fn render_ct_trail_split(
    app: &App,
    trail: &crate::aws::services::cloudtrail::CloudTrailTrail,
    area: Rect,
    frame: &mut Frame,
) {
    let mut scope = vec![trail.home_region.clone()];
    if trail.is_multi_region {
        scope.push("multi-region".to_string());
    }
    if trail.is_org_trail {
        scope.push("organization".to_string());
    }
    render_simple_split(
        app,
        area,
        frame,
        "CloudTrail Trail",
        &trail.name,
        &scope.join(" · "),
        &descriptor_tabs(app, &crate::aws::services::cloudtrail::CT_TRAIL_SECTIONS),
    );
}

pub fn ct_trail_section_lines(
    trail: &crate::aws::services::cloudtrail::CloudTrailTrail,
    section: CtTrailDetailSection,
) -> Vec<(String, String)> {
    let push_if = |rows: &mut Vec<(String, String)>, k: &str, v: &str| {
        if !v.is_empty() {
            rows.push((k.to_string(), v.to_string()));
        }
    };
    match section {
        CtTrailDetailSection::Overview => {
            let mut rows = vec![
                ("Name".to_string(), trail.name.clone()),
                ("Home region".to_string(), trail.home_region.clone()),
                (
                    "Multi-region".to_string(),
                    if trail.is_multi_region { "✓ yes" } else { "✗ no" }.to_string(),
                ),
                (
                    "Organization trail".to_string(),
                    if trail.is_org_trail { "✓ yes" } else { "✗ no" }.to_string(),
                ),
                (
                    "Global service events".to_string(),
                    if trail.include_global_events {
                        "✓ included"
                    } else {
                        "✗ excluded"
                    }
                    .to_string(),
                ),
                (
                    "Log file validation".to_string(),
                    if trail.log_file_validation {
                        "✓ enabled"
                    } else {
                        "✗ disabled"
                    }
                    .to_string(),
                ),
                (
                    "Insights".to_string(),
                    if trail.has_insight_selectors {
                        "✓ enabled"
                    } else {
                        "not configured"
                    }
                    .to_string(),
                ),
                (String::new(), String::new()),
                ("Destinations".to_string(), String::new()), // group header
            ];
            // `s3://` value → jumps to the bucket via the generic classifier.
            if !trail.s3_bucket.is_empty() {
                let uri = if trail.s3_prefix.is_empty() {
                    format!("s3://{}", trail.s3_bucket)
                } else {
                    format!("s3://{}/{}", trail.s3_bucket, trail.s3_prefix)
                };
                rows.push(("S3".to_string(), uri));
            }
            if !trail.log_group_arn.is_empty() {
                let group = trail
                    .cw_log_group()
                    .map(|(g, _)| g)
                    .unwrap_or_else(|| trail.log_group_arn.clone());
                rows.push(("CloudWatch Logs".to_string(), group));
                rows.push((
                    "  (press t to tail — cross-region for multi-region trails)".to_string(),
                    String::new(),
                ));
                push_if(&mut rows, "  Delivery role", &trail.cw_role_arn);
            }
            push_if(&mut rows, "SNS topic", &trail.sns_topic_arn);
            push_if(&mut rows, "KMS key", &trail.kms_key_id);
            rows.push((String::new(), String::new()));
            rows.push(("ARN".to_string(), trail.arn.clone()));
            rows
        }
        CtTrailDetailSection::Status => {
            if let Some(err) = &trail.status_error {
                return error_rows(err);
            }
            let mut rows = vec![(
                "Logging".to_string(),
                match trail.is_logging {
                    Some(true) => "✓ on".to_string(),
                    Some(false) => "✗ OFF — no events are being recorded".to_string(),
                    None => "unknown".to_string(),
                },
            )];
            push_if(&mut rows, "Started", &trail.start_logging_time);
            push_if(&mut rows, "Stopped", &trail.stop_logging_time);
            rows.push((String::new(), String::new()));
            rows.push(("Delivery".to_string(), String::new())); // group header
            push_if(&mut rows, "Last S3 delivery", &trail.latest_delivery_time);
            if !trail.latest_delivery_error.is_empty() {
                rows.push((
                    format!("  ⚠ S3: {}", trail.latest_delivery_error),
                    String::new(),
                ));
            }
            push_if(&mut rows, "Last CW Logs delivery", &trail.latest_cw_delivery_time);
            if !trail.latest_cw_delivery_error.is_empty() {
                rows.push((
                    format!("  ⚠ CloudWatch Logs: {}", trail.latest_cw_delivery_error),
                    String::new(),
                ));
            }
            push_if(&mut rows, "Last digest delivery", &trail.latest_digest_time);
            if !trail.latest_digest_error.is_empty() {
                rows.push((
                    format!("  ⚠ Digest: {}", trail.latest_digest_error),
                    String::new(),
                ));
            }
            if !trail.latest_notification_error.is_empty() {
                rows.push((
                    format!("  ⚠ SNS: {}", trail.latest_notification_error),
                    String::new(),
                ));
            }
            if !trail.has_delivery_error() {
                rows.push(("  All delivery legs healthy".to_string(), String::new()));
            }
            rows
        }
        CtTrailDetailSection::Selectors => {
            if let Some(err) = &trail.selectors_error {
                return error_rows(err);
            }
            if trail.selector_rows.is_empty() {
                return vec![("  (no event selectors)".to_string(), String::new())];
            }
            trail.selector_rows.clone()
        }
    }
}

/// Render a pretty-printed JSON block as plain, indented body lines (with a
/// `v`-viewer hint). Empty/absent → a "(none)" line.
pub(super) fn json_block_rows(pretty: &str) -> Vec<(String, String)> {
    if pretty.trim().is_empty() {
        return vec![("  (none)".to_string(), String::new())];
    }
    let mut rows = Vec::new();
    rows.push((String::new(), String::new()));
    for line in pretty.lines() {
        rows.push((format!("  {}", line), String::new()));
    }
    rows
}
