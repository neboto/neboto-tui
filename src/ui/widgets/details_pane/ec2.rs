use super::*;

// ── EC2 Instance split pane ──────────────────────────────────────────────────

/// Entry point: outer block + inner layout of header / separator / tabs / separator / body.
pub(super) fn render_ec2_instance_split(app: &App, instance: &Ec2Instance, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;

    let footer = detail_footer(app, focused, 5, "");

    let mut block = theme::pane_block("EC2 Instance", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );

    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Build the header lines first so we can size the header area dynamically.
    let header_lines = build_ec2_header_lines(instance);
    let header_h = header_lines.len() as u16;

    // Layout: header | rule | tabs | rule | scrollable body
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(header_h),
            Constraint::Length(1), // ── rule ──
            Constraint::Length(1), // section tab bar
            Constraint::Length(1), // ── rule ──
            Constraint::Min(0),    // scrollable section body
        ])
        .split(inner);

    frame.render_widget(Paragraph::new(header_lines), chunks[0]);
    render_hr(chunks[1], frame);
    render_ec2_section_tabs(app, chunks[2], frame);
    render_hr(chunks[3], frame);
    render_ec2_section_body(app, chunks[4], frame);
}

/// Builds the header as a Vec<Line> so the caller can measure its height before
/// allocating layout space.  One piece of information per line; 2-space indent;
/// HEADER_LABEL_W-char label column for aligned key/value pairs.
pub(super) fn build_ec2_header_lines(instance: &Ec2Instance) -> Vec<Line<'static>> {
    let state_color = match instance.state.as_str() {
        "running" => theme::success(),
        "stopped" => theme::error(),
        "pending" | "stopping" | "shutting-down" => theme::warning(),
        _ => crate::ui::theme::text_muted(),
    };
    let state_dot = match instance.state.as_str() {
        "running" => "● ",
        "stopped" => "○ ",
        _ => "◌ ",
    };

    let name = instance
        .tags
        .get("Name")
        .map(|s| s.as_str())
        .unwrap_or(&instance.instance_id);

    let mut lines: Vec<Line<'static>> = vec![Line::raw("")];

    // Name (prominent)
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            name.to_string(),
            Style::default()
                .fg(crate::ui::theme::text_primary())
                .add_modifier(Modifier::BOLD),
        ),
    ]));

    // Instance ID  ·  ● state  ·  type
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            instance.instance_id.clone(),
            Style::default().fg(theme::text_dim()),
        ),
        Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
        Span::styled(state_dot.to_string(), Style::default().fg(state_color)),
        Span::styled(
            instance.state.clone(),
            Style::default()
                .fg(state_color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
        Span::styled(
            instance.instance_type.clone(),
            Style::default().fg(theme::aws_orange()),
        ),
    ]));

    lines.push(Line::raw(""));
    lines
}

/// One-line tab bar — active tab is orange-on-black; inactive tabs are dimmed.
pub(super) fn render_ec2_section_tabs(app: &App, area: Rect, frame: &mut Frame) {
    render_section_tab_bar(app, area, frame, &descriptor_tabs(app, &crate::aws::services::ec2::EC2_INSTANCE_SECTIONS));
}

/// Scrollable body — manually slices rows to implement scroll offset.
/// Jumpable rows (SG IDs in Security, ENI headers in Networking) get a `→`
/// indicator; when selected the indicator becomes an `⏎` badge.
pub(super) fn render_ec2_section_body(app: &App, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let has_search = focused
        && (!app.detail_search_query.is_empty() || app.detail_search_active);

    let (content_area, search_area) = if has_search {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(1)])
            .split(area);
        (chunks[0], Some(chunks[1]))
    } else {
        (area, None)
    };

    let all_rows = app.get_detail_lines_filtered();
    let cursor = app.details_selected_index.unwrap_or(0);
    let q_lower = app.detail_search_query.to_lowercase();

    let lines = layout_detail_body(app, &all_rows, content_area, cursor, |idx, key, value, key_w| {
        let jump = jump_indicator(app, key, value);
        let is_selected = focused && app.detail_line_in_selection(idx);
        let is_cursor = focused && Some(idx) == app.details_selected_index;
        let mut line = style_detail_row(key, value, None, key_w);

        if is_selected {
            let sel = theme::selection_style(true);
            let mut spans: Vec<Span> = line
                .spans
                .into_iter()
                .map(|s| Span::styled(s.content, sel))
                .collect();
            if jump.is_some() && is_cursor {
                spans.push(Span::styled("  →", Style::default().fg(theme::aws_orange())));
            }
            Line::from(spans).style(sel)
        } else if !q_lower.is_empty() {
            let combined = format!("{}{}", key, value);
            if combined.to_lowercase().contains(&q_lower) {
                let mut spans: Vec<Span> = line
                    .spans
                    .into_iter()
                    .map(|s| Span::styled(s.content, s.style.fg(crate::ui::theme::text_primary())))
                    .collect();
                if jump.is_some() {
                    spans.push(Span::styled("  →", Style::default().fg(theme::text_dim())));
                }
                Line::from(spans)
            } else {
                if jump.is_some() {
                    line.spans.push(Span::styled(
                        "  →",
                        Style::default().fg(theme::text_dim()),
                    ));
                }
                line
            }
        } else {
            if jump.is_some() {
                line.spans.push(Span::styled(
                    "  →",
                    Style::default().fg(theme::text_dim()),
                ));
            }
            line
        }
    });

    frame.render_widget(Paragraph::new(lines), content_area);

    if let Some(sb_area) = search_area {
        let match_count = all_rows.len();
        let match_info = if app.detail_search_query.is_empty() {
            String::new()
        } else {
            format!(
                " ({} match{})",
                match_count,
                if match_count == 1 { "" } else { "es" }
            )
        };
        let search_line = Line::from(vec![
            Span::styled(
                " / ",
                Style::default()
                    .fg(theme::aws_orange())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                app.detail_search_query.clone(),
                Style::default().fg(crate::ui::theme::text_primary()),
            ),
            if app.detail_search_active {
                Span::styled("█", Style::default().fg(theme::aws_orange()))
            } else {
                Span::raw("")
            },
            Span::styled(match_info, Style::default().fg(theme::text_dim())),
            Span::styled(
                "  ⏎ confirm · Esc clear",
                Style::default().fg(theme::text_dim()),
            ),
        ]);
        frame.render_widget(Paragraph::new(search_line), sb_area);
    }
}

