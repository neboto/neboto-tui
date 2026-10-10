use super::*;

// ── Load Balancer split pane ──────────────────────────────────────────────

pub(super) fn render_load_balancer_split(app: &App, lb: &LoadBalancer, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 4, "");

    let mut block = theme::pane_block("Load Balancer", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let header_lines = build_load_balancer_header_lines(lb);
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
    render_load_balancer_section_tabs(app, chunks[2], frame);
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub(super) fn build_load_balancer_header_lines(lb: &LoadBalancer) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = vec![Line::raw("")];

    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            lb.name.clone(),
            Style::default()
                .fg(crate::ui::theme::text_primary())
                .add_modifier(Modifier::BOLD),
        ),
    ]));

    lines.push(Line::raw(""));
    lines.push(header_kv(
        "Type",
        &format!("{} · {}", lb.lb_type, lb.scheme),
    ));
    lines.push(header_kv("State", &lb.state_code));
    if !lb.dns_name.is_empty() {
        lines.push(header_kv("DNS", &lb.dns_name));
    }

    lines.push(Line::raw(""));
    lines
}

pub(super) fn render_load_balancer_section_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let tabs = descriptor_tabs(app, &crate::aws::services::elb::LOAD_BALANCER_SECTIONS);
    render_section_tab_bar(app, area, frame, &tabs);
}

pub fn load_balancer_section_lines(
    lb: &LoadBalancer,
    section: LoadBalancerDetailSection,
    details_state: Option<&crate::lazy::Lazy<crate::aws::services::elb::LbDetails>>,
) -> Vec<(String, String)> {
    match section {
        LoadBalancerDetailSection::Details => lb_details_lines(lb),
        LoadBalancerDetailSection::Listeners => lb_listeners_lines(details_state),
        LoadBalancerDetailSection::Attributes => lb_attributes_lines(details_state),
        LoadBalancerDetailSection::Tags => lb_tags_lines(lb),
    }
}

pub(super) fn lb_details_lines(lb: &LoadBalancer) -> Vec<(String, String)> {
    let mut rows = vec![];

    rows.push(("Name".to_string(), lb.name.clone()));
    rows.push(("Type".to_string(), lb.lb_type.clone()));
    rows.push(("Scheme".to_string(), lb.scheme.clone()));
    rows.push(("State".to_string(), lb.state_code.clone()));
    if let Some(reason) = &lb.state_reason {
        rows.push(("State Reason".to_string(), reason.clone()));
    }

    rows.push(("".to_string(), "".to_string()));
    if !lb.dns_name.is_empty() {
        rows.push(("DNS Name".to_string(), lb.dns_name.clone()));
    }
    if let Some(ip) = &lb.ip_address_type {
        rows.push(("IP Address Type".to_string(), ip.clone()));
    }

    rows.push(("".to_string(), "".to_string()));
    if let Some(vpc) = &lb.vpc_id {
        rows.push(("VPC".to_string(), vpc.clone()));
    }
    if !lb.availability_zones.is_empty() {
        rows.push((
            "Availability Zones".to_string(),
            format!("{} zones", lb.availability_zones.len()),
        ));
        for az in &lb.availability_zones {
            rows.push((format!("  {}", az), "".to_string()));
        }
    }

    if !lb.security_groups.is_empty() {
        rows.push(("".to_string(), "".to_string()));
        rows.push((
            "Security Groups".to_string(),
            format!("{} attached", lb.security_groups.len()),
        ));
        for sg in &lb.security_groups {
            rows.push((format!("  {}", sg), "".to_string()));
        }
    }

    if !lb.created_time.is_empty() {
        rows.push(("".to_string(), "".to_string()));
        rows.push(("Created".to_string(), lb.created_time.clone()));
    }

    rows.push(("".to_string(), "".to_string()));
    rows
}

pub(super) fn lb_listeners_lines(state: Option<&crate::lazy::Lazy<crate::aws::services::elb::LbDetails>>) -> Vec<(String, String)> {
    let mut rows = vec![("".to_string(), "".to_string())];

    match state {
        None | Some(crate::lazy::Lazy::Loading) => {
            rows.push(("  Loading listeners…".to_string(), "".to_string()));
        }
        Some(crate::lazy::Lazy::Loaded(details)) => {
            if details.listeners.is_empty() {
                rows.push(("  No listeners".to_string(), "".to_string()));
            } else {
                for l in &details.listeners {
                    let port = l.port.map(|p| p.to_string()).unwrap_or_else(|| "-".to_string());
                    rows.push((format!("{}:{}", l.protocol, port), String::new()));
                    if let Some(policy) = &l.ssl_policy {
                        rows.push(("  SSL Policy".to_string(), policy.clone()));
                    }
                    if l.certificate_count > 0 {
                        rows.push((
                            "  Certificates".to_string(),
                            l.certificate_count.to_string(),
                        ));
                    }
                    // Non-default rules first (most specific routing), then the
                    // default action as the catch-all at the bottom.
                    for rule in &l.rules {
                        rows.push((
                            format!("  [{}]", rule.priority),
                            rule.action.clone(),
                        ));
                        for cond in &rule.conditions {
                            rows.push((format!("      if {}", cond), "".to_string()));
                        }
                    }
                    if !l.default_action.is_empty() {
                        rows.push(("  default".to_string(), l.default_action.clone()));
                    }
                    rows.push(("".to_string(), "".to_string()));
                }
            }
        }
        Some(crate::lazy::Lazy::Error(e)) => {
            rows.extend(error_rows(e));
        }
    }

    rows.push(("".to_string(), "".to_string()));
    rows
}

