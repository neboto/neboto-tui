use super::*;

// ── Control Tower split panes ────────────────────────────────────────────────

pub(super) fn render_landing_zone_split(app: &App, z: &LandingZone, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 4, "");
    let mut block = theme::pane_block("Landing Zone", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let (status_label, state_color) = match (z.status.as_deref(), z.drift_status.as_deref()) {
        (_, Some("DRIFTED")) => ("DRIFTED".to_string(), theme::error()),
        (Some("FAILED"), _) => ("FAILED".to_string(), theme::error()),
        (Some("PROCESSING"), _) => ("PROCESSING".to_string(), theme::warning()),
        (Some(s), _) if z.version_outdated() => (format!("{} · update available", s), theme::warning()),
        (Some(s), _) => (s.to_string(), theme::success()),
        (None, _) => ("unknown".to_string(), theme::text_dim()),
    };
    let header = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                format!("Landing Zone v{}", z.version),
                Style::default().fg(theme::text_primary()).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(status_label, Style::default().fg(state_color)),
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
        &descriptor_tabs(app, &crate::aws::services::controltower::LANDING_ZONE_SECTIONS),
    );
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub fn landing_zone_section_lines(
    z: &LandingZone,
    section: LandingZoneDetailSection,
    sibling_ops: &[&CtOp],
    tags: Option<&Lazy<Vec<(String, String)>>>,
) -> Vec<(String, String)> {
    match section {
        LandingZoneDetailSection::Overview => {
            let mut rows = vec![
                ("ARN".to_string(), z.arn.clone()),
                ("Version".to_string(), z.version.clone()),
            ];
            if let Some(latest) = &z.latest_available_version {
                if z.version_outdated() {
                    rows.push((
                        "Latest Available".to_string(),
                        format!("⚠ {} — update available", latest),
                    ));
                } else {
                    rows.push(("Latest Available".to_string(), latest.clone()));
                }
            }
            if let Some(s) = &z.status {
                rows.push(("Status".to_string(), s.clone()));
            }
            if let Some(d) = &z.drift_status {
                rows.push(("Drift".to_string(), drift_mark(d)));
            }
            if !z.remediation_types.is_empty() {
                rows.push(("Remediation".to_string(), z.remediation_types.join(", ")));
            }
            if let Some(e) = &z.detail_error {
                rows.extend(error_rows(e));
            }
            rows
        }
        LandingZoneDetailSection::Manifest => match &z.manifest_pretty {
            Some(manifest) => manifest
                .lines()
                .map(|l| (format!(" {}", l), String::new()))
                .collect(),
            None => match &z.detail_error {
                Some(e) => error_rows(e),
                None => vec![("".to_string(), "No manifest".to_string())],
            },
        },
        LandingZoneDetailSection::Operations => {
            if sibling_ops.is_empty() {
                return vec![(
                    "".to_string(),
                    "No landing zone operations recorded".to_string(),
                )];
            }
            let mut rows = Vec::new();
            for (i, op) in sibling_ops.iter().enumerate() {
                if i > 0 {
                    rows.push((String::new(), String::new()));
                }
                rows.push((op.operation_type.clone(), String::new())); // group header
                if let Some(s) = &op.status {
                    let val = match s.as_str() {
                        "FAILED" => format!("✗ {}", s),
                        "IN_PROGRESS" => format!("⚠ {}", s),
                        _ => format!("✓ {}", s),
                    };
                    rows.push(("  Status".to_string(), val));
                }
                if let Some(t) = &op.start_time {
                    rows.push(("  Started".to_string(), t.clone()));
                }
                if let Some(t) = &op.end_time {
                    rows.push(("  Ended".to_string(), t.clone()));
                }
                if let Some(m) = &op.status_message {
                    rows.push(("  Message".to_string(), m.clone()));
                }
                rows.push(("  Operation ID".to_string(), op.operation_identifier.clone()));
            }
            rows
        }
        LandingZoneDetailSection::Tags => tower_tag_rows(tags),
    }
}

