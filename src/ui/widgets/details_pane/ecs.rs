use super::*;

// ── ECS split panes ────────────────────────────────────────────────────────

/// Shared scaffold for the ECS split detail panes: fixed header | rule |
/// section tab bar | rule | scrollable body.
pub(super) fn render_ecs_split(
    app: &App,
    title: &str,
    footer: &str,
    header_lines: Vec<Line<'static>>,
    tabs: &[(char, &str, bool)],
    area: Rect,
    frame: &mut Frame,
) {
    let focused = app.details_focused;

    let mut block = theme::pane_block(title, focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );

    let inner = block.inner(area);
    frame.render_widget(block, area);

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
    render_ecs_section_tabs(app, chunks[2], tabs, frame);
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub(super) fn render_ecs_section_tabs(app: &App, area: Rect, tabs: &[(char, &str, bool)], frame: &mut Frame) {
    render_section_tab_bar(app, area, frame, tabs);
}

pub(super) fn header_name_line(name: &str) -> Line<'static> {
    Line::from(vec![
        Span::raw("  "),
        Span::styled(
            name.to_string(),
            Style::default()
                .fg(crate::ui::theme::text_primary())
                .add_modifier(Modifier::BOLD),
        ),
    ])
}

// ── ECS Cluster split pane ──────────────────────────────────────────────────

pub(super) fn render_ecs_cluster_split(app: &App, cluster: &EcsCluster, area: Rect, frame: &mut Frame) {
    let header = build_ecs_cluster_header_lines(cluster);
    let tabs = descriptor_tabs(app, &crate::aws::services::ecs::ECS_CLUSTER_SECTIONS);
    render_ecs_split(
        app,
        "ECS Cluster",
        &detail_footer(app, app.details_focused, 3, ""),
        header,
        &tabs,
        area,
        frame,
    );
}

pub(super) fn build_ecs_cluster_header_lines(cluster: &EcsCluster) -> Vec<Line<'static>> {
    let mut lines = vec![Line::raw(""), header_name_line(&cluster.cluster_name)];
    lines.push(Line::raw(""));
    lines.push(header_kv("Status", &cluster.status));
    lines.push(header_kv(
        "Tasks",
        &format!("{} running / {} pending", cluster.running_tasks, cluster.pending_tasks),
    ));
    lines.push(header_kv(
        "Services",
        &format!("{} active", cluster.active_services),
    ));
    lines.push(Line::raw(""));
    lines
}

pub fn ecs_cluster_section_lines(
    cluster: &EcsCluster,
    section: EcsClusterDetailSection,
) -> Vec<(String, String)> {
    match section {
        EcsClusterDetailSection::Overview => {
            let mut rows = vec![("".into(), "".into())];
            rows.push(("Cluster Name".into(), cluster.cluster_name.clone()));
            rows.push(("Status".into(), cluster.status.clone()));
            rows.push(("Running Tasks".into(), cluster.running_tasks.to_string()));
            rows.push(("Pending Tasks".into(), cluster.pending_tasks.to_string()));
            rows.push(("Active Services".into(), cluster.active_services.to_string()));
            rows.push((
                "Container Instances".into(),
                cluster.container_instances.to_string(),
            ));
            rows.push(("ARN".into(), cluster.cluster_arn.clone()));

            if !cluster.capacity_providers.is_empty() || !cluster.default_strategy.is_empty() {
                rows.push(("".into(), "".into()));
                rows.push(("Capacity Providers".into(), "".into()));
                if cluster.capacity_providers.is_empty() {
                    rows.push(("  none".into(), "".into()));
                } else {
                    rows.push(("  Providers".into(), cluster.capacity_providers.join(", ")));
                }
                for s in &cluster.default_strategy {
                    rows.push((format!("  default: {}", s), "".into()));
                }
            }
            rows.push(("".into(), "".into()));
            rows
        }
        EcsClusterDetailSection::Settings => {
            let mut rows = vec![("".into(), "".into())];
            rows.push((
                "Container Insights".into(),
                cluster
                    .container_insights
                    .clone()
                    .unwrap_or_else(|| "—".into()),
            ));
            let extra: Vec<&(String, String)> = cluster
                .settings
                .iter()
                .filter(|(k, _)| k != "containerInsights")
                .collect();
            for (k, v) in extra {
                rows.push((k.clone(), v.clone()));
            }

            if !cluster.statistics.is_empty() {
                rows.push(("".into(), "".into()));
                rows.push(("Statistics".into(), "".into()));
                for (k, v) in &cluster.statistics {
                    rows.push((format!("  {}", k), v.clone()));
                }
            }
            rows.push(("".into(), "".into()));
            rows
        }
        EcsClusterDetailSection::Tags => map_tags_lines(&cluster.tags),
    }
}

