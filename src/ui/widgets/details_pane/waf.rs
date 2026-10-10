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
    metrics: Option<&WafMetricsState>,
    detail: Option<&crate::lazy::Lazy<Box<crate::aws::services::waf::WafWebAclDetail>>>,
    insights: Option<&Lazy<Box<WafInsights>>>,
) -> Vec<(String, String)> {
    match section {
        WafWebAclDetailSection::Details => waf_details_lines(acl),
        WafWebAclDetailSection::Traffic => waf_traffic_lines(metrics),
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
        WafIpSetDetailSection::Details => {
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
        WafRuleGroupDetailSection::Details => {
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

pub(super) fn waf_traffic_lines(metrics: Option<&WafMetricsState>) -> Vec<(String, String)> {
    match metrics {
        None | Some(WafMetricsState::Loading) => {
            vec![("".to_string(), "Loading traffic metrics…".to_string())]
        }
        Some(WafMetricsState::Error(e)) => {
            let mut rows = vec![("".to_string(), "".to_string())];
            rows.extend(error_rows(e));
            rows
        }
        Some(WafMetricsState::Loaded(data)) => {
            let sum = |pts: &[(f64, f64)]| -> f64 { pts.iter().map(|(_, v)| v).sum() };
            let total_allowed = sum(&data.allowed);
            let total_blocked = sum(&data.blocked);
            let total_counted = sum(&data.counted);
            let total_captcha = sum(&data.captcha);
            let total_challenge = sum(&data.challenge);
            let total =
                total_allowed + total_blocked + total_counted + total_captcha + total_challenge;

            let pct = |v: f64| -> String {
                if total > 0.0 {
                    format!("{:.1}%", v / total * 100.0)
                } else {
                    "—".to_string()
                }
            };

            let mut rows = vec![
                ("Traffic Summary".to_string(), "".to_string()),
                ("".to_string(), "".to_string()),
            ];

            // Header
            rows.push((
                " Metric                  Count         % of Total".to_string(),
                "".to_string(),
            ));
            rows.push((
                " ─────────────────────────────────────────────────".to_string(),
                "".to_string(),
            ));
            for (label, v) in [
                ("Allowed Requests", total_allowed),
                ("Blocked Requests", total_blocked),
                ("Counted Requests", total_counted),
                ("CAPTCHA Requests", total_captcha),
                ("Challenge Requests", total_challenge),
            ] {
                rows.push((
                    format!(" {:<23} {:<14}{}", label, format_count(v), pct(v)),
                    "".to_string(),
                ));
            }
            rows.push((
                " ─────────────────────────────────────────────────".to_string(),
                "".to_string(),
            ));
            rows.push((
                format!(
                    " Total                   {}",
                    format_count(total)
                ),
                "".to_string(),
            ));

            rows.push(("".to_string(), "".to_string()));
            rows.push((
                format!("Period: {} │ press m for full chart overlay", data.time_range.label()),
                "".to_string(),
            ));
            rows.push(("".to_string(), "".to_string()));

            // Per-rule range totals (managed rule-group rollups + Default_Action
            // included) — the console dashboard's "top rules" table.
            if let Some(note) = &data.rule_note {
                rows.push((format!(" ⚠ {}", note), "".to_string()));
                rows.push(("".to_string(), "".to_string()));
            }
            if !data.rule_traffic.is_empty() {
                rows.push((
                    format!("Requests by Rule ({})", data.time_range.label()),
                    "".to_string(),
                ));
                rows.push(("".to_string(), "".to_string()));
                rows.push((
                    format!(
                        " {:<34} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8}",
                        "Rule", "Total", "Allow", "Block", "Count", "CAPTCHA", "Chlng"
                    ),
                    "".to_string(),
                ));
                let fmt0 = |v: f64| -> String {
                    if v > 0.0 { format_count(v) } else { "—".to_string() }
                };
                const MAX_RULE_ROWS: usize = 30;
                for r in data.rule_traffic.iter().take(MAX_RULE_ROWS) {
                    let name = if r.name.chars().count() > 34 {
                        format!("{}…", r.name.chars().take(33).collect::<String>())
                    } else {
                        r.name.clone()
                    };
                    rows.push((
                        format!(
                            " {:<34} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8}",
                            name,
                            format_count(r.total()),
                            fmt0(r.allowed),
                            fmt0(r.blocked),
                            fmt0(r.counted),
                            fmt0(r.captcha),
                            fmt0(r.challenge),
                        ),
                        "".to_string(),
                    ));
                }
                if data.rule_traffic.len() > MAX_RULE_ROWS {
                    rows.push((
                        format!(" … +{} more rules", data.rule_traffic.len() - MAX_RULE_ROWS),
                        "".to_string(),
                    ));
                }
                rows.push(("".to_string(), "".to_string()));
            }

            // Per-label totals — managed rule groups emit these (bot
            // categories, attack signals); absent unless labels fired.
            if !data.label_traffic.is_empty() {
                rows.push((
                    format!("Requests by Label ({})", data.time_range.label()),
                    "".to_string(),
                ));
                rows.push(("".to_string(), "".to_string()));
                const MAX_LABEL_ROWS: usize = 20;
                for l in data.label_traffic.iter().take(MAX_LABEL_ROWS) {
                    let name = if l.name.chars().count() > 64 {
                        format!("{}…", l.name.chars().take(63).collect::<String>())
                    } else {
                        l.name.clone()
                    };
                    let mut parts: Vec<String> = Vec::new();
                    for (v, tag) in [
                        (l.allowed, "alw"),
                        (l.blocked, "blk"),
                        (l.counted, "cnt"),
                        (l.captcha, "cap"),
                        (l.challenge, "chl"),
                    ] {
                        if v > 0.0 {
                            parts.push(format!("{} {}", tag, format_count(v)));
                        }
                    }
                    rows.push((
                        format!(
                            " {} — {} ({})",
                            name,
                            format_count(l.total()),
                            parts.join(" · ")
                        ),
                        "".to_string(),
                    ));
                }
                if data.label_traffic.len() > MAX_LABEL_ROWS {
                    rows.push((
                        format!(
                            " … +{} more labels",
                            data.label_traffic.len() - MAX_LABEL_ROWS
                        ),
                        "".to_string(),
                    ));
                }
                rows.push(("".to_string(), "".to_string()));
            }

            // Sparkline-style breakdown per data point
            if !data.allowed.is_empty() || !data.blocked.is_empty() {
                rows.push(("Recent Activity".to_string(), "".to_string()));
                rows.push(("".to_string(), "".to_string()));

                let max_pts = 20;
                let spark = |pts: &[(f64, f64)]| -> String {
                    if pts.is_empty() { return "—".to_string(); }
                    let bars = "▁▂▃▄▅▆▇█";
                    let bar_chars: Vec<char> = bars.chars().collect();
                    let max_v = pts.iter().map(|(_, v)| *v).fold(0.0_f64, f64::max).max(1.0);
                    let step = pts.len().max(1) / max_pts.min(pts.len()).max(1);
                    pts.iter()
                        .step_by(step.max(1))
                        .take(max_pts)
                        .map(|(_, v)| {
                            let idx = ((v / max_v) * 7.0).round() as usize;
                            bar_chars[idx.min(7)]
                        })
                        .collect()
                };

                rows.push((format!(" Allowed   {}", spark(&data.allowed)), "".to_string()));
                rows.push((format!(" Blocked   {}", spark(&data.blocked)), "".to_string()));
                rows.push((format!(" Counted   {}", spark(&data.counted)), "".to_string()));
                if !data.captcha.is_empty() {
                    rows.push((format!(" CAPTCHA   {}", spark(&data.captcha)), "".to_string()));
                }
                if !data.challenge.is_empty() {
                    rows.push((format!(" Challenge {}", spark(&data.challenge)), "".to_string()));
                }
            }

            rows.push(("".to_string(), "".to_string()));
            rows
        }
    }
}

pub(super) fn format_count(v: f64) -> String {
    let n = v as u64;
    if n >= 1_000_000 {
        format!("{:.1}M", v / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", v / 1_000.0)
    } else {
        format!("{}", n)
    }
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
