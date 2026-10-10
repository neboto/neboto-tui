use super::*;

// ── Network Firewall: firewall split pane ───────────────────────────────────────

pub(super) fn render_nfw_firewall_split(app: &App, fw: &NfwFirewall, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 5, "");
    let mut block = theme::pane_block("Firewall", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let (status_color, status_label) = match fw.status.as_str() {
        "READY" => (theme::success(), "READY".to_string()),
        "PROVISIONING" => (theme::warning(), "PROVISIONING".to_string()),
        "DELETING" => (theme::error(), "DELETING".to_string()),
        other => (theme::text_dim(), other.to_string()),
    };
    let header = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                fw.name.clone(),
                Style::default().fg(crate::ui::theme::text_primary()).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("● ", Style::default().fg(status_color)),
            Span::styled(status_label, Style::default().fg(status_color)),
            Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
            Span::styled(fw.vpc_id.clone(), Style::default().fg(theme::text_dim())),
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
        &descriptor_tabs(app, &crate::aws::services::network_firewall::NFW_FIREWALL_SECTIONS),
    );
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub fn nfw_firewall_section_lines(
    fw: &NfwFirewall,
    section: NfwFirewallDetailSection,
    logging: Option<&crate::lazy::Lazy<Vec<crate::aws::services::network_firewall::NfwLogDestination>>>,
) -> Vec<(String, String)> {
    match section {
        NfwFirewallDetailSection::Details => {
            let mut rows = vec![
                ("Name".to_string(), fw.name.clone()),
                ("Status".to_string(), fw.status.clone()),
                ("Config Sync".to_string(), {
                    if fw.configuration_sync.is_empty() {
                        "—".to_string()
                    } else {
                        fw.configuration_sync.clone()
                    }
                }),
                ("VPC".to_string(), fw.vpc_id.clone()),
            ];
            if !fw.description.is_empty() {
                rows.push(("Description".to_string(), fw.description.clone()));
            }
            rows.push((
                "Delete Protection".to_string(),
                if fw.delete_protection { "✓ on".to_string() } else { "✗ off".to_string() },
            ));
            rows.push((
                "Policy Change Prot.".to_string(),
                if fw.policy_change_protection { "✓ on".to_string() } else { "✗ off".to_string() },
            ));
            rows.push((
                "Subnet Change Prot.".to_string(),
                if fw.subnet_change_protection { "✓ on".to_string() } else { "✗ off".to_string() },
            ));
            rows.push((String::new(), String::new()));
            rows.push(("ARN".to_string(), fw.arn.clone()));
            // Per-AZ sync state — the operational signal.
            if !fw.sync_states.is_empty() {
                rows.push((String::new(), String::new()));
                rows.push(("Per-AZ Sync".to_string(), String::new())); // group header
                for s in &fw.sync_states {
                    let marker = if s.status.eq_ignore_ascii_case("READY") {
                        "✓"
                    } else {
                        "⚠"
                    };
                    rows.push((
                        format!("  {}", s.availability_zone),
                        format!(
                            "{} {}  ({} · {})",
                            marker, s.status, s.subnet_id, s.endpoint_id
                        ),
                    ));
                }
            }
            rows
        }
        NfwFirewallDetailSection::Subnets => {
            if fw.subnet_mappings.is_empty() {
                return vec![("".to_string(), "No subnet mappings".to_string())];
            }
            let mut rows = vec![(
                format!("Subnet Mappings ({})", fw.subnet_mappings.len()),
                String::new(),
            )];
            rows.push((String::new(), String::new()));
            for s in &fw.subnet_mappings {
                // subnet- ids are jump anchors (handled by resource_jump_target).
                rows.push(("Subnet".to_string(), s.clone()));
            }
            rows
        }
        NfwFirewallDetailSection::Policy => {
            vec![
                ("Firewall Policy".to_string(), String::new()), // group header
                (String::new(), String::new()),
                // The policy ARN is a same-service jump anchor (→ Policies tab).
                ("ARN".to_string(), fw.policy_arn.clone()),
            ]
        }
        NfwFirewallDetailSection::Logging => {
            let mut rows = vec![("".to_string(), "".to_string())];
            match logging {
                None | Some(crate::lazy::Lazy::Loading) => {
                    rows.push(("  Loading…".to_string(), "".to_string()));
                }
                Some(crate::lazy::Lazy::Error(e)) => {
                    rows.extend(error_rows(e));
                }
                Some(crate::lazy::Lazy::Loaded(dests)) if dests.is_empty() => {
                    rows.push((
                        "  Logging is not enabled on this firewall.".to_string(),
                        "".to_string(),
                    ));
                    rows.push(("".to_string(), "".to_string()));
                    rows.push((
                        "  Enable FLOW/ALERT logging to a CloudWatch log group to tail (t).".to_string(),
                        "".to_string(),
                    ));
                }
                Some(crate::lazy::Lazy::Loaded(dests)) => {
                    rows.push(("Log Destinations".to_string(), "".to_string())); // group header
                    rows.push(("".to_string(), "".to_string()));
                    for d in dests {
                        // FLOW / ALERT / TLS → where it goes.
                        rows.push((format!("  {}", d.log_type), d.destination_type.clone()));
                        if !d.target.is_empty() {
                            rows.push(("    target".to_string(), d.target.clone()));
                        }
                    }
                    let has_cwl = dests.iter().any(|d| d.destination_type == "CloudWatchLogs");
                    rows.push(("".to_string(), "".to_string()));
                    rows.push((
                        if has_cwl {
                            "  Press t to tail the CloudWatch logs (ALERT preferred).".to_string()
                        } else {
                            "  No CloudWatch destination — t can't tail S3/Firehose logs.".to_string()
                        },
                        "".to_string(),
                    ));
                }
            }
            rows
        }
        NfwFirewallDetailSection::Tags => tag_rows(&fw.tags),
    }
}