pub(super) fn render_enabled_control_split(app: &App, c: &EnabledControl, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 3, "");
    let mut block = theme::pane_block("Enabled Control", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let (status_label, state_color) = tower_status_label(&c.status, &c.drift_status);
    let target = c
        .target_name
        .clone()
        .unwrap_or_else(|| crate::aws::services::controltower::arn_tail(&c.target_identifier).to_string());
    // "PREVENTIVE · HIGH · on ou-… (Security)" — the catalog metadata leads
    // when resolved.
    let mut subtitle = String::new();
    if let Some(b) = &c.behavior {
        subtitle.push_str(b);
        subtitle.push_str("  ·  ");
    }
    if let Some(s) = &c.severity {
        subtitle.push_str(s);
        subtitle.push_str("  ·  ");
    }
    subtitle.push_str(&format!("on {}", target));
    let header = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                c.control_name.clone(),
                Style::default().fg(theme::text_primary()).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(subtitle, Style::default().fg(theme::text_dim())),
            Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
            Span::styled(status_label, Style::default().fg(state_color)),
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
        &descriptor_tabs(app, &crate::aws::services::controltower::ENABLED_CONTROL_SECTIONS),
    );
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub fn enabled_control_section_lines(
    c: &EnabledControl,
    section: EnabledControlDetailSection,
    detail: Option<&Lazy<CtEnabledControlDetail>>,
    tags: Option<&Lazy<Vec<(String, String)>>>,
) -> Vec<(String, String)> {
    match section {
        EnabledControlDetailSection::Overview => {
            let mut rows = vec![
                ("Control".to_string(), c.control_name.clone()),
                ("Control Identifier".to_string(), c.control_identifier.clone()),
            ];
            if let Some(s) = &c.severity {
                let val = match s.as_str() {
                    "CRITICAL" | "HIGH" => format!("⚠ {}", s),
                    _ => s.clone(),
                };
                rows.push(("Severity".to_string(), val));
            }
            if let Some(b) = &c.behavior {
                rows.push(("Behavior".to_string(), b.clone()));
            }
            if let Some(i) = &c.implementation {
                rows.push(("Implementation".to_string(), i.clone()));
            }
            if let Some(d) = &c.description {
                rows.push((String::new(), String::new()));
                rows.push(("Description".to_string(), String::new())); // group header
                for line in crate::aws::services::controltower::wrap_words(d, 90) {
                    rows.push((format!(" {}", line), String::new()));
                }
            }
            rows.push((String::new(), String::new()));
            // Key-value form so the organizations-ARN classifier makes the
            // target Enter-jumpable.
            rows.push(("Target".to_string(), c.target_identifier.clone()));
            if let Some(t) = &c.target_name {
                rows.push(("Target Name".to_string(), t.clone()));
            }
            rows.push((String::new(), String::new()));
            if let Some(s) = &c.status {
                rows.push(("Status".to_string(), s.clone()));
            }
            if let Some(d) = &c.drift_status {
                rows.push(("Drift".to_string(), drift_mark(d)));
            }
            if let Some(op) = &c.last_operation_id {
                rows.push(("Last Operation".to_string(), op.clone()));
            }
            if let Some(p) = &c.parent_identifier {
                rows.push(("Parent".to_string(), p.clone()));
            }
            rows.push(("ARN".to_string(), c.arn.clone()));
            rows
        }
        EnabledControlDetailSection::Parameters => match detail {
            None | Some(Lazy::Loading) => {
                vec![("".to_string(), "Loading parameters…".to_string())]
            }
            Some(Lazy::Error(e)) => error_rows(e),
            Some(Lazy::Loaded(d)) => {
                let mut rows = Vec::new();
                if d.parameters.is_empty() {
                    rows.push(("".to_string(), "No parameters".to_string()));
                } else {
                    rows.push(("Parameters".to_string(), String::new())); // group header
                    for (k, v) in &d.parameters {
                        rows.push((format!("  {}", k), v.clone()));
                    }
                }
                if !d.target_regions.is_empty() {
                    rows.push((String::new(), String::new()));
                    rows.push(("Target Regions".to_string(), String::new())); // group header
                    for r in &d.target_regions {
                        rows.push((format!(" {}", r), String::new()));
                    }
                }
                rows
            }
        },
        EnabledControlDetailSection::Tags => tower_tag_rows(tags),
    }
}

