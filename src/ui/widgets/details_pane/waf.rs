use super::*;

// ── WAF Web ACL split pane ────────────────────────────────────────────────────

pub(super) fn render_waf_web_acl_split(app: &App, acl: &WafWebAcl, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 6, "");

    let mut block = theme::pane_block("WAF Web ACL", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let header_lines = build_waf_web_acl_header_lines(acl);
    let header_lines = one_row_header(app, header_lines, area.width.saturating_sub(2) as usize);
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
    render_waf_section_tabs(app, chunks[2], frame);
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub(super) fn build_waf_web_acl_header_lines(acl: &WafWebAcl) -> Vec<Line<'static>> {
    let mut lines = vec![Line::raw("")];
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            acl.name.clone(),
            Style::default().fg(crate::ui::theme::text_primary()).add_modifier(Modifier::BOLD),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(acl.id.clone(), Style::default().fg(theme::text_dim())),
        Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
        Span::styled(acl.scope.clone(), Style::default().fg(theme::text_dim())),
    ]));
    lines.push(Line::raw(""));
    lines
}

pub(super) fn render_waf_section_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let tabs = descriptor_tabs(app, &crate::aws::services::waf::WAF_WEB_ACL_SECTIONS);
    render_section_tab_bar(app, area, frame, &tabs);
}

pub fn waf_web_acl_section_lines(
    acl: &WafWebAcl,
    section: WafWebAclDetailSection,
    traffic: Option<&Lazy<Box<WafTraffic>>>,
    detail: Option<&crate::lazy::Lazy<Box<crate::aws::services::waf::WafWebAclDetail>>>,
    insights: Option<&Lazy<Box<WafInsights>>>,
) -> Vec<(String, String)> {
    match section {
        WafWebAclDetailSection::Overview => waf_details_lines(acl),
        WafWebAclDetailSection::Traffic => waf_traffic_lines(traffic),
        WafWebAclDetailSection::Insights => waf_insights_lines(insights),
        WafWebAclDetailSection::Rules => {
            let mut rows = vec![("".to_string(), "".to_string())];
            match detail {
                None | Some(crate::lazy::Lazy::Loading) => {
                    rows.push(("  Loading…".to_string(), "".to_string()))
                }
                Some(crate::lazy::Lazy::Error(e)) => {
                    rows.extend(error_rows(e))
                }
                Some(crate::lazy::Lazy::Loaded(d)) => {
                    rows.push(("Default Action".to_string(), d.default_action.clone()));
                    rows.push(("Capacity (WCU)".to_string(), d.capacity.to_string()));
                    rows.push(("".to_string(), "".to_string()));
                    rows.push((format!("Rules ({})", d.rules.len()), "".to_string()));
                    rows.push(("".to_string(), "".to_string()));
                    if d.rules.is_empty() {
                        rows.push(("  (no rules)".to_string(), "".to_string()));
                    }
                    for r in &d.rules {
                        rows.push((
                            r.name.clone(),
                            format!("{}  ·  {}  ·  priority {}", r.action, r.kind, r.priority),
                        ));
                    }
                }
            }
            rows
        }
        WafWebAclDetailSection::Associated => {
            let mut rows = vec![("".to_string(), "".to_string())];
            match detail {
                None | Some(crate::lazy::Lazy::Loading) => {
                    rows.push(("  Loading…".to_string(), "".to_string()))
                }
                Some(crate::lazy::Lazy::Error(e)) => {
                    rows.extend(error_rows(e))
                }
                Some(crate::lazy::Lazy::Loaded(d)) => {
                    if let Some(note) = &d.associated_note {
                        rows.push((format!("  {}", note), "".to_string()));
                    } else if d.associated.is_empty() {
                        rows.push(("  Not associated with any resource.".to_string(), "".to_string()));
                    } else {
                        rows.push((
                            format!("Protected Resources ({})", d.associated.len()),
                            "".to_string(),
                        ));
                        rows.push(("".to_string(), "".to_string()));
                        for a in &d.associated {
                            // ARN value → Enter jumps via arn_jump_target.
                            rows.push(("Resource".to_string(), a.clone()));
                        }
                    }
                }
            }
            rows
        }
        WafWebAclDetailSection::Logging => {
            let mut rows = vec![("".to_string(), "".to_string())];
            match detail {
                None | Some(crate::lazy::Lazy::Loading) => {
                    rows.push(("  Loading…".to_string(), "".to_string()))
                }
                Some(crate::lazy::Lazy::Error(e)) => {
                    rows.extend(error_rows(e))
                }
                Some(crate::lazy::Lazy::Loaded(d)) => match &d.logging {
                    None => rows.push(("  Logging is not enabled.".to_string(), "".to_string())),
                    Some(l) => {
                        rows.push((
                            "Managed by Firewall Mgr".to_string(),
                            if l.managed_by_fm { "yes".to_string() } else { "no".to_string() },
                        ));
                        rows.push(("Redacted Fields".to_string(), l.redacted_fields.to_string()));
                        rows.push(("".to_string(), "".to_string()));
                        rows.push((format!("Destinations ({})", l.destinations.len()), "".to_string()));
                        rows.push(("".to_string(), "".to_string()));
                        for dst in &l.destinations {
                            rows.push(("Destination".to_string(), dst.clone()));
                        }
                    }
                },
            }
            rows
        }
    }
}