// ── EC2 section content builders ──────────────────────────────────────────────

/// Returns the scrollable rows for the active EC2 instance detail section.
/// Called by `App::get_detail_lines` so navigation, scroll, and copy all see
/// the same rows. One `Option<&Lazy<_>>` per lazy section, hence the count.
#[allow(clippy::too_many_arguments)]
pub fn ec2_section_lines(
    instance: &Ec2Instance,
    section: Ec2InstanceDetailSection,
    profile_roles: Option<&crate::lazy::Lazy<Vec<crate::aws::services::ec2::InstanceProfileRole>>>,
    ssm_status: Option<crate::aws::services::ec2::SsmInstanceStatus>,
    user_data: Option<&crate::lazy::Lazy<Option<String>>>,
    console: Option<&crate::lazy::Lazy<Option<crate::aws::services::ec2::ConsoleOutput>>>,
    lb: Option<&Lazy<crate::aws::services::elb::InstanceLbInfo>>,
    optimizer: Option<&Lazy<Option<crate::aws::services::computeoptimizer::OptimizerRec>>>,
    enrollment: Option<&crate::aws::services::computeoptimizer::CoEnrollment>,
) -> Vec<(String, String)> {
    match section {
        Ec2InstanceDetailSection::Details => ec2_details_lines(instance, ssm_status),
        Ec2InstanceDetailSection::Security => ec2_security_lines(instance, profile_roles),
        Ec2InstanceDetailSection::Networking => ec2_networking_lines(instance),
        Ec2InstanceDetailSection::LoadBalancing => ec2_load_balancing_lines(lb),
        Ec2InstanceDetailSection::Storage => ec2_storage_lines(instance),
        Ec2InstanceDetailSection::UserData => ec2_user_data_lines(user_data),
        Ec2InstanceDetailSection::Console => ec2_console_lines(console),
        Ec2InstanceDetailSection::Tags => ec2_tags_lines(instance),
        Ec2InstanceDetailSection::Optimizer => optimizer_lines(optimizer, enrollment),
    }
}

/// User data (`DescribeInstanceAttribute`, base64-decoded) as plain content
/// lines — `e` opens the full script in `$EDITOR` via the snapshot path.
pub(super) fn ec2_user_data_lines(
    user_data: Option<&crate::lazy::Lazy<Option<String>>>,
) -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = Vec::new();
    match user_data {
        None | Some(crate::lazy::Lazy::Loading) => {
            rows.push(("Loading…".to_string(), String::new()));
        }
        Some(crate::lazy::Lazy::Error(e)) => rows.extend(error_rows(e)),
        Some(crate::lazy::Lazy::Loaded(None)) => {
            // Group header (non-empty key, empty value): the common "none" state.
            rows.push(("No user data configured".to_string(), String::new()));
        }
        Some(crate::lazy::Lazy::Loaded(Some(script))) => {
            for line in script.lines() {
                // Leading-space key + empty value → plain content line (no colon).
                rows.push((format!(" {}", line), String::new()));
            }
        }
    }
    rows
}

/// System console output (`GetConsoleOutput`, base64-decoded) — a captured-at
/// row, then one plain content line per log line so `e` opens the whole
/// buffer in `$EDITOR` via the snapshot path.
/// Target-group membership (lazy `DescribeTargetHealth` probe). One group
/// header per target group; the `Target Group` / `Load Balancer` ARN rows
/// jump via the generic ARN classifier.
pub(super) fn ec2_load_balancing_lines(
    lb: Option<&Lazy<crate::aws::services::elb::InstanceLbInfo>>,
) -> Vec<(String, String)> {
    use crate::aws::services::elb::{lb_kind_and_name, MAX_INSTANCE_LB_CANDIDATES};
    let mut rows: Vec<(String, String)> = Vec::new();
    let info = match lb {
        None | Some(Lazy::Loading) => {
            rows.push(("Loading…".to_string(), String::new()));
            return rows;
        }
        Some(Lazy::Error(e)) => {
            rows.extend(error_rows(e));
            return rows;
        }
        Some(Lazy::Loaded(info)) => info,
    };

    let groups: std::collections::BTreeSet<&str> = info
        .memberships
        .iter()
        .map(|m| m.target_group_arn.as_str())
        .collect();
    let healthy = info.memberships.iter().filter(|m| m.state == "healthy").count();
    if info.memberships.is_empty() {
        rows.push(("Behind a load balancer".to_string(), "✗ No".to_string()));
    } else {
        let mut lbs: Vec<String> = info
            .memberships
            .iter()
            .flat_map(|m| m.load_balancer_arns.iter())
            .filter_map(|arn| lb_kind_and_name(arn).map(|(k, n)| format!("{} {}", k, n)))
            .collect();
        lbs.sort();
        lbs.dedup();
        rows.push((
            "Behind a load balancer".to_string(),
            if lbs.is_empty() {
                "⚠ Registered, but no group is attached to a load balancer".to_string()
            } else {
                format!("✓ {}", lbs.join(", "))
            },
        ));
        rows.push((
            "Target groups".to_string(),
            format!(
                "{} · {}/{} target{} healthy",
                groups.len(),
                healthy,
                info.memberships.len(),
                if info.memberships.len() == 1 { "" } else { "s" }
            ),
        ));
    }
    if let Some(asg) = &info.asg_name {
        rows.push((
            "Auto Scaling group".to_string(),
            format!(
                "{} ({} target group{} attached)",
                asg,
                info.asg_target_groups,
                if info.asg_target_groups == 1 { "" } else { "s" }
            ),
        ));
    }
    let checked = if info.candidates > info.checked {
        format!(
            "{} of {} same-VPC target groups (capped at {})",
            info.checked, info.candidates, MAX_INSTANCE_LB_CANDIDATES
        )
    } else {
        format!(
            "{} same-VPC target group{}",
            info.checked,
            if info.checked == 1 { "" } else { "s" }
        )
    };
    rows.push(("Checked".to_string(), checked));
    for w in &info.warnings {
        rows.push((format!(" ⚠ {}", w), String::new()));
    }
    if info.memberships.is_empty() {
        rows.push((String::new(), String::new()));
        rows.push((
            String::new(),
            "· instance- and ip-type groups in this VPC were checked by instance id and private IP"
                .to_string(),
        ));
        return rows;
    }

    for m in &info.memberships {
        rows.push((String::new(), String::new()));
        rows.push((m.target_group_name.clone(), String::new()));
        let health = match (&m.reason, m.state.as_str()) {
            (Some(r), s) if s != "healthy" => format!("{} — {}", s, r),
            (_, s) => s.to_string(),
        };
        rows.push(("Health".to_string(), health));
        if m.state != "healthy" {
            if let Some(d) = &m.description {
                rows.push(("Reason".to_string(), d.clone()));
            }
        }
        let port = match (m.group_port, m.target_port) {
            (Some(g), Some(t)) if g != t => format!("{}:{} → instance :{}", m.protocol, g, t),
            (_, Some(t)) => format!("{}:{}", m.protocol, t),
            (Some(g), None) => format!("{}:{}", m.protocol, g),
            (None, None) => m.protocol.clone(),
        };
        rows.push(("Port".to_string(), port));
        if let Some(ip) = &m.matched_ip {
            rows.push(("Registered as".to_string(), format!("IP {}", ip)));
        }
        if let Some(az) = &m.availability_zone {
            rows.push(("Zone".to_string(), az.clone()));
        }
        if m.via_asg {
            rows.push(("Registered by".to_string(), "Auto Scaling group".to_string()));
        }
        rows.push(("Target Group".to_string(), m.target_group_arn.clone()));
        if m.load_balancer_arns.is_empty() {
            rows.push(("Load Balancer".to_string(), "⚠ none — group not attached".to_string()));
        }
        for arn in &m.load_balancer_arns {
            rows.push(("Load Balancer".to_string(), arn.clone()));
        }
    }
    rows
}