pub(super) fn render_enabled_baseline_split(app: &App, b: &EnabledBaseline, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 3, "");
    let mut block = theme::pane_block("Enabled Baseline", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let (status_label, state_color) = tower_status_label(&b.status, &b.drift_status);
    let target = b
        .target_name
        .clone()
        .unwrap_or_else(|| crate::aws::services::controltower::arn_tail(&b.target_identifier).to_string());
    let header = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                b.display_name().to_string(),
                Style::default().fg(theme::text_primary()).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(format!("on {}", target), Style::default().fg(theme::text_dim())),
            Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
            Span::styled(status_label, Style::default().fg(state_color)),
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
        &descriptor_tabs(app, &crate::aws::services::controltower::ENABLED_BASELINE_SECTIONS),
    );
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub fn enabled_baseline_section_lines(
    b: &EnabledBaseline,
    section: EnabledBaselineDetailSection,
    detail: Option<&Lazy<CtEnabledBaselineDetail>>,
    tags: Option<&Lazy<Vec<(String, String)>>>,
) -> Vec<(String, String)> {
    match section {
        EnabledBaselineDetailSection::Overview => {
            let mut rows = vec![
                ("Baseline".to_string(), b.display_name().to_string()),
                ("Baseline Identifier".to_string(), b.baseline_identifier.clone()),
            ];
            if let Some(v) = &b.baseline_version {
                rows.push(("Version".to_string(), v.clone()));
            }
            rows.push((String::new(), String::new()));
            rows.push(("Target".to_string(), b.target_identifier.clone()));
            if let Some(t) = &b.target_name {
                rows.push(("Target Name".to_string(), t.clone()));
            }
            rows.push((String::new(), String::new()));
            if let Some(s) = &b.status {
                rows.push(("Status".to_string(), s.clone()));
            }
            if let Some(d) = &b.drift_status {
                rows.push(("Drift".to_string(), drift_mark(d)));
            }
            if let Some(p) = &b.parent_identifier {
                rows.push(("Parent".to_string(), p.clone()));
            }
            rows.push(("ARN".to_string(), b.arn.clone()));
            rows
        }
        EnabledBaselineDetailSection::Parameters => match detail {
            None | Some(Lazy::Loading) => {
                vec![("".to_string(), "Loading parameters…".to_string())]
            }
            Some(Lazy::Error(e)) => error_rows(e),
            Some(Lazy::Loaded(d)) => {
                if d.parameters.is_empty() {
                    vec![("".to_string(), "No parameters".to_string())]
                } else {
                    let mut rows = vec![("Parameters".to_string(), String::new())];
                    for (k, v) in &d.parameters {
                        rows.push((format!("  {}", k), v.clone()));
                    }
                    rows
                }
            }
        },
        EnabledBaselineDetailSection::Tags => tower_tag_rows(tags),
    }
}