// ── WAF IP set / rule group split panes ─────────────────────────────────────

/// Shared two/three-section split-pane chrome for WAF IP sets & rule groups.
pub(super) fn render_waf_simple_split(
    app: &App,
    title: &str,
    name: &str,
    id: &str,
    scope: &str,
    tabs: &[(char, &str, bool)],
    area: Rect,
    frame: &mut Frame,
) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, tabs.len(), "");
    let mut block = theme::pane_block(title, focused);
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
                name.to_string(),
                Style::default().fg(crate::ui::theme::text_primary()).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(id.to_string(), Style::default().fg(theme::text_dim())),
            Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
            Span::styled(scope.to_string(), Style::default().fg(theme::text_dim())),
        ]),
        Line::raw(""),
    ];
    let header = one_row_header(app, header, area.width.saturating_sub(2) as usize);
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
    render_section_tab_bar(app, chunks[2], frame, tabs);
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub(super) fn render_waf_ip_set_split(
    app: &App,
    s: &crate::aws::services::waf::WafIpSet,
    area: Rect,
    frame: &mut Frame,
) {
    render_waf_simple_split(
        app,
        "WAF IP Set",
        &s.name,
        &s.id,
        &s.scope,
        &descriptor_tabs(app, &crate::aws::services::waf::WAF_IP_SET_SECTIONS),
        area,
        frame,
    );
}

pub(super) fn render_waf_rule_group_split(
    app: &App,
    s: &crate::aws::services::waf::WafRuleGroup,
    area: Rect,
    frame: &mut Frame,
) {
    render_waf_simple_split(
        app,
        "WAF Rule Group",
        &s.name,
        &s.id,
        &s.scope,
        &descriptor_tabs(app, &crate::aws::services::waf::WAF_RULE_GROUP_SECTIONS),
        area,
        frame,
    );
}

pub fn waf_ip_set_section_lines(
    s: &crate::aws::services::waf::WafIpSet,
    section: WafIpSetDetailSection,
    state: Option<&crate::lazy::Lazy<Box<crate::aws::services::waf::WafIpSetDetail>>>,
) -> Vec<(String, String)> {
    match section {
        WafIpSetDetailSection::Overview => {
            let mut rows = vec![
                ("Name".to_string(), s.name.clone()),
                ("ID".to_string(), s.id.clone()),
                ("ARN".to_string(), s.arn.clone()),
                ("Scope".to_string(), s.scope.clone()),
            ];
            if !s.description.is_empty() {
                rows.push(("Description".to_string(), s.description.clone()));
            }
            if let Some(crate::lazy::Lazy::Loaded(d)) = state {
                rows.push(("IP Version".to_string(), d.ip_address_version.clone()));
                rows.push(("Addresses".to_string(), d.addresses.len().to_string()));
            }
            rows
        }
        WafIpSetDetailSection::Addresses => {
            let mut rows = vec![("".to_string(), "".to_string())];
            match state {
                None | Some(crate::lazy::Lazy::Loading) => {
                    rows.push(("  Loading…".to_string(), "".to_string()))
                }
                Some(crate::lazy::Lazy::Error(e)) => rows.extend(error_rows(e)),
                Some(crate::lazy::Lazy::Loaded(d)) if d.addresses.is_empty() => {
                    rows.push(("  (no addresses)".to_string(), "".to_string()))
                }
                Some(crate::lazy::Lazy::Loaded(d)) => {
                    rows.push((format!("Addresses ({})", d.addresses.len()), "".to_string()));
                    rows.push(("".to_string(), "".to_string()));
                    for a in &d.addresses {
                        rows.push((format!("  {}", a), "".to_string()));
                    }
                }
            }
            rows
        }
        WafIpSetDetailSection::Tags => match state {
            None | Some(crate::lazy::Lazy::Loading) => {
                vec![("  Loading…".to_string(), "".to_string())]
            }
            Some(crate::lazy::Lazy::Error(e)) => error_rows(e),
            Some(crate::lazy::Lazy::Loaded(d)) => bundle_tag_rows(&d.tags, d.tags_error.as_deref()),
        },
    }
}