pub(super) fn ec2_console_lines(
    console: Option<&crate::lazy::Lazy<Option<crate::aws::services::ec2::ConsoleOutput>>>,
) -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = Vec::new();
    match console {
        None | Some(crate::lazy::Lazy::Loading) => {
            rows.push(("Loading…".to_string(), String::new()));
        }
        Some(crate::lazy::Lazy::Error(e)) => rows.extend(error_rows(e)),
        Some(crate::lazy::Lazy::Loaded(None)) => {
            rows.push(("No console output available yet".to_string(), String::new()));
            rows.push((
                "".to_string(),
                "· AWS posts the buffer a few minutes after a start, stop or reboot — r to refetch"
                    .to_string(),
            ));
        }
        Some(crate::lazy::Lazy::Loaded(Some(out))) => {
            let captured = match (&out.captured_at, out.captured_secs) {
                (Some(at), Some(secs)) => format!("{at} ({})", console_age(secs)),
                (Some(at), None) => at.clone(),
                _ => "unknown".to_string(),
            };
            rows.push(("Captured".to_string(), captured));
            rows.push(("Lines".to_string(), out.lines.len().to_string()));
            rows.push((String::new(), String::new()));
            for line in &out.lines {
                // Leading-space key + empty value → plain content line (no colon).
                rows.push((format!(" {}", line), String::new()));
            }
        }
    }
    rows
}

/// Age of a console capture relative to now: "2m ago", "3h ago", "5d ago".
pub(super) fn console_age(captured_secs: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(captured_secs);
    let secs = (now - captured_secs).max(0);
    if secs < 60 {
        "just now".to_string()
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86_400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86_400)
    }
}

pub(super) fn ec2_details_lines(
    instance: &Ec2Instance,
    ssm_status: Option<crate::aws::services::ec2::SsmInstanceStatus>,
) -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = Vec::new();

    let name = instance
        .tags
        .get("Name")
        .cloned()
        .unwrap_or_else(|| instance.instance_id.clone());

    rows.push(("Identity".to_string(), "".to_string()));
    rows.push(("".to_string(), "".to_string()));
    rows.push(("  Name".to_string(), name));
    rows.push(("  Instance ID".to_string(), instance.instance_id.clone()));
    rows.push(("  State".to_string(), instance.state.clone()));
    rows.push(("  Type".to_string(), instance.instance_type.clone()));
    // SSM Session Manager connectability (press `s` to connect when online).
    let ssm_label = match ssm_status {
        Some(s) if s.is_connectable() => format!("{} — press s to connect", s.label()),
        Some(s) => s.label().to_string(),
        None => "Not managed".to_string(),
    };
    rows.push(("  SSM".to_string(), ssm_label));

    rows.push(("".to_string(), "".to_string()));
    rows.push(("Network".to_string(), "".to_string()));
    rows.push(("".to_string(), "".to_string()));
    if let Some(ip) = &instance.private_ip {
        rows.push(("  Private IP".to_string(), ip.clone()));
    }
    if let Some(ip) = &instance.public_ip {
        rows.push(("  Public IP".to_string(), ip.clone()));
    }
    if let Some(az) = &instance.availability_zone {
        rows.push(("  AZ".to_string(), az.clone()));
    }
    if let Some(vpc) = &instance.vpc_id {
        rows.push(("  VPC ID".to_string(), vpc.clone()));
    }
    if let Some(sub) = &instance.subnet_id {
        rows.push(("  Subnet ID".to_string(), sub.clone()));
    }

    rows.push(("".to_string(), "".to_string()));
    rows.push(("Launch Config".to_string(), "".to_string()));
    rows.push(("".to_string(), "".to_string()));
    if let Some(t) = &instance.launch_time {
        rows.push(("  Launched".to_string(), t.clone()));
    }
    if let Some(ami) = &instance.ami_id {
        rows.push(("  AMI".to_string(), ami.clone()));
    }
    if let Some(key) = &instance.key_name {
        rows.push(("  Key pair".to_string(), key.clone()));
    }
    if let Some(arch) = &instance.architecture {
        let platform = instance.platform.as_deref().unwrap_or("Linux/UNIX");
        rows.push(("  Platform".to_string(), format!("{platform}  ({arch})")));
    }

    rows
}

