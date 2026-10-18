use super::*;

// ── Auto Scaling Group split pane ─────────────────────────────────────────

pub(super) fn render_asg_group_split(app: &App, group: &AsgGroup, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, crate::aws::services::asg::ASG_GROUP_SECTIONS.len(), "");

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
    lifecycle: Option<&Lazy<crate::aws::services::asg::AsgLifecycle>>,
) -> Vec<(String, String)> {
    match section {
        AsgGroupDetailSection::Capacity => asg_capacity_lines(group),
        AsgGroupDetailSection::Instances => asg_instances_lines(group),
        AsgGroupDetailSection::Activities => asg_activities_lines(activities_state),
        AsgGroupDetailSection::Tags => asg_tags_lines(group),
        AsgGroupDetailSection::Optimizer => optimizer_lines(optimizer, enrollment),
        AsgGroupDetailSection::Lifecycle => asg_lifecycle_lines(lifecycle),
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
    if !group.termination_policies.is_empty() {
        rows.push(("Termination Policies".to_string(), group.termination_policies.join(" → ")));
    }
    if group.scale_in_protection {
        rows.push(("Scale-in Protection".to_string(), "new instances protected".to_string()));
    }

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
    let lw = lifecycle_col_width(group.instances.iter().map(|i| i.lifecycle_state.as_str()));
    rows.push((
        format!(
            " {:<19}  {:<11}  {:<12}  {:<lw$}  {:<9}",
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
            "─".repeat(lw),
            "─".repeat(9)
        ),
        "".to_string(),
    ));
    for inst in &group.instances {
        let itype = inst.instance_type.clone().unwrap_or_else(|| "-".to_string());
        rows.push((
            format!(
                " {:<19}  {:<11}  {:<12}  {:<lw$}  {:<9}",
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

/// Width of the Lifecycle column: the longest state present, at least the
/// header. A fixed width can't hold the hook-held states
/// (`Warmed:Terminating:Retained` is 27 chars), and `{:<N}` never truncates,
/// so the Health column would shift on exactly the rows that matter.
fn lifecycle_col_width<'a>(states: impl Iterator<Item = &'a str>) -> usize {
    states.map(|s| s.chars().count()).max().unwrap_or(0).max("Lifecycle".len())
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

/// Lifecycle hooks (the pauses on launch / terminate) and the warm pool. The
/// two calls fail independently, so each half renders its own error.
pub(super) fn asg_lifecycle_lines(
    state: Option<&Lazy<crate::aws::services::asg::AsgLifecycle>>,
) -> Vec<(String, String)> {
    let mut rows = vec![(String::new(), String::new())];
    let lc = match state {
        None | Some(Lazy::Loading) => {
            rows.push((String::new(), "Loading…".to_string()));
            return rows;
        }
        Some(Lazy::Error(e)) => {
            rows.extend(error_rows(e));
            return rows;
        }
        Some(Lazy::Loaded(lc)) => lc,
    };

    match &lc.hooks {
        Err(e) => {
            rows.push(("Lifecycle Hooks".to_string(), String::new()));
            rows.extend(error_rows(e));
            rows.push((String::new(), String::new()));
        }
        Ok(hooks) if hooks.is_empty() => {
            rows.push(("Lifecycle Hooks".to_string(), String::new()));
            rows.push((String::new(), String::new()));
            rows.push((String::new(), "No lifecycle hooks".to_string()));
            rows.push((String::new(), String::new()));
        }
        Ok(hooks) => {
            rows.push((format!("Lifecycle Hooks ({})", hooks.len()), String::new()));
            rows.push((String::new(), String::new()));
            for h in hooks {
                rows.push((h.name.clone(), String::new()));
                // "autoscaling:EC2_INSTANCE_LAUNCHING" → "launching".
                let transition = h
                    .transition
                    .rsplit('_')
                    .next()
                    .map(|t| t.to_ascii_lowercase())
                    .unwrap_or_default();
                rows.push(("  Transition".to_string(), transition));
                if let Some(t) = h.heartbeat_timeout {
                    let then = h
                        .default_result
                        .as_deref()
                        .map(|r| format!(", then {r}"))
                        .unwrap_or_default();
                    rows.push(("  Heartbeat timeout".to_string(), format!("{t}s{then}")));
                }
                if let Some(t) = h.global_timeout {
                    rows.push(("  Global timeout".to_string(), format!("{t}s")));
                }
                if let Some(arn) = &h.notification_target {
                    rows.push(("  Notification target".to_string(), arn.clone()));
                }
                if let Some(arn) = &h.role_arn {
                    rows.push(("  Role".to_string(), arn.clone()));
                }
                if let Some(m) = &h.metadata {
                    rows.push(("  Metadata".to_string(), m.clone()));
                }
                rows.push((String::new(), String::new()));
            }
        }
    }

    rows.push(("Warm Pool".to_string(), String::new()));
    rows.push((String::new(), String::new()));
    match &lc.warm_pool {
        Err(e) => {
            rows.extend(error_rows(e));
        }
        Ok(None) => {
            rows.push((String::new(), "No warm pool".to_string()));
        }
        Ok(Some(p)) => {
            rows.push(("  Pool state".to_string(), p.pool_state.clone()));
            if let Some(s) = &p.status {
                rows.push(("  Status".to_string(), s.clone()));
            }
            rows.push((
                "  Min size".to_string(),
                p.min_size.map(|n| n.to_string()).unwrap_or_else(|| "0".to_string()),
            ));
            rows.push((
                "  Max prepared capacity".to_string(),
                p.max_prepared_capacity
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "the group's max size".to_string()),
            ));
            rows.push((
                "  Reuse on scale-in".to_string(),
                if p.reuse_on_scale_in == Some(true) { "✓ yes" } else { "no" }.to_string(),
            ));
            rows.push((String::new(), String::new()));
            rows.push((format!("Warm Instances ({})", p.instances.len()), String::new()));
            rows.push((String::new(), String::new()));
            if p.instances.is_empty() {
                rows.push((String::new(), "No instances in the pool".to_string()));
            } else {
                // Same table shape as the Instances section.
                let lw = lifecycle_col_width(p.instances.iter().map(|i| i.lifecycle_state.as_str()));
                rows.push((
                    format!(
                        " {:<19}  {:<11}  {:<12}  {:<lw$}  {:<9}",
                        "Instance", "Type", "Zone", "Lifecycle", "Health"
                    ),
                    String::new(),
                ));
                rows.push((
                    format!(
                        " {}  {}  {}  {}  {}",
                        "─".repeat(19),
                        "─".repeat(11),
                        "─".repeat(12),
                        "─".repeat(lw),
                        "─".repeat(9)
                    ),
                    String::new(),
                ));
                for i in &p.instances {
                    rows.push((
                        format!(
                            " {:<19}  {:<11}  {:<12}  {:<lw$}  {:<9}",
                            i.instance_id,
                            i.instance_type.as_deref().unwrap_or("-"),
                            i.az.as_deref().unwrap_or("-"),
                            i.lifecycle_state,
                            i.health.as_deref().unwrap_or("-"),
                        ),
                        String::new(),
                    ));
                }
            }
        }
    }
    rows.push((String::new(), String::new()));
    rows
}

#[cfg(test)]
mod asg_lifecycle_tests {
    use super::*;
    use crate::aws::services::asg::{AsgLifecycle, AsgLifecycleHook, AsgWarmInstance, AsgWarmPool};

    fn text(rows: &[(String, String)]) -> String {
        rows.iter().map(|(k, v)| format!("{k}={v}\n")).collect()
    }

    #[test]
    fn a_denied_warm_pool_still_shows_the_hooks() {
        let lc = Lazy::Loaded(AsgLifecycle {
            hooks: Ok(vec![AsgLifecycleHook {
                name: "drain".into(),
                transition: "autoscaling:EC2_INSTANCE_TERMINATING".into(),
                heartbeat_timeout: Some(300),
                global_timeout: None,
                default_result: Some("CONTINUE".into()),
                notification_target: None,
                role_arn: None,
                metadata: None,
            }]),
            warm_pool: Err("AccessDenied: no".into()),
        });
        let t = text(&asg_lifecycle_lines(Some(&lc)));
        assert!(t.contains("  Transition=terminating"), "{t}");
        assert!(t.contains("300s, then CONTINUE"), "{t}");
        assert!(t.contains("AccessDenied"), "{t}");
    }

    #[test]
    fn no_warm_pool_is_not_an_error() {
        let lc = Lazy::Loaded(AsgLifecycle { hooks: Ok(vec![]), warm_pool: Ok(None) });
        let t = text(&asg_lifecycle_lines(Some(&lc)));
        assert!(t.contains("No lifecycle hooks") && t.contains("No warm pool"), "{t}");
        assert!(!t.contains('⚠'), "{t}");
    }

    #[test]
    fn warm_instances_list_with_their_state() {
        let lc = Lazy::Loaded(AsgLifecycle {
            hooks: Ok(vec![]),
            warm_pool: Ok(Some(AsgWarmPool {
                pool_state: "Stopped".into(),
                status: None,
                min_size: Some(2),
                max_prepared_capacity: None,
                reuse_on_scale_in: Some(true),
                instances: vec![AsgWarmInstance {
                    instance_id: "i-0123456789abcdef0".into(),
                    lifecycle_state: "Warmed:Stopped".into(),
                    az: Some("us-east-1a".into()),
                    instance_type: Some("t3.large".into()),
                    health: Some("Healthy".into()),
                }],
            })),
        });
        let t = text(&asg_lifecycle_lines(Some(&lc)));
        assert!(t.contains("the group's max size"), "{t}");
        assert!(t.contains("i-0123456789abcdef0") && t.contains("Warmed:Stopped"), "{t}");
    }

    #[test]
    fn long_hook_held_states_keep_the_health_column_aligned() {
        let inst = |id: &str, state: &str| AsgWarmInstance {
            instance_id: id.into(),
            lifecycle_state: state.into(),
            az: Some("us-east-1a".into()),
            instance_type: Some("t3.large".into()),
            health: Some("Healthy".into()),
        };
        let lc = Lazy::Loaded(AsgLifecycle {
            hooks: Ok(vec![]),
            warm_pool: Ok(Some(AsgWarmPool {
                pool_state: "Stopped".into(),
                status: None,
                min_size: None,
                max_prepared_capacity: None,
                reuse_on_scale_in: None,
                instances: vec![
                    inst("i-0000000000000000a", "Warmed:Stopped"),
                    inst("i-0000000000000000b", "Warmed:Terminating:Retained"),
                ],
            })),
        });
        let rows = asg_lifecycle_lines(Some(&lc));
        // Header ("Health") + both rows ("Healthy") start the column at one offset.
        let cols: Vec<usize> = rows
            .iter()
            .filter_map(|(k, _)| k.find("Health"))
            .collect();
        assert_eq!(cols.len(), 3, "{rows:?}");
        assert!(cols.iter().all(|c| *c == cols[0]), "{rows:?}");
    }
}