pub fn waf_rule_group_section_lines(
    s: &crate::aws::services::waf::WafRuleGroup,
    section: WafRuleGroupDetailSection,
    state: Option<&crate::lazy::Lazy<Box<crate::aws::services::waf::WafRuleGroupDetail>>>,
) -> Vec<(String, String)> {
    match section {
        WafRuleGroupDetailSection::Overview => {
            let mut rows = vec![
                ("Name".to_string(), s.name.clone()),
                ("ID".to_string(), s.id.clone()),
                ("ARN".to_string(), s.arn.clone()),
                ("Scope".to_string(), s.scope.clone()),
            ];
            if !s.description.is_empty() {
                rows.push(("Description".to_string(), s.description.clone()));
            }
            if let Some(crate::lazy::Lazy::Loaded(d)) = state {
                rows.push(("Capacity (WCU)".to_string(), d.capacity.to_string()));
                rows.push(("Rules".to_string(), d.rules.len().to_string()));
            }
            rows
        }
        WafRuleGroupDetailSection::Rules => {
            let mut rows = vec![("".to_string(), "".to_string())];
            match state {
                None | Some(crate::lazy::Lazy::Loading) => {
                    rows.push(("  Loading…".to_string(), "".to_string()))
                }
                Some(crate::lazy::Lazy::Error(e)) => rows.extend(error_rows(e)),
                Some(crate::lazy::Lazy::Loaded(d)) if d.rules.is_empty() => {
                    rows.push(("  (no rules)".to_string(), "".to_string()))
                }
                Some(crate::lazy::Lazy::Loaded(d)) => {
                    rows.push((format!("Rules ({})", d.rules.len()), "".to_string()));
                    rows.push(("".to_string(), "".to_string()));
                    for r in &d.rules {
                        rows.push((
                            r.name.clone(),
                            format!("{}  ·  priority {}", r.action, r.priority),
                        ));
                    }
                }
            }
            rows
        }
        WafRuleGroupDetailSection::Tags => match state {
            None | Some(crate::lazy::Lazy::Loading) => {
                vec![("  Loading…".to_string(), "".to_string())]
            }
            Some(crate::lazy::Lazy::Error(e)) => error_rows(e),
            Some(crate::lazy::Lazy::Loaded(d)) => bundle_tag_rows(&d.tags, d.tags_error.as_deref()),
        },
    }
}

pub(super) fn waf_details_lines(acl: &WafWebAcl) -> Vec<(String, String)> {
    vec![
        ("Name".to_string(), acl.name.clone()),
        ("ID".to_string(), acl.id.clone()),
        ("ARN".to_string(), acl.arn.clone()),
        ("Scope".to_string(), acl.scope.clone()),
        ("".to_string(), "".to_string()),
        ("Description".to_string(), acl.description.clone()),
        ("".to_string(), "".to_string()),
    ]
}

pub(super) fn waf_traffic_lines(traffic: Option<&Lazy<Box<WafTraffic>>>) -> Vec<(String, String)> {
    let mut rows = vec![("".to_string(), "".to_string())];
    let t = match traffic {
        None | Some(Lazy::Loading) => {
            rows.push(("  Loading…".to_string(), "".to_string()));
            return rows;
        }
        Some(Lazy::Error(e)) => {
            rows.extend(error_rows(e));
            return rows;
        }
        Some(Lazy::Loaded(t)) => t,
    };
    // Count doesn't end a request, so the percentages are of the requests
    // that did end here (`WafTraffic::requests`) and Counted has none.
    let requests = t.requests();
    rows.push(("Last 24 hours".to_string(), "".to_string()));
    if requests <= 0.0 && t.totals.counted <= 0.0 {
        rows.push(("".to_string(), "No requests in the last 24 hours".to_string()));
    } else {
        rows.push(("Requests".to_string(), format_count(requests)));
        let tt = &t.totals;
        for (name, v) in [("Allowed", tt.allowed), ("Blocked", tt.blocked), ("CAPTCHA", tt.captcha), ("Challenge", tt.challenge)] {
            // Allowed and Blocked always; the rest only when a rule uses them.
            if v > 0.0 || name == "Allowed" || name == "Blocked" {
                rows.push((name.to_string(), format!("{}  ({})", format_count(v), crate::aws::services::waf::pct(v, requests))));
            }
        }
        if tt.counted > 0.0 {
            rows.push(("Counted".to_string(), format!("{}  (also allowed or blocked later)", format_count(tt.counted))));
        }
    }
    rows.push(("".to_string(), "· m charts them over any range, with these tables beside the chart".to_string()));
    // The tables the `m` view draws, from the same formatter.
    const MAX_ROWS: usize = 30;
    for table in t.tables() {
        rows.push(("".to_string(), "".to_string()));
        rows.push((table.title.clone(), "".to_string()));
        if table.rows.is_empty() {
            rows.push(("".to_string(), "No per-rule traffic in the last 24 hours".to_string()));
            continue;
        }
        let (header, lines) = table.text(100, MAX_ROWS);
        rows.push((header, "".to_string()));
        rows.extend(lines.into_iter().map(|l| (l, "".to_string())));
        if table.rows.len() > MAX_ROWS {
            rows.push(("".to_string(), format!("· {} more", table.rows.len() - MAX_ROWS)));
        }
    }
    // AWS doesn't publish these to the web ACL, so no table can show them.
    rows.push(("".to_string(), "".to_string()));
    rows.push((
        "".to_string(),
        "· Rows are rules' metric names; Count matches inside managed rule groups aren't published here".to_string(),
    ));
    rows
}