pub(super) fn lb_attributes_lines(state: Option<&crate::lazy::Lazy<crate::aws::services::elb::LbDetails>>) -> Vec<(String, String)> {
    let mut rows = vec![("".to_string(), "".to_string())];

    match state {
        None | Some(crate::lazy::Lazy::Loading) => {
            rows.push(("  Loading attributes…".to_string(), "".to_string()));
        }
        Some(crate::lazy::Lazy::Loaded(details)) => {
            if details.attributes.is_empty() {
                rows.push(("  No attributes".to_string(), "".to_string()));
            } else {
                for (key, value) in &details.attributes {
                    rows.push((key.clone(), value.clone()));
                }
            }
        }
        Some(crate::lazy::Lazy::Error(e)) => {
            rows.extend(error_rows(e));
        }
    }

    rows.push(("".to_string(), "".to_string()));
    rows
}

pub(super) fn lb_tags_lines(lb: &LoadBalancer) -> Vec<(String, String)> {
    let mut rows = vec![("".to_string(), "".to_string())];

    if lb.tags.is_empty() {
        rows.push(("  No tags".to_string(), "".to_string()));
    } else {
        let mut sorted: Vec<(&String, &String)> = lb.tags.iter().collect();
        sorted.sort_by_key(|(k, _)| k.as_str());
        for (key, value) in sorted {
            rows.push((format!("  {}", key), value.clone()));
        }
    }

    rows.push(("".to_string(), "".to_string()));
    rows
}

// ── Target Group split pane ───────────────────────────────────────────────

pub(super) fn render_target_group_split(app: &App, tg: &TargetGroup, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 4, "");

    let mut block = theme::pane_block("Target Group", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let header_lines = build_target_group_header_lines(tg);
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
    render_target_group_section_tabs(app, chunks[2], frame);
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub(super) fn build_target_group_header_lines(tg: &TargetGroup) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = vec![Line::raw("")];

    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            tg.name.clone(),
            Style::default()
                .fg(crate::ui::theme::text_primary())
                .add_modifier(Modifier::BOLD),
        ),
    ]));

    lines.push(Line::raw(""));
    lines.push(header_kv("Protocol", &tg.target_display()));
    lines.push(header_kv("Target Type", &tg.target_type));
    if let Some(vpc) = &tg.vpc_id {
        lines.push(header_kv("VPC", vpc));
    }
    lines.push(header_kv(
        "Load Balancers",
        &tg.load_balancer_arns.len().to_string(),
    ));

    lines.push(Line::raw(""));
    lines
}

pub(super) fn render_target_group_section_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let tabs = descriptor_tabs(app, &crate::aws::services::elb::TARGET_GROUP_SECTIONS);
    render_section_tab_bar(app, area, frame, &tabs);
}

pub fn target_group_section_lines(
    tg: &TargetGroup,
    section: TargetGroupDetailSection,
    health_state: Option<&crate::lazy::Lazy<Vec<crate::aws::services::elb::TargetHealthEntry>>>,
    attributes_state: Option<&crate::lazy::Lazy<Vec<(String, String)>>>,
) -> Vec<(String, String)> {
    match section {
        TargetGroupDetailSection::Health => target_group_health_lines(health_state),
        TargetGroupDetailSection::Config => target_group_config_lines(tg),
        TargetGroupDetailSection::Attributes => target_group_attributes_lines(attributes_state),
        TargetGroupDetailSection::Tags => target_group_tags_lines(tg),
    }
}

pub(super) fn target_group_attributes_lines(
    state: Option<&crate::lazy::Lazy<Vec<(String, String)>>>,
) -> Vec<(String, String)> {
    let mut rows = vec![("".to_string(), "".to_string())];

    match state {
        None | Some(crate::lazy::Lazy::Loading) => {
            rows.push(("  Loading attributes…".to_string(), "".to_string()));
        }
        Some(crate::lazy::Lazy::Loaded(attributes)) => {
            if attributes.is_empty() {
                rows.push(("  No attributes".to_string(), "".to_string()));
            } else {
                for (key, value) in attributes {
                    rows.push((key.clone(), value.clone()));
                }
            }
        }
        Some(crate::lazy::Lazy::Error(e)) => {
            rows.extend(error_rows(e));
        }
    }

    rows.push(("".to_string(), "".to_string()));
    rows
}