pub(super) fn ec2_security_lines(
    instance: &Ec2Instance,
    profile_roles: Option<&crate::lazy::Lazy<Vec<crate::aws::services::ec2::InstanceProfileRole>>>,
) -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = Vec::new();

    // IAM instance-profile block. describe_instances returns only the *instance
    // profile* ARN, not the role; the role(s) inside the profile are resolved
    // lazily via iam:GetInstanceProfile and shown below the profile.
    rows.push(("IAM Instance Profile".to_string(), "".to_string()));
    rows.push(("".to_string(), "".to_string()));
    if let Some(arn) = &instance.iam_profile_arn {
        let profile = arn.rsplit('/').next().unwrap_or(arn.as_str());
        rows.push(("  Profile".to_string(), profile.to_string()));
        rows.push(("  ARN".to_string(), arn.clone()));

        // Role(s) inside the profile (lazy-loaded).
        match profile_roles {
            None | Some(crate::lazy::Lazy::Loading) => {
                rows.push(("  Role".to_string(), "Loading…".to_string()));
            }
            Some(crate::lazy::Lazy::Loaded(roles)) if roles.is_empty() => {
                rows.push(("  Role".to_string(), "(no role in profile)".to_string()));
            }
            Some(crate::lazy::Lazy::Loaded(roles)) => {
                for role in roles {
                    rows.push(("  Role".to_string(), role.role_name.clone()));
                    rows.push(("  Role ARN".to_string(), role.role_arn.clone()));
                }
            }
            Some(crate::lazy::Lazy::Error(e)) => {
                rows.extend(error_rows(e));
            }
        }
    } else {
        rows.push(("  (none)".to_string(), "No IAM instance profile attached".to_string()));
    }
    if let Some(mon) = &instance.monitoring_state {
        rows.push(("".to_string(), "".to_string()));
        rows.push(("  Monitoring".to_string(), mon.clone()));
    }

    // Security groups block
    rows.push(("".to_string(), "".to_string()));
    rows.push(("Security Groups".to_string(), "".to_string()));
    rows.push(("".to_string(), "".to_string()));
    if instance.security_groups.is_empty() {
        rows.push(("  (none)".to_string(), "No security groups attached".to_string()));
    } else {
        for (id, name) in &instance.security_groups {
            rows.push((format!("  {}", id), name.clone()));
        }
    }
    rows.push(("".to_string(), "".to_string()));

    rows
}

pub(super) fn ec2_networking_lines(instance: &Ec2Instance) -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = Vec::new();

    rows.push(("Network Interfaces".to_string(), "".to_string()));

    if instance.network_interfaces.is_empty() {
        rows.push(("".to_string(), "".to_string()));
        rows.push(("  (none)".to_string(), "No network interfaces found".to_string()));
    } else {
        for (i, nic) in instance.network_interfaces.iter().enumerate() {
            rows.push(("".to_string(), "".to_string()));
            // Device header — renders as a group sub-header (magenta)
            rows.push((
                format!("eth{}  {}", i, nic.interface_id),
                "".to_string(),
            ));
            if !nic.status.is_empty() {
                rows.push(("  Status".to_string(), nic.status.clone()));
            }
            if let Some(ip) = &nic.private_ip {
                rows.push(("  Private IP".to_string(), ip.clone()));
            }
            if let Some(ip) = &nic.public_ip {
                rows.push(("  Public IP".to_string(), ip.clone()));
            }
            if let Some(sub) = &nic.subnet_id {
                rows.push(("  Subnet".to_string(), sub.clone()));
            }
            if let Some(vpc) = &nic.vpc_id {
                rows.push(("  VPC".to_string(), vpc.clone()));
            }
            if let Some(mac) = &nic.mac_address {
                rows.push(("  MAC".to_string(), mac.clone()));
            }
            if !nic.security_groups.is_empty() {
                for (sg_id, sg_name) in &nic.security_groups {
                    rows.push((
                        "  Security Group".to_string(),
                        format!("{sg_name}  ({sg_id})"),
                    ));
                }
            }
            if !nic.description.is_empty() {
                rows.push(("  Description".to_string(), nic.description.clone()));
            }
        }
    }

    rows
}

pub(super) fn ec2_storage_lines(instance: &Ec2Instance) -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = Vec::new();

    rows.push(("Block Devices".to_string(), "".to_string()));

    if instance.block_devices.is_empty() {
        rows.push(("".to_string(), "".to_string()));
        rows.push(("  (none)".to_string(), "No block device mappings".to_string()));
    } else {
        for vol in &instance.block_devices {
            rows.push(("".to_string(), "".to_string()));
            // Device header
            let device_label = if vol.is_root {
                format!("{}  (root)", vol.device_name)
            } else {
                vol.device_name.clone()
            };
            rows.push((device_label, "".to_string()));
            rows.push(("  Volume ID".to_string(), vol.volume_id.clone()));
            if !vol.status.is_empty() {
                rows.push(("  Status".to_string(), vol.status.clone()));
            }
            rows.push((
                "  Delete on Term".to_string(),
                if vol.delete_on_termination {
                    "✓ Yes".to_string()
                } else {
                    "✗ No — persists after termination".to_string()
                },
            ));
        }
    }

    if let Some(rt) = &instance.root_device_type {
        rows.push(("".to_string(), "".to_string()));
        rows.push(("Root Device".to_string(), "".to_string()));
        rows.push(("".to_string(), "".to_string()));
        rows.push(("  Type".to_string(), rt.clone()));
    }

    rows
}

pub(super) fn ec2_tags_lines(instance: &Ec2Instance) -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = Vec::new();

    if instance.tags.is_empty() {
        rows.push(("  (none)".to_string(), "No tags".to_string()));
    } else {
        let mut sorted: Vec<_> = instance.tags.iter().collect();
        sorted.sort_by_key(|(k, _)| k.as_str());
        for (key, value) in sorted {
            rows.push((format!("  {}", key), value.clone()));
        }
    }

    rows
}

// ── Security group split pane (EC2 + VPC, shared) ────────────────────────────

