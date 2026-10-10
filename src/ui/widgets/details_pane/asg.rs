use super::*;

// ── Auto Scaling Group split pane ─────────────────────────────────────────

pub(super) fn render_asg_group_split(app: &App, group: &AsgGroup, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 4, "");

    let mut block = theme::pane_block("Auto Scaling Group", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let header_lines = build_asg_header_lines(group);
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
    render_asg_section_tabs(app, chunks[2], frame);
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub(super) fn build_asg_header_lines(group: &AsgGroup) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = vec![Line::raw("")];

    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            group.name.clone(),
            Style::default()
                .fg(crate::ui::theme::text_primary())
                .add_modifier(Modifier::BOLD),
        ),
    ]));

    lines.push(Line::raw(""));
    lines.push(header_kv(
        "Capacity",
        &format!(
            "{} desired ({} min / {} max)",
            group.desired_capacity, group.min_size, group.max_size
        ),
    ));
    lines.push(header_kv(
        "Instances",
        &format!(
            "{} in service / {}",
            group.in_service_count(),
            group.instances.len()
        ),
    ));
    if !group.health_check_type.is_empty() {
        lines.push(header_kv("Health Check", &group.health_check_type));
    }

    lines.push(Line::raw(""));
    lines
}

pub(super) fn render_asg_section_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let tabs = descriptor_tabs(app, &crate::aws::services::asg::ASG_GROUP_SECTIONS);
    render_section_tab_bar(app, area, frame, &tabs);
}

pub fn asg_group_section_lines(
    group: &AsgGroup,
    section: AsgGroupDetailSection,
    activities_state: Option<&crate::lazy::Lazy<Vec<crate::aws::services::asg::ScalingActivity>>>,
    optimizer: Option<&Lazy<Option<crate::aws::services::computeoptimizer::OptimizerRec>>>,
    enrollment: Option<&crate::aws::services::computeoptimizer::CoEnrollment>,
) -> Vec<(String, String)> {
    match section {
        AsgGroupDetailSection::Capacity => asg_capacity_lines(group),
        AsgGroupDetailSection::Instances => asg_instances_lines(group),
        AsgGroupDetailSection::Activities => asg_activities_lines(activities_state),
        AsgGroupDetailSection::Tags => asg_tags_lines(group),
        AsgGroupDetailSection::Optimizer => optimizer_lines(optimizer, enrollment),
    }
}

pub(super) fn asg_capacity_lines(group: &AsgGroup) -> Vec<(String, String)> {
    let mut rows = vec![];

    rows.push(("Desired Capacity".to_string(), group.desired_capacity.to_string()));
    rows.push(("Min Size".to_string(), group.min_size.to_string()));
    rows.push(("Max Size".to_string(), group.max_size.to_string()));
    rows.push((
        "In Service".to_string(),
        format!("{} of {}", group.in_service_count(), group.instances.len()),
    ));

    rows.push(("".to_string(), "".to_string()));
    if let Some(lt) = &group.launch_template {
        rows.push(("Launch Template".to_string(), lt.clone()));
    }
    if let Some(lc) = &group.launch_configuration {
        rows.push(("Launch Config".to_string(), lc.clone()));
    }
    if group.mixed_instances_policy {
        rows.push(("Mixed Instances".to_string(), "Yes".to_string()));
    }

    rows.push(("".to_string(), "".to_string()));
    if !group.health_check_type.is_empty() {
        rows.push(("Health Check Type".to_string(), group.health_check_type.clone()));
    }
    if let Some(grace) = group.health_check_grace_period {
        rows.push(("Grace Period".to_string(), format!("{}s", grace)));
    }
    rows.push(("Default Cooldown".to_string(), format!("{}s", group.default_cooldown)));

    rows.push(("".to_string(), "".to_string()));
    if !group.availability_zones.is_empty() {
        rows.push(("Availability Zones".to_string(), group.availability_zones.join(", ")));
    }
    if let Some(vpc_zone) = &group.vpc_zone_identifier {
        rows.push(("Subnets".to_string(), vpc_zone.clone()));
    }

    if !group.load_balancer_names.is_empty() || group.target_group_count > 0 {
        rows.push(("".to_string(), "".to_string()));
        if !group.load_balancer_names.is_empty() {
            rows.push((
                "Classic LBs".to_string(),
                group.load_balancer_names.join(", "),
            ));
        }
        if group.target_group_count > 0 {
            rows.push((
                "Traffic Sources".to_string(),
                format!("{} target group(s)", group.target_group_count),
            ));
        }
    }

    if !group.suspended_processes.is_empty() {
        rows.push(("".to_string(), "".to_string()));
        rows.push((
            "Suspended Processes".to_string(),
            group.suspended_processes.join(", "),
        ));
    }

    if !group.created_time.is_empty() {
        rows.push(("".to_string(), "".to_string()));
        rows.push(("Created".to_string(), group.created_time.clone()));
    }

    rows.push(("".to_string(), "".to_string()));
    rows
}