// ── Network Firewall: policy split pane ─────────────────────────────────────────

pub(super) fn render_nfw_policy_split(app: &App, policy: &NfwPolicy, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 3, "");
    let mut block = theme::pane_block("Firewall Policy", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let header = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                policy.name.clone(),
                Style::default().fg(crate::ui::theme::text_primary()).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                format!(
                    "{} stateful · {} stateless rule groups",
                    policy.stateful_rule_groups.len(),
                    policy.stateless_rule_groups.len()
                ),
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
        &descriptor_tabs(app, &crate::aws::services::network_firewall::NFW_POLICY_SECTIONS),
    );
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub fn nfw_policy_section_lines(
    policy: &NfwPolicy,
    section: NfwPolicyDetailSection,
) -> Vec<(String, String)> {
    match section {
        NfwPolicyDetailSection::Stateful => {
            let mut rows = vec![("Stateful Default Actions".to_string(), String::new())]; // header
            if policy.stateful_default_actions.is_empty() {
                rows.push((
                    "  (engine default)".to_string(),
                    String::new(),
                ));
            } else {
                for a in &policy.stateful_default_actions {
                    rows.push((format!("  {}", a), String::new()));
                }
            }
            rows.push((String::new(), String::new()));
            rows.push((
                format!("Rule Groups ({})", policy.stateful_rule_groups.len()),
                String::new(),
            )); // header
            if policy.stateful_rule_groups.is_empty() {
                rows.push(("  (none)".to_string(), String::new()));
            } else {
                rows.push((String::new(), String::new()));
                for r in &policy.stateful_rule_groups {
                    let prio = r
                        .priority
                        .map(|p| format!("priority {}", p))
                        .unwrap_or_else(|| "strict-order".to_string());
                    rows.push((format!("{} ·", prio), String::new()));
                    // ARN is a same-service jump anchor (→ Rule Groups tab).
                    rows.push(("  ARN".to_string(), r.arn.clone()));
                }
            }
            rows
        }
        NfwPolicyDetailSection::Stateless => {
            let mut rows = vec![("Stateless Default Actions".to_string(), String::new())]; // header
            if policy.stateless_default_actions.is_empty() {
                rows.push(("  (none)".to_string(), String::new()));
            } else {
                for a in &policy.stateless_default_actions {
                    rows.push((format!("  {}", a), String::new()));
                }
            }
            if !policy.stateless_fragment_default_actions.is_empty() {
                rows.push((String::new(), String::new()));
                rows.push(("Fragment Default Actions".to_string(), String::new())); // header
                for a in &policy.stateless_fragment_default_actions {
                    rows.push((format!("  {}", a), String::new()));
                }
            }
            rows.push((String::new(), String::new()));
            rows.push((
                format!("Rule Groups ({})", policy.stateless_rule_groups.len()),
                String::new(),
            )); // header
            if policy.stateless_rule_groups.is_empty() {
                rows.push(("  (none)".to_string(), String::new()));
            } else {
                rows.push((String::new(), String::new()));
                for r in &policy.stateless_rule_groups {
                    let prio = r
                        .priority
                        .map(|p| format!("priority {}", p))
                        .unwrap_or_else(|| "—".to_string());
                    rows.push((format!("{} ·", prio), String::new()));
                    rows.push(("  ARN".to_string(), r.arn.clone()));
                }
            }
            rows
        }
        NfwPolicyDetailSection::Tags => tag_rows(&policy.tags),
    }
}

// ── Network Firewall: rule group split pane ─────────────────────────────────────