#[allow(clippy::too_many_arguments)]
pub(super) fn render_security_group_split(
    app: &App,
    name: &str,
    group_id: &str,
    vpc_id: Option<&str>,
    description: &str,
    area: Rect,
    frame: &mut Frame,
) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 4, "");
    let mut block = theme::pane_block("Security Group", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let subtitle = match vpc_id {
        Some(v) if !v.is_empty() => format!("{}  ·  {}", group_id, v),
        _ => group_id.to_string(),
    };
    let mut header = vec![
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
            Span::styled(subtitle, Style::default().fg(theme::text_dim())),
        ]),
    ];
    if !description.is_empty() {
        header.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(description.to_string(), Style::default().fg(theme::text_dim())),
        ]));
    }
    header.push(Line::raw(""));
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
    render_section_tab_bar(app, chunks[2], frame, &descriptor_tabs(app, &crate::aws::services::ec2::SECURITY_GROUP_SECTIONS));
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub fn security_group_section_lines(
    section: SecurityGroupDetailSection,
    inbound: &[crate::aws::services::ec2::SgRule],
    outbound: &[crate::aws::services::ec2::SgRule],
    tags: &std::collections::HashMap<String, String>,
    used_by: Option<&crate::lazy::Lazy<Vec<crate::aws::services::ec2::SgEni>>>,
) -> Vec<(String, String)> {
    use crate::aws::services::ec2::sg_rule_rows;
    match section {
        // Rule rows already carry the sg-/pl-/CIDR token, so referenced security
        // groups are Enter-jumpable via the generic resource_jump_target.
        SecurityGroupDetailSection::Inbound => sg_rule_rows("Inbound", inbound),
        SecurityGroupDetailSection::Outbound => sg_rule_rows("Outbound", outbound),
        SecurityGroupDetailSection::UsedBy => {
            let mut rows = vec![("".to_string(), "".to_string())];
            match used_by {
                None | Some(crate::lazy::Lazy::Loading) => {
                    rows.push(("  Loading…".to_string(), "".to_string()));
                }
                Some(crate::lazy::Lazy::Error(e)) => {
                    rows.extend(error_rows(e));
                }
                Some(crate::lazy::Lazy::Loaded(list)) if list.is_empty() => {
                    rows.push((
                        "  Not attached to any network interface.".to_string(),
                        "".to_string(),
                    ));
                }
                Some(crate::lazy::Lazy::Loaded(list)) => {
                    rows.push((format!("Network Interfaces ({})", list.len()), "".to_string()));
                    rows.push(("".to_string(), "".to_string()));
                    for e in list {
                        // eni id in the key → Enter jumps to the network interface.
                        let mut ctx = Vec::new();
                        if !e.attached_to.is_empty() {
                            ctx.push(e.attached_to.clone());
                        }
                        if !e.private_ip.is_empty() {
                            ctx.push(e.private_ip.clone());
                        }
                        if !e.status.is_empty() {
                            ctx.push(e.status.clone());
                        }
                        rows.push((e.eni_id.clone(), ctx.join("  ·  ")));
                        if !e.description.is_empty() {
                            rows.push((format!("  {}", e.description), "".to_string()));
                        }
                    }
                }
            }
            rows
        }
        SecurityGroupDetailSection::Tags => tag_rows(tags),
    }
}

// ── EBS volume split pane ────────────────────────────────────────────────────