// ── ECS Service split pane ──────────────────────────────────────────────────

pub(super) fn render_ecs_service_split(app: &App, svc: &EcsServiceInfo, area: Rect, frame: &mut Frame) {
    let header = build_ecs_service_header_lines(svc);
    let tabs = descriptor_tabs(app, &crate::aws::services::ecs::ECS_SERVICE_SECTIONS);
    render_ecs_split(
        app,
        "ECS Service",
        &detail_footer(app, app.details_focused, 6, ""),
        header,
        &tabs,
        area,
        frame,
    );
}

pub(super) fn build_ecs_service_header_lines(svc: &EcsServiceInfo) -> Vec<Line<'static>> {
    let mut lines = vec![Line::raw(""), header_name_line(&svc.service_name)];
    lines.push(Line::raw(""));
    lines.push(header_kv("Status", &svc.status));
    lines.push(header_kv(
        "Tasks",
        &format!(
            "{} running / {} desired ({} pending)",
            svc.running_count, svc.desired_count, svc.pending_count
        ),
    ));
    lines.push(header_kv("Task Definition", &svc.task_definition));
    lines.push(Line::raw(""));
    lines
}

pub fn ecs_service_section_lines(
    svc: &EcsServiceInfo,
    section: EcsServiceDetailSection,
    tasks: Option<&crate::lazy::Lazy<Vec<crate::aws::services::ecs::EcsTask>>>,
    optimizer: Option<&Lazy<Option<crate::aws::services::computeoptimizer::OptimizerRec>>>,
    enrollment: Option<&crate::aws::services::computeoptimizer::CoEnrollment>,
) -> Vec<(String, String)> {
    match section {
        EcsServiceDetailSection::Overview => {
            let mut rows = vec![("".into(), "".into())];
            rows.push(("Service Name".into(), svc.service_name.clone()));
            rows.push(("Cluster".into(), svc.cluster_name.clone()));
            rows.push(("Status".into(), svc.status.clone()));
            rows.push(("Desired Count".into(), svc.desired_count.to_string()));
            rows.push(("Running Count".into(), svc.running_count.to_string()));
            rows.push(("Pending Count".into(), svc.pending_count.to_string()));
            // Jump anchor → the Tasks sub-tab filtered to this service (⏎).
            rows.push((
                "Tasks".into(),
                format!("→ view this service's tasks ({} running)", svc.running_count),
            ));
            rows.push(("Task Definition".into(), svc.task_definition.clone()));
            rows.push(("Launch Type".into(), svc.launch_type.clone()));
            if let Some(pv) = &svc.platform_version {
                rows.push(("Platform Version".into(), pv.clone()));
            }
            if let Some(s) = &svc.scheduling_strategy {
                rows.push(("Scheduling".into(), s.clone()));
            }
            if let Some(c) = &svc.deployment_controller {
                rows.push(("Deploy Controller".into(), c.clone()));
            }
            rows.push((
                "Execute Command".into(),
                if svc.enable_execute_command { "enabled".into() } else { "disabled".into() },
            ));
            if let Some(p) = &svc.propagate_tags {
                rows.push(("Propagate Tags".into(), p.clone()));
            }
            if !svc.created_at.is_empty() {
                rows.push(("Created".into(), svc.created_at.clone()));
            }
            if let Some(r) = &svc.role_arn {
                rows.push(("".into(), "".into()));
                rows.push(("Service Role".into(), r.clone()));
            }
            rows.push(("".into(), "".into()));
            rows
        }
        EcsServiceDetailSection::Deployments => {
            let mut rows = vec![("".into(), "".into())];
            rows.push(("Deployment Config".into(), "".into()));
            if let Some(m) = svc.min_healthy_percent {
                rows.push(("  Min Healthy %".into(), m.to_string()));
            }
            if let Some(m) = svc.max_percent {
                rows.push(("  Max %".into(), m.to_string()));
            }
            if let Some((enable, rollback)) = svc.circuit_breaker {
                rows.push((
                    "  Circuit Breaker".into(),
                    if enable { "enabled".into() } else { "disabled".into() },
                ));
                if enable {
                    rows.push((
                        "  Auto Rollback".into(),
                        if rollback { "enabled".into() } else { "disabled".into() },
                    ));
                }
            }

            rows.push(("".into(), "".into()));
            if svc.deployments.is_empty() {
                rows.push(("  No active deployments".into(), "".into()));
            } else {
                for d in &svc.deployments {
                    rows.push((d.status.clone(), d.task_definition.clone()));
                    rows.push((
                        "  Tasks".into(),
                        format!(
                            "{} running / {} desired ({} pending, {} failed)",
                            d.running, d.desired, d.pending, d.failed
                        ),
                    ));
                    if let Some(rs) = &d.rollout_state {
                        let reason = d
                            .rollout_state_reason
                            .as_deref()
                            .filter(|r| !r.is_empty())
                            .map(|r| format!(" — {}", r))
                            .unwrap_or_default();
                        rows.push(("  Rollout".into(), format!("{}{}", rs, reason)));
                    }
                    if !d.created_at.is_empty() {
                        rows.push(("  Created".into(), d.created_at.clone()));
                    }
                    if !d.updated_at.is_empty() {
                        rows.push(("  Updated".into(), d.updated_at.clone()));
                    }
                    rows.push(("".into(), "".into()));
                }
            }
            rows.push(("".into(), "".into()));
            rows
        }
        EcsServiceDetailSection::Tasks => {
            let mut rows = vec![("".into(), "".into())];
            match tasks {
                None | Some(crate::lazy::Lazy::Loading) => {
                    rows.push(("  Loading…".into(), "".into()));
                }
                Some(crate::lazy::Lazy::Error(e)) => {
                    rows.extend(error_rows(e));
                }
                Some(crate::lazy::Lazy::Loaded(list)) if list.is_empty() => {
                    rows.push(("  No tasks".into(), "".into()));
                }
                Some(crate::lazy::Lazy::Loaded(list)) => {
                    let running: Vec<_> =
                        list.iter().filter(|t| t.last_status != "STOPPED").collect();
                    let stopped: Vec<_> =
                        list.iter().filter(|t| t.last_status == "STOPPED").collect();

                    rows.push((format!("Running / Active ({})", running.len()), "".into()));
                    if running.is_empty() {
                        rows.push(("  none".into(), "".into()));
                    }
                    for t in &running {
                        // value = task id → Enter jumps to the task (⏎ tails logs).
                        rows.push((t.last_status.clone(), t.task_id.clone()));
                        if let Some(h) = &t.health_status {
                            rows.push((format!("  health: {}", h), "".into()));
                        }
                        if !t.started_at.is_empty() {
                            rows.push((format!("  started: {}", t.started_at), "".into()));
                        }
                    }

                    rows.push(("".into(), "".into()));
                    rows.push((format!("Recently Stopped ({})", stopped.len()), "".into()));
                    if stopped.is_empty() {
                        rows.push(("  none".into(), "".into()));
                    }
                    for t in &stopped {
                        rows.push((t.last_status.clone(), t.task_id.clone()));
                        // Why it stopped (stop code + reason).
                        if let Some(code) = &t.stop_code {
                            let reason = t.stopped_reason.as_deref().unwrap_or("");
                            let detail = if reason.is_empty() {
                                code.clone()
                            } else {
                                format!("{} — {}", code, reason)
                            };
                            rows.push((format!("  reason: {}", detail), "".into()));
                        } else if let Some(reason) = &t.stopped_reason {
                            rows.push((format!("  reason: {}", reason), "".into()));
                        }
                        // Per-container exit codes — the usual smoking gun.
                        for c in &t.containers {
                            if let Some(code) = c.exit_code {
                                let cr = c
                                    .reason
                                    .as_deref()
                                    .filter(|r| !r.is_empty())
                                    .map(|r| format!(" ({})", r))
                                    .unwrap_or_default();
                                rows.push((
                                    format!("  {} → exit {}{}", c.name, code, cr),
                                    "".into(),
                                ));
                            } else if let Some(r) = &c.reason {
                                if !r.is_empty() {
                                    rows.push((format!("  {}: {}", c.name, r), "".into()));
                                }
                            }
                        }
                        if !t.stopped_at.is_empty() {
                            rows.push((format!("  stopped: {}", t.stopped_at), "".into()));
                        }
                        rows.push(("".into(), "".into()));
                    }
                }
            }
            rows.push(("".into(), "".into()));
            rows
        }
        EcsServiceDetailSection::Networking => {
            let mut rows = vec![("".into(), "".into())];

            rows.push(("Load Balancers".into(), "".into()));
            if svc.load_balancers.is_empty() {
                rows.push(("  none".into(), "".into()));
            } else {
                for lb in &svc.load_balancers {
                    let port = lb
                        .container_port
                        .map(|p| format!(":{}", p))
                        .unwrap_or_default();
                    rows.push((
                        "  Container".into(),
                        format!("{}{}", lb.container_name, port),
                    ));
                    // Target group name in the value → the ECS-service jump
                    // classifier turns this row into a hop to the ELB Target
                    // Groups tab (⏎). For a classic ELB, show its name instead.
                    if !lb.target_group_name.is_empty() {
                        rows.push(("  Target Group".into(), lb.target_group_name.clone()));
                    } else if !lb.load_balancer_name.is_empty() {
                        rows.push(("  Load Balancer".into(), lb.load_balancer_name.clone()));
                    }
                    rows.push(("".into(), "".into()));
                }
            }

            rows.push(("".into(), "".into()));
            rows.push(("Service Registries".into(), "".into()));
            if svc.service_registries.is_empty() {
                rows.push(("  none".into(), "".into()));
            } else {
                for r in &svc.service_registries {
                    rows.push((format!("  {}", r), "".into()));
                }
            }

            if !svc.awsvpc_subnets.is_empty()
                || !svc.awsvpc_security_groups.is_empty()
                || svc.assign_public_ip.is_some()
            {
                rows.push(("".into(), "".into()));
                rows.push(("Network (awsvpc)".into(), "".into()));
                if !svc.awsvpc_subnets.is_empty() {
                    rows.push(("  Subnets".into(), svc.awsvpc_subnets.join(", ")));
                }
                if !svc.awsvpc_security_groups.is_empty() {
                    rows.push((
                        "  Security Groups".into(),
                        svc.awsvpc_security_groups.join(", "),
                    ));
                }
                if let Some(p) = &svc.assign_public_ip {
                    rows.push(("  Public IP".into(), p.clone()));
                }
            }

            if !svc.capacity_provider_strategy.is_empty() {
                rows.push(("".into(), "".into()));
                rows.push(("Capacity Provider Strategy".into(), "".into()));
                for s in &svc.capacity_provider_strategy {
                    rows.push((format!("  {}", s), "".into()));
                }
            }
            rows.push(("".into(), "".into()));
            rows
        }
        EcsServiceDetailSection::Events => {
            let mut rows = vec![("".into(), "".into())];
            if svc.events.is_empty() {
                rows.push(("  No recent events".into(), "".into()));
            } else {
                for e in &svc.events {
                    rows.push((e.created_at.clone(), "".into()));
                    rows.push((format!("  {}", e.message), "".into()));
                    rows.push(("".into(), "".into()));
                }
            }
            rows.push(("".into(), "".into()));
            rows
        }
        EcsServiceDetailSection::Tags => map_tags_lines(&svc.tags),
        EcsServiceDetailSection::Optimizer => optimizer_lines(optimizer, enrollment),
    }
}