pub(super) fn render_ct_account_split(app: &App, a: &CtAccount, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 3, "");
    let mut block = theme::pane_block("Account", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // The subtitle is the account's verdict at a glance: where it sits, then
    // whichever of enrolment / violations / drift is actually wrong.
    let (verdict, color) = if a.status.as_deref() == Some("SUSPENDED") {
        ("suspended".to_string(), theme::text_dim())
    } else if a.noncompliant > 0 {
        (
            format!("{} non-compliant", a.noncompliant),
            theme::error(),
        )
    } else if !a.enrolled {
        ("not enrolled".to_string(), theme::warning())
    } else if a.controls_drifted > 0 {
        (format!("{} drifted", a.controls_drifted), theme::warning())
    } else {
        ("compliant".to_string(), theme::success())
    };
    let header = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                a.account_name.clone(),
                Style::default().fg(theme::text_primary()).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("  {}", a.account_id),
                Style::default().fg(theme::text_dim()),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                a.ou_path.clone().unwrap_or_else(|| "OU unknown".to_string()),
                Style::default().fg(theme::text_dim()),
            ),
            Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
            Span::styled(verdict, Style::default().fg(color)),
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
        &descriptor_tabs(app, &crate::aws::services::controltower::CT_ACCOUNT_SECTIONS),
    );
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub fn ct_account_section_lines(
    a: &CtAccount,
    section: CtAccountDetailSection,
    controls: &[&crate::aws::services::controltower::EnabledControl],
    compliance: &[&CtCompliance],
) -> Vec<(String, String)> {
    match section {
        CtAccountDetailSection::Overview => {
            let mut rows = vec![
                ("Account".to_string(), a.account_id.clone()),
                ("Name".to_string(), a.account_name.clone()),
            ];
            if let Some(e) = &a.email {
                rows.push(("Email".to_string(), e.clone()));
            }
            if let Some(s) = &a.status {
                let val = match s.as_str() {
                    "ACTIVE" => format!("✓ {}", s),
                    _ => format!("⚠ {}", s),
                };
                rows.push(("Status".to_string(), val));
            }
            if let Some(m) = &a.joined_method {
                rows.push(("Joined".to_string(), match &a.joined_at {
                    Some(t) => format!("{} ({})", t, m),
                    None => m.clone(),
                }));
            }

            rows.push((String::new(), String::new()));
            rows.push(("Organization".to_string(), String::new())); // group header
            rows.push((
                "  OU".to_string(),
                a.ou_path.clone().unwrap_or_else(|| "unknown".to_string()),
            ));
            // Key-value form so the ou- id classifies as an Enter-jump into
            // Organizations (the "organizations" arm in arn_jump_target).
            if let Some(id) = &a.ou_id {
                rows.push(("  OU ID".to_string(), id.clone()));
            }

            rows.push((String::new(), String::new()));
            rows.push(("Governance".to_string(), String::new())); // group header
            rows.push((
                "  Enrolled".to_string(),
                if a.enrolled {
                    "✓ baseline enabled".to_string()
                } else {
                    "✗ no baseline on this account".to_string()
                },
            ));
            if let Some(s) = &a.baseline_status {
                rows.push(("  Baseline".to_string(), state_mark(s)));
            }
            if let Some(d) = &a.baseline_drift {
                rows.push(("  Baseline Drift".to_string(), drift_mark(d)));
            }
            rows.push(("  Controls".to_string(), a.controls_applied.to_string()));
            if a.controls_drifted > 0 {
                rows.push((
                    "  Controls Drifted".to_string(),
                    format!("⚠ {}", a.controls_drifted),
                ));
            }

            rows.push((String::new(), String::new()));
            rows.push(("Compliance".to_string(), String::new())); // group header
            if a.compliant + a.noncompliant == 0 {
                // Distinguishes "clean" from "the Compliance tab has no data
                // here" — which is the common case outside the audit account.
                rows.push((
                    "  · no aggregated compliance data for this account".to_string(),
                    String::new(),
                ));
            } else {
                rows.push((
                    "  Non-Compliant".to_string(),
                    if a.noncompliant > 0 {
                        format!("✗ {}", a.noncompliant)
                    } else {
                        "0".to_string()
                    },
                ));
                rows.push(("  Compliant".to_string(), a.compliant.to_string()));
            }
            rows
        }
        CtAccountDetailSection::Controls => {
            if controls.is_empty() {
                return vec![(
                    "".to_string(),
                    "No controls enabled on this account or its OU".to_string(),
                )];
            }
            let mut rows = Vec::new();
            for (i, c) in controls.iter().enumerate() {
                if i > 0 {
                    rows.push((String::new(), String::new()));
                }
                rows.push((c.control_name.clone(), String::new())); // group header
                if let Some(s) = &c.severity {
                    rows.push((
                        "  Severity".to_string(),
                        match s.as_str() {
                            "CRITICAL" | "HIGH" => format!("⚠ {}", s),
                            _ => s.clone(),
                        },
                    ));
                }
                if let Some(s) = &c.status {
                    rows.push(("  Status".to_string(), state_mark(s)));
                }
                if let Some(d) = &c.drift_status {
                    rows.push(("  Drift".to_string(), drift_mark(d)));
                }
                // Enabled on the OU vs on the account itself — the difference
                // between org policy and a one-off.
                if let Some(t) = &c.target_name {
                    rows.push(("  Target".to_string(), t.clone()));
                }
            }
            rows
        }
        CtAccountDetailSection::Compliance => {
            if compliance.is_empty() {
                return vec![(
                    "".to_string(),
                    "No aggregated compliance data for this account".to_string(),
                )];
            }
            // Violations first — the reason anyone opens this section.
            let mut sorted: Vec<&&CtCompliance> = compliance.iter().collect();
            sorted.sort_by_key(|c| c.compliance.as_deref() != Some("NON_COMPLIANT"));
            let mut rows = Vec::new();
            for (i, c) in sorted.iter().enumerate() {
                if i > 0 {
                    rows.push((String::new(), String::new()));
                }
                rows.push((
                    c.control_name.clone().unwrap_or_else(|| c.control_label.clone()),
                    String::new(),
                )); // group header
                if let Some(comp) = &c.compliance {
                    rows.push((
                        "  Compliance".to_string(),
                        match comp.as_str() {
                            "NON_COMPLIANT" => format!("✗ {}", comp),
                            "COMPLIANT" => format!("✓ {}", comp),
                            _ => comp.clone(),
                        },
                    ));
                }
                if let Some((count, capped)) = c.noncompliant_count {
                    rows.push((
                        "  Resources".to_string(),
                        if capped {
                            format!("{}+ (count capped)", count)
                        } else {
                            count.to_string()
                        },
                    ));
                }
                rows.push(("  Source".to_string(), c.source.to_string()));
                rows.push(("  Region".to_string(), c.region.clone()));
            }
            rows
        }
    }
}