pub(super) fn format_count(v: f64) -> String {
    crate::aws::metrics::fmt_count(v)
}

pub(super) fn waf_insights_lines(
    state: Option<&Lazy<Box<WafInsights>>>,
) -> Vec<(String, String)> {
    let mut rows = vec![("".to_string(), "".to_string())];
    match state {
        None | Some(Lazy::Loading) => {
            rows.push(("  Aggregating sampled requests…".to_string(), "".to_string()));
            return rows;
        }
        Some(Lazy::Error(e)) => {
            rows.extend(error_rows(e));
            return rows;
        }
        Some(Lazy::Loaded(d)) => {
            rows.push(("Sampled Requests (last 3 hours)".to_string(), "".to_string()));
            rows.push(("".to_string(), "".to_string()));
            rows.push((
                "Population".to_string(),
                format!(
                    "{} requests · {} samples · {} samplers",
                    format_count(d.population as f64),
                    d.samples,
                    d.samplers
                ),
            ));
            if d.skipped_rules > 0 {
                rows.push((
                    format!(
                        "  ⚠ {} lower-priority rules not sampled (cap {})",
                        d.skipped_rules,
                        crate::aws::services::waf::MAX_INSIGHT_SAMPLERS
                    ),
                    "".to_string(),
                ));
            }
            if d.sampling_disabled > 0 {
                rows.push((
                    format!("  {} rules have sampled requests disabled", d.sampling_disabled),
                    "".to_string(),
                ));
            }
            if d.failed_samplers > 0 {
                rows.push((
                    format!("  ⚠ {} samplers failed", d.failed_samplers),
                    "".to_string(),
                ));
            }
            if d.samples == 0 {
                rows.push(("".to_string(), "".to_string()));
                rows.push((
                    "  No sampled requests in the last 3 hours.".to_string(),
                    "".to_string(),
                ));
                return rows;
            }

            let total: u64 = d.actions.iter().map(|(_, c)| c).sum::<u64>().max(1);
            rows.push(("".to_string(), "".to_string()));
            rows.push(("Actions".to_string(), "".to_string()));
            for (action, count) in &d.actions {
                rows.push((
                    action.clone(),
                    format!(
                        "{} ({:.1}%)",
                        format_count(*count as f64),
                        *count as f64 / total as f64 * 100.0
                    ),
                ));
            }

            let mut push_top = |title: &str, list: &[crate::aws::services::waf::WafInsightRow]| {
                if list.is_empty() {
                    return;
                }
                rows.push(("".to_string(), "".to_string()));
                rows.push((title.to_string(), "".to_string()));
                for r in list {
                    let key: String = if r.key.chars().count() > 70 {
                        format!("{}…", r.key.chars().take(69).collect::<String>())
                    } else {
                        r.key.clone()
                    };
                    let blocked = if r.blocked > 0 {
                        format!(" · {} blocked", format_count(r.blocked as f64))
                    } else {
                        String::new()
                    };
                    rows.push((
                        format!(
                            "  {} — {} ({:.1}%){}",
                            key,
                            format_count(r.count as f64),
                            r.count as f64 / total as f64 * 100.0,
                            blocked
                        ),
                        "".to_string(),
                    ));
                }
            };
            push_top("Top Rules", &d.top_rules);
            push_top("Top Client IPs", &d.top_ips);
            push_top("Top Countries", &d.top_countries);
            push_top("Top URIs", &d.top_uris);
            push_top("Top User Agents", &d.top_agents);
            push_top("Top Labels", &d.top_labels);

            rows.push(("".to_string(), "".to_string()));
            rows.push((
                "  Weighted from GetSampledRequests — indicative, not exact counts.".to_string(),
                "".to_string(),
            ));
        }
    }
    rows.push(("".to_string(), "".to_string()));
    rows
}