// ── ECS Task Definition split pane ──────────────────────────────────────────

pub(super) fn render_ecs_taskdef_split(app: &App, td: &EcsTaskDefinition, area: Rect, frame: &mut Frame) {
    let header = build_ecs_taskdef_header_lines(td);
    let tabs = descriptor_tabs(app, &crate::aws::services::ecs::ECS_TASKDEF_SECTIONS);
    render_ecs_split(
        app,
        "Task Definition",
        &detail_footer(app, app.details_focused, 4, ""),
        header,
        &tabs,
        area,
        frame,
    );
}

pub(super) fn build_ecs_taskdef_header_lines(td: &EcsTaskDefinition) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::raw(""),
        header_name_line(&format!("{}:{}", td.family, td.revision)),
    ];
    lines.push(Line::raw(""));
    lines.push(header_kv("Family", &td.family));
    lines.push(header_kv("Revision", &td.revision.to_string()));
    lines.push(header_kv("Status", &td.status));
    lines.push(Line::raw(""));
    lines
}

pub fn ecs_taskdef_section_lines(
    td: &EcsTaskDefinition,
    section: EcsTaskDefDetailSection,
    state: Option<&crate::lazy::Lazy<Box<crate::aws::services::ecs::EcsTaskDefinitionDetails>>>,
) -> Vec<(String, String)> {
    let details = match state {
        None | Some(crate::lazy::Lazy::Loading) => {
            return vec![
                ("".into(), "".into()),
                ("  Loading task definition…".into(), "".into()),
                ("".into(), "".into()),
            ];
        }
        Some(crate::lazy::Lazy::Error(e)) => return error_rows(e),
        Some(crate::lazy::Lazy::Loaded(d)) => d,
    };

    match section {
        EcsTaskDefDetailSection::Overview => {
            let mut rows = vec![("".into(), "".into())];
            rows.push(("Family".into(), td.family.clone()));
            rows.push(("Revision".into(), td.revision.to_string()));
            if let Some(c) = &details.cpu {
                rows.push(("Task CPU".into(), c.clone()));
            }
            if let Some(m) = &details.memory {
                rows.push(("Task Memory".into(), m.clone()));
            }
            if let Some(n) = &details.network_mode {
                rows.push(("Network Mode".into(), n.clone()));
            }
            if !details.requires_compatibilities.is_empty() {
                rows.push((
                    "Compatibilities".into(),
                    details.requires_compatibilities.join(", "),
                ));
            }
            if let Some(a) = &details.cpu_architecture {
                rows.push(("CPU Architecture".into(), a.clone()));
            }
            if let Some(o) = &details.os_family {
                rows.push(("OS Family".into(), o.clone()));
            }
            if let Some(p) = &details.pid_mode {
                rows.push(("PID Mode".into(), p.clone()));
            }
            if let Some(i) = &details.ipc_mode {
                rows.push(("IPC Mode".into(), i.clone()));
            }
            if !details.registered_at.is_empty() {
                rows.push(("Registered".into(), details.registered_at.clone()));
            }
            rows.push(("".into(), "".into()));
            rows.push(("Roles".into(), "".into()));
            rows.push((
                "  Task Role".into(),
                details.task_role_arn.clone().unwrap_or_else(|| "—".into()),
            ));
            rows.push((
                "  Execution Role".into(),
                details
                    .execution_role_arn
                    .clone()
                    .unwrap_or_else(|| "—".into()),
            ));
            rows.push(("".into(), "".into()));
            rows
        }
        EcsTaskDefDetailSection::Containers => {
            let mut rows = vec![("".into(), "".into())];
            if details.containers.is_empty() {
                rows.push(("  No containers".into(), "".into()));
                rows.push(("".into(), "".into()));
                return rows;
            }
            for c in &details.containers {
                let badge = if c.essential { "" } else { " (non-essential)" };
                rows.push((format!("{}{}", c.name, badge), "".into()));
                rows.push(("  Image".into(), c.image.clone()));
                let mut size = Vec::new();
                if c.cpu > 0 {
                    size.push(format!("{} cpu", c.cpu));
                }
                if let Some(m) = c.memory {
                    size.push(format!("{} MiB", m));
                }
                if let Some(m) = c.memory_reservation {
                    size.push(format!("{} MiB reserved", m));
                }
                if !size.is_empty() {
                    rows.push(("  Resources".into(), size.join(", ")));
                }
                if !c.port_mappings.is_empty() {
                    rows.push(("  Ports".into(), c.port_mappings.join(", ")));
                }
                if !c.entry_point.is_empty() {
                    rows.push(("  Entrypoint".into(), c.entry_point.join(" ")));
                }
                if !c.command.is_empty() {
                    rows.push(("  Command".into(), c.command.join(" ")));
                }
                if let Some(d) = &c.log_driver {
                    let grp = c
                        .log_options
                        .iter()
                        .find(|(k, _)| k == "awslogs-group")
                        .map(|(_, v)| format!(" → {}", v))
                        .unwrap_or_default();
                    rows.push(("  Logs".into(), format!("{}{}", d, grp)));
                }
                if c.has_health_check {
                    rows.push(("  Health Check".into(), "configured".into()));
                }
                if c.mount_points > 0 {
                    rows.push(("  Mount Points".into(), c.mount_points.to_string()));
                }
                if !c.environment.is_empty() {
                    rows.push((format!("  Environment ({})", c.environment.len()), "".into()));
                    for (k, v) in &c.environment {
                        rows.push((format!("    {}", k), v.clone()));
                    }
                }
                if !c.secrets.is_empty() {
                    rows.push((format!("  Secrets ({})", c.secrets.len()), "".into()));
                    for (name, from) in &c.secrets {
                        rows.push((format!("    {}", name), from.clone()));
                    }
                }
                rows.push(("".into(), "".into()));
            }
            rows.push(("".into(), "".into()));
            rows
        }
        EcsTaskDefDetailSection::Volumes => {
            let mut rows = vec![("".into(), "".into())];
            if details.volumes.is_empty() {
                rows.push(("  No volumes".into(), "".into()));
            } else {
                for v in &details.volumes {
                    rows.push((v.name.clone(), v.kind.clone()));
                }
            }
            rows.push(("".into(), "".into()));
            rows
        }
        EcsTaskDefDetailSection::Tags => {
            let mut rows = vec![("".into(), "".into())];
            if details.tags.is_empty() {
                rows.push(("  No tags".into(), "".into()));
            } else {
                let mut sorted = details.tags.clone();
                sorted.sort_by(|a, b| a.0.cmp(&b.0));
                for (k, v) in sorted {
                    rows.push((format!("  {}", k), v));
                }
            }
            rows.push(("".into(), "".into()));
            rows
        }
    }
}