/// Drift status → row text. Only `DRIFTED` is a problem; `IN_SYNC` is a clean
/// ✓, and `NOT_CHECKING` / `UNKNOWN` render as a dim annotation (the `· `
/// convention) rather than a ✓ that overstates what was actually verified.
pub(super) fn drift_mark(drift: &str) -> String {
    match drift {
        crate::aws::services::controltower::DRIFTED => format!("✗ {}", drift),
        "IN_SYNC" => format!("✓ {}", drift),
        _ => format!("· {}", drift),
    }
}

/// Control Tower status strings (`SUCCEEDED` / `FAILED` / `UNDER_CHANGE`).
pub(super) fn state_mark(status: &str) -> String {
    match status {
        "SUCCEEDED" => format!("✓ {}", status),
        s if s.contains("FAILED") => format!("✗ {}", status),
        _ => format!("⚠ {}", status),
    }
}

pub(super) fn render_ct_compliance_split(app: &App, c: &CtCompliance, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 2, "");
    let mut block = theme::pane_block("Compliance", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let (label, color) = match c.compliance.as_deref() {
        Some("NON_COMPLIANT") => ("NON_COMPLIANT".to_string(), theme::error()),
        Some("COMPLIANT") => ("COMPLIANT".to_string(), theme::success()),
        Some(other) => (other.to_string(), theme::text_dim()),
        None => ("unknown".to_string(), theme::text_dim()),
    };
    let account = c
        .account_name
        .clone()
        .unwrap_or_else(|| c.account_id.clone());
    // Second line carries the raw guardrail slug only when the catalog
    // resolved a friendlier title for the first — never the slug twice.
    let mut subtitle = vec![
        Span::raw("  "),
        Span::styled(
            format!("{} · {}", account, c.region),
            Style::default().fg(theme::text_dim()),
        ),
        Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
        Span::styled(label, Style::default().fg(color)),
    ];
    if let Some(sev) = &c.severity {
        subtitle.push(Span::styled("  ·  ", Style::default().fg(theme::text_dim())));
        subtitle.push(Span::styled(
            sev.clone(),
            Style::default().fg(severity_color(sev)),
        ));
    }
    subtitle.push(Span::styled(
        format!("  ·  {}", c.source),
        Style::default().fg(theme::text_dim()),
    ));
    let header = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                c.control_name.clone().unwrap_or_else(|| c.control_label.clone()),
                Style::default().fg(theme::text_primary()).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                match &c.control_name {
                    Some(_) => format!("  {}", c.control_label),
                    None => String::new(),
                },
                Style::default().fg(theme::text_dim()),
            ),
        ]),
        Line::from(subtitle),
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
        &descriptor_tabs(app, &crate::aws::services::controltower::CT_COMPLIANCE_SECTIONS),
    );
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub fn ct_compliance_section_lines(
    c: &CtCompliance,
    section: CtComplianceDetailSection,
    resources: Option<&Lazy<Vec<CtComplianceResource>>>,
) -> Vec<(String, String)> {
    match section {
        CtComplianceDetailSection::Overview => {
            let mut rows = vec![("Control".to_string(), c.control_label.clone())];
            if let Some(n) = &c.control_name {
                rows.push(("Name".to_string(), n.clone()));
            }
            if let Some(s) = &c.severity {
                let val = match s.as_str() {
                    "CRITICAL" | "HIGH" => format!("⚠ {}", s),
                    _ => s.clone(),
                };
                rows.push(("Severity".to_string(), val));
            }
            // Which system deployed the rule — where you'd go to change it.
            rows.push(("Source".to_string(), c.source.to_string()));
            rows.push(("Config Rule".to_string(), c.rule_name.clone()));
            if let Some(d) = &c.control_description {
                rows.push((String::new(), String::new()));
                rows.push(("Description".to_string(), String::new())); // group header
                for line in crate::aws::services::controltower::wrap_words(d, 90) {
                    rows.push((format!(" {}", line), String::new()));
                }
            }
            rows.extend([
                (String::new(), String::new()),
                ("Account".to_string(), c.account_id.clone()),
            ]);
            if let Some(n) = &c.account_name {
                rows.push(("Account Name".to_string(), n.clone()));
            }
            rows.push(("Region".to_string(), c.region.clone()));
            rows.push((String::new(), String::new()));
            if let Some(comp) = &c.compliance {
                let val = match comp.as_str() {
                    "NON_COMPLIANT" => format!("✗ {}", comp),
                    "COMPLIANT" => format!("✓ {}", comp),
                    _ => comp.clone(),
                };
                rows.push(("Compliance".to_string(), val));
            }
            if let Some((count, capped)) = c.noncompliant_count {
                let val = if capped {
                    format!("{}+ (count capped)", count)
                } else {
                    count.to_string()
                };
                rows.push(("Non-Compliant Resources".to_string(), val));
            }
            rows.push(("Aggregator".to_string(), c.aggregator.clone()));
            // Where the aggregator was read from, when that isn't the account
            // being browsed — otherwise this row's provenance is invisible.
            if let Some(src) = &c.source_account {
                rows.push(("Aggregator Account".to_string(), src.clone()));
            }
            rows
        }
        CtComplianceDetailSection::Resources => match resources {
            None | Some(Lazy::Loading) => {
                vec![("".to_string(), "Loading resources…".to_string())]
            }
            Some(Lazy::Error(e)) => error_rows(e),
            Some(Lazy::Loaded(list)) if list.is_empty() => {
                vec![("".to_string(), "No non-compliant resources".to_string())]
            }
            Some(Lazy::Loaded(list)) => {
                let mut rows = Vec::new();
                for (i, r) in list.iter().enumerate() {
                    if i > 0 {
                        rows.push((String::new(), String::new()));
                    }
                    rows.push((r.resource_type.clone(), String::new())); // group header
                    // Key-value form so id prefixes (i-, vol-, sg-, …)
                    // classify as Enter-jumpable where the type is browsable.
                    rows.push(("  Resource".to_string(), r.resource_id.clone()));
                    if let Some(a) = &r.annotation {
                        rows.push(("  Annotation".to_string(), a.clone()));
                    }
                    if let Some(t) = &r.recorded {
                        rows.push(("  Recorded".to_string(), t.clone()));
                    }
                }
                rows
            }
        },
    }
}