pub(super) fn render_nfw_rule_group_split(app: &App, rg: &NfwRuleGroup, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 2, "");
    let mut block = theme::pane_block("NFW Rule Group", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let kind_label = if rg.kind.is_empty() { "—".to_string() } else { rg.kind.clone() };
    let header = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                rg.name.clone(),
                Style::default().fg(crate::ui::theme::text_primary()).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(kind_label, Style::default().fg(theme::text_dim())),
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
        &descriptor_tabs(app, &crate::aws::services::network_firewall::NFW_RULE_GROUP_SECTIONS),
    );
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub fn nfw_rule_group_section_lines(
    rg: &NfwRuleGroup,
    section: NfwRuleGroupDetailSection,
    rules: Option<&crate::lazy::Lazy<crate::aws::services::network_firewall::NfwRuleGroupRules>>,
) -> Vec<(String, String)> {
    match section {
        NfwRuleGroupDetailSection::Rules => match rules {
            None | Some(crate::lazy::Lazy::Loading) => {
                vec![("".to_string(), "Loading rules…".to_string())]
            }
            Some(crate::lazy::Lazy::Error(e)) => {
                let mut rows = error_rows(e);
                // Add wrapped display lines for readability.
                rows.push((String::new(), String::new()));
                for line in wrap_plain(e, 70) {
                    rows.push((format!("  {}", line), String::new()));
                }
                rows
            }
            Some(crate::lazy::Lazy::Loaded(r)) => {
                let mut rows = vec![
                    (
                        "Type".to_string(),
                        if r.kind.is_empty() { rg.kind.clone() } else { r.kind.clone() },
                    ),
                    ("Capacity".to_string(), r.capacity.to_string()),
                ];
                if !r.description.is_empty() {
                    rows.push(("Description".to_string(), r.description.clone()));
                }
                rows.push((String::new(), String::new()));

                // Suricata-style raw rules text → summary + pointer to `v`.
                if let Some(text) = &r.rules_string {
                    let n = text.lines().filter(|l| !l.trim().is_empty()).count();
                    rows.push(("Suricata Rules".to_string(), String::new())); // header
                    rows.push((
                        format!("  {} rule line(s) — e to edit", n),
                        String::new(),
                    ));
                    rows.push((String::new(), String::new()));
                    // Preview the first few lines inline.
                    for line in text.lines().take(12) {
                        rows.push((format!("  {}", line), String::new()));
                    }
                    if n > 12 {
                        rows.push(("  …".to_string(), String::new()));
                    }
                }

                // Structured stateful 5-tuple rules → table.
                if !r.stateful_rules.is_empty() {
                    rows.push((
                        format!("5-Tuple Rules ({})", r.stateful_rules.len()),
                        String::new(),
                    )); // header
                    rows.push((String::new(), String::new()));
                    rows.push((
                        format!(
                            "  {:<8} {:<6} {:<18} {:<6} {:<4} {:<18} {:<6}",
                            "ACTION", "PROTO", "SOURCE", "SPORT", "DIR", "DEST", "DPORT"
                        ),
                        String::new(),
                    ));
                    for sr in &r.stateful_rules {
                        rows.push((
                            format!(
                                "  {:<8} {:<6} {:<18} {:<6} {:<4} {:<18} {:<6}",
                                truncate_cell(&sr.action, 8),
                                truncate_cell(&sr.protocol, 6),
                                truncate_cell(&sr.source, 18),
                                truncate_cell(&sr.source_port, 6),
                                truncate_cell(&sr.direction, 4),
                                truncate_cell(&sr.destination, 18),
                                truncate_cell(&sr.destination_port, 6),
                            ),
                            String::new(),
                        ));
                        if !sr.options.is_empty() {
                            rows.push((format!("    {}", sr.options), String::new()));
                        }
                    }
                }

                // Domain allow/deny list.
                if !r.domain_targets.is_empty() {
                    rows.push((
                        format!("Domain List ({})", r.domain_targets.len()),
                        String::new(),
                    )); // header
                    if let Some(t) = &r.domain_rules_type {
                        rows.push(("  Type".to_string(), t.clone()));
                    }
                    rows.push((String::new(), String::new()));
                    for d in &r.domain_targets {
                        rows.push((format!("  {}", d), String::new()));
                    }
                }

                // Stateless rule count (full expansion is out of scope v1).
                if r.stateless_rule_count > 0 {
                    rows.push((
                        "Stateless Rules".to_string(),
                        format!("{} rule(s)", r.stateless_rule_count),
                    ));
                }

                if r.rules_string.is_none()
                    && r.stateful_rules.is_empty()
                    && r.domain_targets.is_empty()
                    && r.stateless_rule_count == 0
                {
                    rows.push((String::new(), String::new()));
                    rows.push(("  (no rules in this group)".to_string(), String::new()));
                }
                rows
            }
        },
        NfwRuleGroupDetailSection::Tags => match rules {
            Some(crate::lazy::Lazy::Loaded(r)) => tag_rows(&r.tags),
            None | Some(crate::lazy::Lazy::Loading) => {
                vec![("".to_string(), "Loading tags…".to_string())]
            }
            Some(crate::lazy::Lazy::Error(e)) => {
                // Show why the detail fetch failed, then fall back to the
                // tags already present on the list summary.
                let mut rows = error_rows(e);
                rows.push((String::new(), String::new()));
                rows.extend(tag_rows(&rg.tags));
                rows
            }
        },
    }
}

/// Truncate a fixed-width table cell (keeps the 5-tuple table aligned).
pub(super) fn truncate_cell(s: &str, max: usize) -> String {
    if s.chars().count() > max {
        s.chars().take(max.saturating_sub(1)).collect::<String>() + "…"
    } else {
        s.to_string()
    }
}