pub(super) fn target_group_health_lines(health_state: Option<&crate::lazy::Lazy<Vec<crate::aws::services::elb::TargetHealthEntry>>>) -> Vec<(String, String)> {
    let mut rows = vec![("".to_string(), "".to_string())];

    match health_state {
        None | Some(crate::lazy::Lazy::Loading) => {
            rows.push(("  Loading target health…".to_string(), "".to_string()));
        }
        Some(crate::lazy::Lazy::Loaded(entries)) => {
            if entries.is_empty() {
                rows.push(("  No registered targets".to_string(), "".to_string()));
            } else {
                let healthy = entries.iter().filter(|e| e.state == "healthy").count();
                rows.push((
                    "Summary".to_string(),
                    format!("{}/{} healthy", healthy, entries.len()),
                ));
                rows.push(("".to_string(), "".to_string()));

                // Fixed-width table: Target / Port / Zone / State
                rows.push((
                    format!(
                        " {:<22}  {:>5}  {:<12}  {:<11}",
                        "Target", "Port", "Zone", "State"
                    ),
                    "".to_string(),
                ));
                rows.push((
                    format!(
                        " {}  {}  {}  {}",
                        "─".repeat(22),
                        "─".repeat(5),
                        "─".repeat(12),
                        "─".repeat(11)
                    ),
                    "".to_string(),
                ));
                for e in entries {
                    let port = e.port.map(|p| p.to_string()).unwrap_or_else(|| "-".to_string());
                    let zone = e.availability_zone.clone().unwrap_or_else(|| "-".to_string());
                    rows.push((
                        format!(
                            " {:<22}  {:>5}  {:<12}  {:<11}",
                            e.target_id, port, zone, e.state
                        ),
                        "".to_string(),
                    ));
                    if e.state != "healthy" {
                        if let Some(reason) = &e.reason {
                            rows.push((format!("    reason: {}", reason), "".to_string()));
                        }
                        if let Some(desc) = &e.description {
                            rows.push((format!("    {}", desc), "".to_string()));
                        }
                    }
                }
            }
        }
        Some(crate::lazy::Lazy::Error(e)) => {
            rows.extend(error_rows(e));
        }
    }

    rows.push(("".to_string(), "".to_string()));
    rows
}

pub(super) fn target_group_config_lines(tg: &TargetGroup) -> Vec<(String, String)> {
    let mut rows = vec![];

    rows.push(("Name".to_string(), tg.name.clone()));
    rows.push(("Protocol".to_string(), tg.target_display()));
    rows.push(("Target Type".to_string(), tg.target_type.clone()));
    if let Some(pv) = &tg.protocol_version {
        rows.push(("Protocol Version".to_string(), pv.clone()));
    }
    if let Some(vpc) = &tg.vpc_id {
        rows.push(("VPC".to_string(), vpc.clone()));
    }

    rows.push(("".to_string(), "".to_string()));
    rows.push(("Health Check".to_string(), "".to_string()));
    rows.push((
        "  Enabled".to_string(),
        if tg.health_check_enabled { "Yes".to_string() } else { "No".to_string() },
    ));
    if !tg.health_check_protocol.is_empty() {
        rows.push(("  Protocol".to_string(), tg.health_check_protocol.clone()));
    }
    if let Some(port) = &tg.health_check_port {
        rows.push(("  Port".to_string(), port.clone()));
    }
    if let Some(path) = &tg.health_check_path {
        rows.push(("  Path".to_string(), path.clone()));
    }
    if let Some(interval) = tg.health_check_interval_secs {
        rows.push(("  Interval".to_string(), format!("{}s", interval)));
    }
    if let Some(timeout) = tg.health_check_timeout_secs {
        rows.push(("  Timeout".to_string(), format!("{}s", timeout)));
    }
    if let Some(h) = tg.healthy_threshold {
        rows.push(("  Healthy Threshold".to_string(), h.to_string()));
    }
    if let Some(u) = tg.unhealthy_threshold {
        rows.push(("  Unhealthy Threshold".to_string(), u.to_string()));
    }
    if let Some(matcher) = &tg.matcher {
        rows.push(("  Success Codes".to_string(), matcher.clone()));
    }

    if !tg.load_balancer_arns.is_empty() {
        rows.push(("".to_string(), "".to_string()));
        rows.push((
            "Load Balancers".to_string(),
            format!("{} attached", tg.load_balancer_arns.len()),
        ));
        for arn in &tg.load_balancer_arns {
            let name = arn.rsplit('/').nth(1).unwrap_or(arn);
            // Carry the full ARN in the value so the row is an Enter-able jump
            // anchor to the Load Balancers tab (resolved by arn_jump_target).
            rows.push((format!("  {}", name), arn.clone()));
        }
    }

    rows.push(("".to_string(), "".to_string()));
    rows
}

pub(super) fn target_group_tags_lines(tg: &TargetGroup) -> Vec<(String, String)> {
    let mut rows = vec![("".to_string(), "".to_string())];

    if tg.tags.is_empty() {
        rows.push(("  No tags".to_string(), "".to_string()));
    } else {
        let mut sorted: Vec<(&String, &String)> = tg.tags.iter().collect();
        sorted.sort_by_key(|(k, _)| k.as_str());
        for (key, value) in sorted {
            rows.push((format!("  {}", key), value.clone()));
        }
    }

    rows.push(("".to_string(), "".to_string()));
    rows
}