pub(super) fn render_ebs_volume_split(
    app: &App,
    vol: &crate::aws::services::ec2::EbsVolume,
    area: Rect,
    frame: &mut Frame,
) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 4, "");
    let mut block = theme::pane_block("EBS Volume", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let (dot, dot_label) = match vol.state.as_str() {
        "in-use" => (theme::success(), "in-use"),
        "available" => (theme::warning(), "available"),
        "error" => (theme::error(), "error"),
        other => (theme::text_dim(), other),
    };
    let name = vol.name();
    let header = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                name.to_string(),
                Style::default().fg(crate::ui::theme::text_primary()).add_modifier(Modifier::BOLD),
            ),
            Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
            Span::styled("● ", Style::default().fg(dot)),
            Span::styled(dot_label.to_string(), Style::default().fg(dot)),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                format!(
                    "{}  ·  {} GiB  ·  {}",
                    vol.volume_type, vol.size_gb, vol.availability_zone
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
    render_section_tab_bar(app, chunks[2], frame, &descriptor_tabs(app, &crate::aws::services::ec2::EBS_VOLUME_SECTIONS));
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub fn ebs_volume_section_lines(
    vol: &crate::aws::services::ec2::EbsVolume,
    section: EbsVolumeDetailSection,
    snapshots: Option<&crate::lazy::Lazy<Vec<crate::aws::services::ec2::EbsSnapshot>>>,
    optimizer: Option<&Lazy<Option<crate::aws::services::computeoptimizer::OptimizerRec>>>,
    enrollment: Option<&crate::aws::services::computeoptimizer::CoEnrollment>,
) -> Vec<(String, String)> {
    match section {
        EbsVolumeDetailSection::Details => {
            let mut rows = vec![
                ("Volume ID".to_string(), vol.volume_id.clone()),
                ("Type".to_string(), vol.volume_type.clone()),
                ("Size".to_string(), format!("{} GiB", vol.size_gb)),
                ("State".to_string(), vol.state.clone()),
                ("Availability Zone".to_string(), vol.availability_zone.clone()),
            ];
            if let Some(i) = vol.iops {
                rows.push(("IOPS".to_string(), i.to_string()));
            }
            if let Some(t) = vol.throughput {
                rows.push(("Throughput".to_string(), format!("{} MiB/s", t)));
            }
            rows.push((
                "Encrypted".to_string(),
                if vol.encrypted { "✓ yes".to_string() } else { "✗ no".to_string() },
            ));
            if let Some(k) = &vol.kms_key_id {
                rows.push(("KMS Key".to_string(), k.clone()));
            }
            if vol.multi_attach {
                rows.push(("Multi-Attach".to_string(), "enabled".to_string()));
            }
            if !vol.create_time.is_empty() {
                rows.push(("Created".to_string(), vol.create_time.clone()));
            }
            if let Some(s) = &vol.snapshot_id {
                // snap- source → jumps to the Snapshots sub-tab.
                rows.push(("Source Snapshot".to_string(), s.clone()));
            }
            rows
        }
        EbsVolumeDetailSection::Attachments => {
            if vol.attachments.is_empty() {
                return vec![("  Not attached.".to_string(), String::new())];
            }
            let mut rows = vec![(
                format!("Attachments ({})", vol.attachments.len()),
                String::new(),
            )];
            rows.push((String::new(), String::new()));
            for a in &vol.attachments {
                // instance id in value → Enter jumps to the instance.
                rows.push(("Instance".to_string(), a.instance_id.clone()));
                if !a.device.is_empty() {
                    rows.push(("  Device".to_string(), a.device.clone()));
                }
                if !a.state.is_empty() {
                    rows.push(("  State".to_string(), a.state.clone()));
                }
                rows.push((
                    "  Delete on Termination".to_string(),
                    if a.delete_on_termination { "yes".to_string() } else { "no".to_string() },
                ));
                rows.push((String::new(), String::new()));
            }
            rows
        }
        EbsVolumeDetailSection::Snapshots => {
            let mut rows = vec![("".to_string(), "".to_string())];
            match snapshots {
                None | Some(crate::lazy::Lazy::Loading) => {
                    rows.push(("  Loading…".to_string(), "".to_string()));
                }
                Some(crate::lazy::Lazy::Error(e)) => {
                    rows.extend(error_rows(e));
                }
                Some(crate::lazy::Lazy::Loaded(list)) if list.is_empty() => {
                    rows.push(("  No snapshots of this volume.".to_string(), "".to_string()));
                }
                Some(crate::lazy::Lazy::Loaded(list)) => {
                    rows.push((format!("Snapshots ({})", list.len()), "".to_string()));
                    rows.push(("".to_string(), "".to_string()));
                    for s in list {
                        let status = if s.state == "completed" {
                            s.state.clone()
                        } else {
                            format!("{} {}", s.state, s.progress)
                        };
                        rows.push((s.snapshot_id.clone(), status));
                        if !s.started.is_empty() {
                            rows.push(("  Started".to_string(), s.started.clone()));
                        }
                        if !s.description.is_empty() {
                            rows.push((format!("  {}", s.description), "".to_string()));
                        }
                        rows.push(("".to_string(), "".to_string()));
                    }
                }
            }
            rows
        }
        EbsVolumeDetailSection::Tags => tag_rows(&vol.tags),
        EbsVolumeDetailSection::Optimizer => optimizer_lines(optimizer, enrollment),
    }
}

// ── AMI split pane ──────────────────────────────────────────────────────────

pub(super) fn render_ami_split(app: &App, ami: &crate::aws::services::ec2::Ami, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 4, "");
    let mut block = theme::pane_block("AMI", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let (dot, dot_label) = match ami.state.as_str() {
        "available" => (theme::success(), "available"),
        "pending" | "transient" => (theme::warning(), ami.state.as_str()),
        "failed" | "invalid" | "error" => (theme::error(), ami.state.as_str()),
        "deregistered" => (theme::text_dim(), "deregistered"),
        other => (theme::text_dim(), other),
    };
    let header = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                ami.name().to_string(),
                Style::default().fg(crate::ui::theme::text_primary()).add_modifier(Modifier::BOLD),
            ),
            Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
            Span::styled("● ", Style::default().fg(dot)),
            Span::styled(dot_label.to_string(), Style::default().fg(dot)),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                format!("{}  ·  {}", ami.image_id, ami.architecture),
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
    render_section_tab_bar(app, chunks[2], frame, &descriptor_tabs(app, &crate::aws::services::ec2::AMI_SECTIONS));
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub fn ami_section_lines(
    ami: &crate::aws::services::ec2::Ami,
    section: AmiDetailSection,
    permissions: Option<&crate::lazy::Lazy<Vec<String>>>,
) -> Vec<(String, String)> {
    match section {
        AmiDetailSection::Details => {
            let mut rows = vec![
                ("Image ID".to_string(), ami.image_id.clone()),
                ("Name".to_string(), ami.name.clone()),
            ];
            if !ami.description.is_empty() {
                rows.push(("Description".to_string(), ami.description.clone()));
            }
            rows.push(("State".to_string(), ami.state.clone()));
            rows.push(("Architecture".to_string(), ami.architecture.clone()));
            rows.push(("Virtualization".to_string(), ami.virtualization_type.clone()));
            rows.push(("Root Device Type".to_string(), ami.root_device_type.clone()));
            if !ami.root_device_name.is_empty() {
                rows.push(("Root Device".to_string(), ami.root_device_name.clone()));
            }
            if !ami.hypervisor.is_empty() {
                rows.push(("Hypervisor".to_string(), ami.hypervisor.clone()));
            }
            rows.push((
                "ENA Support".to_string(),
                if ami.ena_support { "✓ yes".to_string() } else { "✗ no".to_string() },
            ));
            rows.push((
                "Platform".to_string(),
                ami.platform.clone().unwrap_or_else(|| "linux".to_string()),
            ));
            rows.push((
                "Public".to_string(),
                if ami.public { "⚠ yes".to_string() } else { "no".to_string() },
            ));
            if let Some(c) = &ami.creation_date {
                rows.push(("Created".to_string(), c.clone()));
            }
            if let Some(d) = &ami.deprecation_time {
                rows.push(("Deprecation".to_string(), d.clone()));
            }
            rows
        }
        AmiDetailSection::BlockDevices => {
            if ami.block_devices.is_empty() {
                return vec![("  No EBS block devices.".to_string(), String::new())];
            }
            let mut rows = vec![(
                format!("Block Devices ({})", ami.block_devices.len()),
                String::new(),
            )];
            rows.push((String::new(), String::new()));
            for b in &ami.block_devices {
                rows.push(("Device".to_string(), b.device_name.clone()));
                if let Some(s) = &b.snapshot_id {
                    // snap- → Enter jumps to the Snapshots sub-tab.
                    rows.push(("Snapshot".to_string(), s.clone()));
                }
                if b.size_gb > 0 {
                    rows.push(("  Size".to_string(), format!("{} GiB", b.size_gb)));
                }
                if !b.volume_type.is_empty() {
                    rows.push(("  Type".to_string(), b.volume_type.clone()));
                }
                rows.push((
                    "  Delete on Termination".to_string(),
                    if b.delete_on_termination { "yes".to_string() } else { "no".to_string() },
                ));
                rows.push((String::new(), String::new()));
            }
            rows
        }
        AmiDetailSection::Permissions => {
            let mut rows = vec![(String::new(), String::new())];
            match permissions {
                None | Some(crate::lazy::Lazy::Loading) => {
                    rows.push(("  Loading…".to_string(), String::new()));
                }
                Some(crate::lazy::Lazy::Error(e)) => {
                    rows.extend(error_rows(e));
                }
                Some(crate::lazy::Lazy::Loaded(list)) if list.is_empty() => {
                    rows.push((
                        "  Private — not shared with any account.".to_string(),
                        String::new(),
                    ));
                }
                Some(crate::lazy::Lazy::Loaded(list)) => {
                    rows.push((format!("Shared with ({})", list.len()), String::new()));
                    rows.push((String::new(), String::new()));
                    for p in list {
                        if p == "public" {
                            rows.push(("Public".to_string(), "⚠ shared with all".to_string()));
                        } else {
                            rows.push(("Account".to_string(), p.clone()));
                        }
                    }
                }
            }
            rows
        }
        AmiDetailSection::Tags => tag_rows(&ami.tags),
    }
}

// ── EBS snapshot split pane (account-wide Snapshots sub-tab) ─────────────────

pub(super) fn render_snapshot_split(
    app: &App,
    snap: &crate::aws::services::ec2::EbsSnapshot,
    area: Rect,
    frame: &mut Frame,
) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 2, "");
    let mut block = theme::pane_block("EBS Snapshot", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let (dot, dot_label) = match snap.state.as_str() {
        "completed" => (theme::success(), "completed"),
        "pending" => (theme::warning(), "pending"),
        "recoverable" => (theme::warning(), "recoverable"),
        "error" => (theme::error(), "error"),
        other => (theme::text_dim(), other),
    };
    let header = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                snap.name().to_string(),
                Style::default().fg(crate::ui::theme::text_primary()).add_modifier(Modifier::BOLD),
            ),
            Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
            Span::styled("● ", Style::default().fg(dot)),
            Span::styled(dot_label.to_string(), Style::default().fg(dot)),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                format!("{}  ·  {} GiB", snap.snapshot_id, snap.size_gb),
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
    render_section_tab_bar(app, chunks[2], frame, &descriptor_tabs(app, &crate::aws::services::ec2::EBS_SNAPSHOT_SECTIONS));
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub fn snapshot_section_lines(
    snap: &crate::aws::services::ec2::EbsSnapshot,
    section: SnapshotDetailSection,
) -> Vec<(String, String)> {
    match section {
        SnapshotDetailSection::Details => {
            let mut rows = vec![
                ("Snapshot ID".to_string(), snap.snapshot_id.clone()),
                ("State".to_string(), snap.state.clone()),
            ];
            if let Some(m) = &snap.state_message {
                rows.push(("State Message".to_string(), m.clone()));
            }
            rows.push(("Progress".to_string(), snap.progress.clone()));
            if !snap.started.is_empty() {
                rows.push(("Started".to_string(), snap.started.clone()));
            }
            if !snap.description.is_empty() {
                rows.push(("Description".to_string(), snap.description.clone()));
            }
            if let Some(v) = &snap.volume_id {
                // vol- → Enter jumps to EBS Volumes.
                rows.push(("Source Volume".to_string(), v.clone()));
            }
            rows.push(("Volume Size".to_string(), format!("{} GiB", snap.size_gb)));
            rows.push((
                "Encrypted".to_string(),
                if snap.encrypted { "✓ yes".to_string() } else { "✗ no".to_string() },
            ));
            if let Some(k) = &snap.kms_key_id {
                rows.push(("KMS Key".to_string(), k.clone()));
            }
            if let Some(o) = &snap.owner_id {
                rows.push(("Owner".to_string(), o.clone()));
            }
            if let Some(t) = &snap.storage_tier {
                rows.push(("Storage Tier".to_string(), t.clone()));
            }
            if let Some(r) = &snap.restore_expiry {
                rows.push(("Restore Expiry".to_string(), r.clone()));
            }
            rows
        }
        SnapshotDetailSection::Tags => tag_rows(&snap.tags),
    }
}