/// Shared status/drift → (label, color) for the Control Tower pane headers.
pub(super) fn tower_status_label(
    status: &Option<String>,
    drift: &Option<String>,
) -> (String, ratatui::style::Color) {
    match (status.as_deref(), drift.as_deref()) {
        (_, Some("DRIFTED")) => ("DRIFTED".to_string(), theme::error()),
        (Some("FAILED"), _) => ("FAILED".to_string(), theme::error()),
        (Some("UNDER_CHANGE"), _) => ("UNDER_CHANGE".to_string(), theme::warning()),
        (Some(s), _) => (s.to_string(), theme::success()),
        (None, _) => ("unknown".to_string(), theme::text_dim()),
    }
}

/// Shared Tags-section rows for the three Control Tower panes (one lazy map).
pub(super) fn tower_tag_rows(tags: Option<&Lazy<Vec<(String, String)>>>) -> Vec<(String, String)> {
    match tags {
        None | Some(Lazy::Loading) => vec![("".to_string(), "Loading tags…".to_string())],
        Some(Lazy::Error(e)) => error_rows(e),
        Some(Lazy::Loaded(list)) if list.is_empty() => {
            vec![("".to_string(), "No tags".to_string())]
        }
        Some(Lazy::Loaded(list)) => list.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
    }
}