// ── ECS Task split pane ─────────────────────────────────────────────────────

pub(super) fn render_ecs_task_split(app: &App, task: &EcsTask, area: Rect, frame: &mut Frame) {
    let header = build_ecs_task_header_lines(task);
    let tabs = descriptor_tabs(app, &crate::aws::services::ecs::ECS_TASK_SECTIONS);
    render_ecs_split(
        app,
        "ECS Task",
        &detail_footer(app, app.details_focused, 4, "e logs"),
        header,
        &tabs,
        area,
        frame,
    );
}

pub(super) fn build_ecs_task_header_lines(task: &EcsTask) -> Vec<Line<'static>> {
    let mut lines = vec![Line::raw("")];
    let title = if task.is_stopped() {
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                task.task_id.clone(),
                Style::default()
                    .fg(crate::ui::theme::text_primary())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(
                "[STOPPED]",
                Style::default()
                    .fg(theme::error())
                    .add_modifier(Modifier::BOLD),
            ),
        ])
    } else {
        header_name_line(&task.task_id)
    };
    lines.push(title);
    lines.push(Line::raw(""));
    lines.push(header_kv("Status", &task.last_status));
    lines.push(header_kv("Task Definition", &task.task_definition));
    if let Some(h) = &task.health_status {
        lines.push(header_kv("Health", h));
    }
    lines.push(Line::raw(""));
    lines
}