// ── Launch template split pane ──────────────────────────────────────────────

pub(super) fn render_launch_template_split(
    app: &App,
    lt: &crate::aws::services::ec2::LaunchTemplate,
    area: Rect,
    frame: &mut Frame,
) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 4, "");
    let mut block = theme::pane_block("Launch Template", focused);
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
                lt.name().to_string(),
                Style::default().fg(crate::ui::theme::text_primary()).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                format!(
                    "{}  ·  default v{}  ·  latest v{}",
                    lt.id, lt.default_version, lt.latest_version
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
    render_section_tab_bar(app, chunks[2], frame, &descriptor_tabs(app, &crate::aws::services::ec2::LAUNCH_TEMPLATE_SECTIONS));
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub fn launch_template_section_lines(
    lt: &crate::aws::services::ec2::LaunchTemplate,
    section: LaunchTemplateDetailSection,
    versions: Option<&crate::lazy::Lazy<Vec<crate::aws::services::ec2::LaunchTemplateVersion>>>,
) -> Vec<(String, String)> {
    match section {
        LaunchTemplateDetailSection::Details => {
            let mut rows = vec![
                ("Template ID".to_string(), lt.id.clone()),
                ("Name".to_string(), lt.name.clone()),
                ("Default Version".to_string(), lt.default_version.to_string()),
                ("Latest Version".to_string(), lt.latest_version.to_string()),
            ];
            if !lt.created_by.is_empty() {
                rows.push(("Created By".to_string(), lt.created_by.clone()));
            }
            if let Some(c) = &lt.create_time {
                rows.push(("Created".to_string(), c.clone()));
            }
            rows
        }
        LaunchTemplateDetailSection::Versions => {
            let mut rows = vec![(String::new(), String::new())];
            match versions {
                None | Some(crate::lazy::Lazy::Loading) => {
                    rows.push(("  Loading…".to_string(), String::new()));
                }
                Some(crate::lazy::Lazy::Error(e)) => {
                    rows.extend(error_rows(e));
                }
                Some(crate::lazy::Lazy::Loaded(list)) if list.is_empty() => {
                    rows.push(("  No versions.".to_string(), String::new()));
                }
                Some(crate::lazy::Lazy::Loaded(list)) => {
                    rows.push((format!("Versions ({})", list.len()), String::new()));
                    rows.push((String::new(), String::new()));
                    for v in list {
                        let label = if v.is_default {
                            format!("v{} (default)", v.version_number)
                        } else {
                            format!("v{}", v.version_number)
                        };
                        rows.push((label, String::new()));
                        if !v.description.is_empty() {
                            rows.push((format!("  {}", v.description), String::new()));
                        }
                        if !v.created_by.is_empty() {
                            rows.push(("  Created By".to_string(), v.created_by.clone()));
                        }
                        if let Some(c) = &v.create_time {
                            rows.push(("  Created".to_string(), c.clone()));
                        }
                        rows.push((String::new(), String::new()));
                    }
                }
            }
            rows
        }
        LaunchTemplateDetailSection::Data => {
            let mut rows = vec![(String::new(), String::new())];
            match versions {
                None | Some(crate::lazy::Lazy::Loading) => {
                    rows.push(("  Loading…".to_string(), String::new()));
                }
                Some(crate::lazy::Lazy::Error(e)) => {
                    rows.extend(error_rows(e));
                }
                Some(crate::lazy::Lazy::Loaded(list)) => {
                    let chosen = list
                        .iter()
                        .find(|v| v.is_default || v.version_number == lt.default_version)
                        .or_else(|| list.first());
                    match chosen {
                        Some(v) => {
                            rows.push((
                                format!("Default version data (v{})", v.version_number),
                                String::new(),
                            ));
                            rows.push((String::new(), String::new()));
                            for line in v.data_json.lines() {
                                rows.push((format!(" {}", line), String::new()));
                            }
                            rows.push((String::new(), String::new()));
                        }
                        None => {
                            rows.push(("  No version data.".to_string(), String::new()));
                        }
                    }
                }
            }
            rows
        }
        LaunchTemplateDetailSection::Tags => tag_rows(&lt.tags),
    }
}

pub(super) fn render_eni_split(app: &App, eni: &NetworkInterface, area: Rect, frame: &mut Frame) {
    let name = eni
        .tags
        .get("Name")
        .map(|s| s.as_str())
        .unwrap_or(&eni.interface_id);
    let subtitle = format!("{} · {}", eni.interface_type, eni.status);
    render_simple_split(
        app,
        area,
        frame,
        "Network Interface",
        name,
        &subtitle,
        &descriptor_tabs(app, &crate::aws::services::ec2::ENI_SECTIONS),
    );
}

pub fn eni_section_lines(
    eni: &NetworkInterface,
    section: EniDetailSection,
) -> Vec<(String, String)> {
    match section {
        EniDetailSection::Overview => {
            let mut rows = vec![
                (String::new(), String::new()),
                ("Interface ID".to_string(), eni.interface_id.clone()),
                ("Status".to_string(), eni.status.clone()),
                ("Type".to_string(), eni.interface_type.clone()),
            ];
            if !eni.description.is_empty() {
                rows.push(("Description".to_string(), eni.description.clone()));
            }
            if let Some(v) = &eni.vpc_id {
                rows.push(("VPC".to_string(), v.clone()));
            }
            if let Some(s) = &eni.subnet_id {
                rows.push(("Subnet".to_string(), s.clone()));
            }
            if let Some(z) = &eni.availability_zone {
                rows.push(("Availability Zone".to_string(), z.clone()));
            }
            if let Some(o) = &eni.owner_id {
                rows.push(("Owner".to_string(), o.clone()));
            }
            // Managed-service ENIs (ELB / Lambda / RDS …) carry the
            // service's requester id — the "what made this?" signal.
            if let Some(r) = &eni.requester_id {
                let val = if eni.requester_managed {
                    format!("{} (managed)", r)
                } else {
                    r.clone()
                };
                rows.push(("Requester".to_string(), val));
            }
            if !eni.security_groups.is_empty() {
                rows.push((String::new(), String::new()));
                rows.push(("Security Groups".to_string(), String::new()));
                for (id, name) in &eni.security_groups {
                    let key = if name.is_empty() {
                        "  Group".to_string()
                    } else {
                        format!("  {}", name)
                    };
                    rows.push((key, id.clone()));
                }
            }
            rows
        }
        EniDetailSection::Addresses => {
            let mut rows = vec![(String::new(), String::new())];
            rows.push(("IPv4 Addresses".to_string(), String::new()));
            if eni.private_ips.is_empty() {
                if let Some(ip) = &eni.private_ip {
                    rows.push(("  Private IP".to_string(), ip.clone()));
                } else {
                    rows.push(("  none".to_string(), String::new()));
                }
            }
            for (ip, primary) in &eni.private_ips {
                let key = if *primary {
                    "  Private IP (primary)"
                } else {
                    "  Private IP"
                };
                rows.push((key.to_string(), ip.clone()));
            }
            if let Some(ip) = &eni.public_ip {
                rows.push(("  Public IP".to_string(), ip.clone()));
            }
            if let Some(dns) = &eni.private_dns {
                rows.push(("  Private DNS".to_string(), dns.clone()));
            }
            if !eni.ipv6_addresses.is_empty() {
                rows.push((String::new(), String::new()));
                rows.push(("IPv6 Addresses".to_string(), String::new()));
                for a in &eni.ipv6_addresses {
                    rows.push(("  IPv6".to_string(), a.clone()));
                }
            }
            rows
        }
        EniDetailSection::Attachment => {
            let mut rows = vec![(String::new(), String::new())];
            match &eni.attachment {
                None => rows.push(("  Not attached".to_string(), String::new())),
                Some(a) => {
                    if let Some(i) = &a.instance_id {
                        rows.push(("Instance".to_string(), i.clone()));
                    }
                    if let Some(o) = &a.instance_owner {
                        rows.push(("Attached By".to_string(), o.clone()));
                    }
                    if let Some(s) = &a.status {
                        rows.push(("Attachment Status".to_string(), s.clone()));
                    }
                    if let Some(d) = a.device_index {
                        rows.push(("Device Index".to_string(), d.to_string()));
                    }
                    rows.push((
                        "Delete on Termination".to_string(),
                        if a.delete_on_termination {
                            "✓ yes".to_string()
                        } else {
                            "no".to_string()
                        },
                    ));
                }
            }
            rows
        }
        EniDetailSection::Tags => tag_rows(&eni.tags),
    }
}