pub(super) fn asg_instances_lines(group: &AsgGroup) -> Vec<(String, String)> {
    let mut rows = vec![("".to_string(), "".to_string())];

    if group.instances.is_empty() {
        rows.push(("  No instances".to_string(), "".to_string()));
        rows.push(("".to_string(), "".to_string()));
        return rows;
    }

    // Fixed-width table: Instance / Type / Zone / Lifecycle / Health
    rows.push((
        format!(
            " {:<19}  {:<11}  {:<12}  {:<13}  {:<9}",
            "Instance", "Type", "Zone", "Lifecycle", "Health"
        ),
        "".to_string(),
    ));
    rows.push((
        format!(
            " {}  {}  {}  {}  {}",
            "─".repeat(19),
            "─".repeat(11),
            "─".repeat(12),
            "─".repeat(13),
            "─".repeat(9)
        ),
        "".to_string(),
    ));
    for inst in &group.instances {
        let itype = inst.instance_type.clone().unwrap_or_else(|| "-".to_string());
        rows.push((
            format!(
                " {:<19}  {:<11}  {:<12}  {:<13}  {:<9}",
                inst.instance_id,
                itype,
                inst.availability_zone,
                inst.lifecycle_state,
                inst.health_status
            ),
            "".to_string(),
        ));
    }

    rows.push(("".to_string(), "".to_string()));
    rows
}

pub(super) fn asg_activities_lines(state: Option<&crate::lazy::Lazy<Vec<crate::aws::services::asg::ScalingActivity>>>) -> Vec<(String, String)> {
    let mut rows = vec![("".to_string(), "".to_string())];

    match state {
        None | Some(crate::lazy::Lazy::Loading) => {
            rows.push(("  Loading scaling activities…".to_string(), "".to_string()));
        }
        Some(crate::lazy::Lazy::Loaded(activities)) => {
            if activities.is_empty() {
                rows.push(("  No scaling activities".to_string(), "".to_string()));
            } else {
                for act in activities {
                    let status = match act.progress {
                        Some(p) if p < 100 && act.status_code != "Successful" => {
                            format!("{} ({}%)", act.status_code, p)
                        }
                        _ => act.status_code.clone(),
                    };
                    rows.push((act.start_time.clone(), status));
                    if !act.description.is_empty() {
                        rows.push((format!("  {}", act.description), "".to_string()));
                    }
                    if !act.cause.is_empty() {
                        rows.push((format!("    cause: {}", act.cause), "".to_string()));
                    }
                    if let Some(msg) = &act.status_message {
                        if !msg.is_empty() {
                            rows.push((format!("    {}", msg), "".to_string()));
                        }
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

pub(super) fn asg_tags_lines(group: &AsgGroup) -> Vec<(String, String)> {
    let mut rows = vec![("".to_string(), "".to_string())];

    if group.tags.is_empty() {
        rows.push(("  No tags".to_string(), "".to_string()));
    } else {
        let mut sorted: Vec<(&String, &String)> = group.tags.iter().collect();
        sorted.sort_by_key(|(k, _)| k.as_str());
        for (key, value) in sorted {
            rows.push((format!("  {}", key), value.clone()));
        }
    }

    rows.push(("".to_string(), "".to_string()));
    rows
}