pub fn ecs_task_section_lines(
    task: &EcsTask,
    section: EcsTaskDetailSection,
) -> Vec<(String, String)> {
    match section {
        EcsTaskDetailSection::Overview => {
            let mut rows = vec![("".into(), "".into())];

            // Stop reason is the headline triage info — show it first when stopped.
            if task.is_stopped() {
                rows.push(("Stopped".into(), "".into()));
                if let Some(c) = &task.stop_code {
                    rows.push(("  Stop Code".into(), c.clone()));
                }
                if let Some(r) = &task.stopped_reason {
                    rows.push(("  Reason".into(), r.clone()));
                }
                if !task.stopped_at.is_empty() {
                    rows.push(("  Stopped At".into(), task.stopped_at.clone()));
                }
                rows.push(("".into(), "".into()));
            }

            if let Some(svc) = &task.service_name {
                rows.push(("Service".into(), svc.clone()));
            }
            rows.push(("Last Status".into(), task.last_status.clone()));
            rows.push(("Desired Status".into(), task.desired_status.clone()));
            if let Some(h) = &task.health_status {
                rows.push(("Health".into(), h.clone()));
            }
            rows.push(("Launch Type".into(), task.launch_type.clone()));
            if let Some(p) = &task.platform_version {
                rows.push(("Platform Version".into(), p.clone()));
            }
            if !task.cpu.is_empty() {
                rows.push(("CPU".into(), task.cpu.clone()));
            }
            if !task.memory.is_empty() {
                rows.push(("Memory".into(), task.memory.clone()));
            }
            if !task.group.is_empty() {
                rows.push(("Group".into(), task.group.clone()));
            }
            if let Some(az) = &task.availability_zone {
                rows.push(("Availability Zone".into(), az.clone()));
            }
            if let Some(cp) = &task.capacity_provider {
                rows.push(("Capacity Provider".into(), cp.clone()));
            }
            if let Some(c) = &task.connectivity {
                rows.push(("Connectivity".into(), c.clone()));
            }
            if let Some(s) = &task.started_by {
                rows.push(("Started By".into(), s.clone()));
            }

            rows.push(("".into(), "".into()));
            if !task.created_at.is_empty() {
                rows.push(("Created".into(), task.created_at.clone()));
            }
            if !task.started_at.is_empty() {
                rows.push(("Started".into(), task.started_at.clone()));
            }
            rows.push(("Cluster".into(), task.cluster_name.clone()));
            rows.push(("Task ARN".into(), task.task_arn.clone()));
            rows.push(("".into(), "".into()));
            rows
        }
        EcsTaskDetailSection::Containers => {
            let mut rows = vec![("".into(), "".into())];
            if task.containers.is_empty() {
                rows.push(("  No containers".into(), "".into()));
                rows.push(("".into(), "".into()));
                return rows;
            }
            for c in &task.containers {
                rows.push((c.name.clone(), c.last_status.clone()));
                if let Some(h) = &c.health_status {
                    rows.push(("  Health".into(), h.clone()));
                }
                if let Some(code) = c.exit_code {
                    rows.push(("  Exit Code".into(), code.to_string()));
                }
                if let Some(r) = &c.reason {
                    if !r.is_empty() {
                        rows.push(("  Reason".into(), r.clone()));
                    }
                }
                if !c.image.is_empty() {
                    rows.push(("  Image".into(), c.image.clone()));
                }
                rows.push(("".into(), "".into()));
            }
            rows.push(("".into(), "".into()));
            rows
        }
        EcsTaskDetailSection::Networking => {
            let mut rows = vec![("".into(), "".into())];
            if task.enis.is_empty() {
                rows.push(("  No network interfaces".into(), "".into()));
            } else {
                for eni in &task.enis {
                    rows.push((
                        "ENI".into(),
                        eni.eni_id.clone().unwrap_or_else(|| "—".into()),
                    ));
                    if let Some(ip) = &eni.private_ip {
                        rows.push(("  Private IP".into(), ip.clone()));
                    }
                    if let Some(s) = &eni.subnet_id {
                        rows.push(("  Subnet".into(), s.clone()));
                    }
                    rows.push(("".into(), "".into()));
                }
            }
            rows.push(("".into(), "".into()));
            rows
        }
        EcsTaskDetailSection::Tags => map_tags_lines(&task.tags),
    }
}
