use super::*;

pub(super) fn render_rds_snapshot_split(app: &App, snap: &RdsSnapshot, area: Rect, frame: &mut Frame) {
    let subtitle = format!("{} {}", snap.engine, snap.engine_version);
    render_simple_split(
        app,
        area,
        frame,
        "RDS Snapshot",
        &snap.id,
        &subtitle,
        &descriptor_tabs(app, &crate::aws::services::rds::RDS_SNAPSHOT_SECTIONS),
    );
}

pub fn rds_snapshot_section_lines(
    snap: &RdsSnapshot,
    section: RdsSnapshotDetailSection,
) -> Vec<(String, String)> {
    match section {
        RdsSnapshotDetailSection::Overview => {
            let mut rows = vec![
                (String::new(), String::new()),
                ("Snapshot ID".to_string(), snap.id.clone()),
                (
                    if snap.is_cluster {
                        "Source Cluster".to_string()
                    } else {
                        "Source DB".to_string()
                    },
                    snap.source.clone(),
                ),
                ("Type".to_string(), snap.snapshot_type.clone()),
                (
                    "Engine".to_string(),
                    format!("{} {}", snap.engine, snap.engine_version),
                ),
                ("Status".to_string(), snap.status.clone()),
            ];
            if snap.status == "creating" {
                rows.push(("Progress".to_string(), format!("⚠ {}%", snap.percent_progress)));
            }
            if let Some(src) = &snap.copy_source {
                rows.push(("Copied From".to_string(), src.clone()));
            }
            if snap.allocated_storage > 0 {
                rows.push((
                    "Allocated Storage".to_string(),
                    format!("{} GiB", snap.allocated_storage),
                ));
            }
            if let Some(st) = &snap.storage_type {
                rows.push(("Storage Type".to_string(), st.clone()));
            }
            if let Some(tp) = snap.storage_throughput {
                rows.push(("Storage Throughput".to_string(), format!("{} MiB/s", tp)));
            }
            if let Some(lm) = &snap.license_model {
                rows.push(("License Model".to_string(), lm.clone()));
            }
            rows.push((
                "IAM Auth".to_string(),
                if snap.iam_auth_enabled { "Enabled".to_string() } else { "Disabled".to_string() },
            ));

            rows.push((String::new(), String::new()));
            if let Some(vpc) = &snap.vpc_id {
                rows.push(("VPC".to_string(), vpc.clone()));
            }
            if let Some(az) = &snap.availability_zone {
                rows.push(("Availability Zone".to_string(), az.clone()));
            }
            if let Some(port) = snap.port {
                if port > 0 {
                    rows.push(("Port".to_string(), port.to_string()));
                }
            }

            rows.push((String::new(), String::new()));
            if let Some(c) = &snap.created {
                rows.push(("Created".to_string(), c.clone()));
            }
            if let Some(c) = &snap.source_created {
                rows.push((
                    if snap.is_cluster {
                        "Cluster Created".to_string()
                    } else {
                        "Instance Created".to_string()
                    },
                    c.clone(),
                ));
            }
            rows
        }
        RdsSnapshotDetailSection::Encryption => {
            let mut rows = vec![
                (String::new(), String::new()),
                (
                    "Encrypted".to_string(),
                    if snap.encrypted {
                        "✓ yes".to_string()
                    } else {
                        "✗ no".to_string()
                    },
                ),
            ];
            if let Some(k) = &snap.kms_key_id {
                rows.push(("KMS Key".to_string(), k.clone()));
            }
            if !snap.encrypted {
                rows.push((String::new(), String::new()));
                rows.push((
                    "  Snapshot storage is not encrypted at rest.".to_string(),
                    String::new(),
                ));
            }
            rows
        }
        RdsSnapshotDetailSection::Tags => tag_rows(&snap.tags),
    }
}

pub(super) fn render_rds_param_group_split(
    app: &App,
    pg: &crate::aws::services::rds::RdsParamGroup,
    area: Rect,
    frame: &mut Frame,
) {
    let subtitle = format!(
        "{} · {}",
        pg.family,
        if pg.is_cluster { "cluster" } else { "instance" },
    );
    render_simple_split(
        app,
        area,
        frame,
        "RDS Parameter Group",
        &pg.name,
        &subtitle,
        &descriptor_tabs(app, &crate::aws::services::rds::RDS_PARAM_GROUP_SECTIONS),
    );
}

pub fn rds_param_group_section_lines(
    pg: &crate::aws::services::rds::RdsParamGroup,
    section: crate::aws::services::rds::RdsParamGroupDetailSection,
    params_state: Option<
        &crate::lazy::Lazy<(Vec<crate::aws::services::rds::RdsParameter>, usize)>,
    >,
) -> Vec<(String, String)> {
    use crate::aws::services::rds::RdsParamGroupDetailSection;
    let mut rows = vec![(String::new(), String::new())];
    match section {
        RdsParamGroupDetailSection::Overview => {
            rows.push(("Name".to_string(), pg.name.clone()));
            rows.push((
                "Kind".to_string(),
                if pg.is_cluster {
                    "cluster parameter group".to_string()
                } else {
                    "instance parameter group".to_string()
                },
            ));
            rows.push(("Family".to_string(), pg.family.clone()));
            rows.push(("Description".to_string(), pg.description.clone()));
            if !pg.arn.is_empty() {
                rows.push(("ARN".to_string(), pg.arn.clone()));
            }
            if pg.name.starts_with("default.") {
                rows.push((String::new(), String::new()));
                rows.push((
                    "  · engine default — not modifiable".to_string(),
                    String::new(),
                ));
            }
        }
        RdsParamGroupDetailSection::Parameters => match params_state {
            None | Some(crate::lazy::Lazy::Loading) => {
                rows.push(("  Loading…".to_string(), String::new()));
            }
            Some(crate::lazy::Lazy::Error(e)) => rows.extend(error_rows(e)),
            Some(crate::lazy::Lazy::Loaded((params, total))) => {
                let modified = params.iter().filter(|p| p.source == "user").count();
                let header = if *total > params.len() {
                    format!(
                        "Parameters — {} modified · showing {} of {}",
                        modified,
                        params.len(),
                        total
                    )
                } else {
                    format!("Parameters ({}) — {} modified", params.len(), modified)
                };
                rows.push((header, String::new()));
                rows.push((String::new(), String::new()));
                if params.is_empty() {
                    rows.push(("  No parameters".to_string(), String::new()));
                }
                // The fetch sorts user-overridden first; split the two blocks
                // with group headers when both exist.
                if modified > 0 {
                    rows.push(("Modified".to_string(), String::new()));
                    rows.push((String::new(), String::new()));
                }
                let mut in_defaults = modified == 0;
                for p in params {
                    if !in_defaults && p.source != "user" {
                        in_defaults = true;
                        rows.push((String::new(), String::new()));
                        rows.push(("Engine Defaults".to_string(), String::new()));
                        rows.push((String::new(), String::new()));
                    }
                    let value = match &p.value {
                        Some(v) if !v.is_empty() => v.clone(),
                        _ => "—".to_string(),
                    };
                    let mut label = format!("  {}", p.name);
                    if p.apply_type == "static" {
                        // Static params need a reboot to apply.
                        label.push_str(" ·static");
                    }
                    rows.push((label, value));
                }
            }
        },
    }
    rows.push((String::new(), String::new()));
    rows
}

// ── RDS Instance Split Pane ───────────────────────────────────────────────────

pub(super) fn render_rds_instance_split(app: &App, db: &RdsInstance, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let extras = if crate::aws::services::rds::RdsInstanceDetailSection::from_index(
        app.detail_section_index(),
    ) == crate::aws::services::rds::RdsInstanceDetailSection::Logs
    {
        "e open file"
    } else {
        ""
    };
    let footer = detail_footer(app, focused, 9, extras);

    let mut block = theme::pane_block("RDS Instance", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let header_lines = build_rds_header_lines(db);
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
    render_rds_section_tabs(app, chunks[2], frame);
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub(super) fn build_rds_header_lines(db: &RdsInstance) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = vec![Line::raw("")];

    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            db.db_identifier.clone(),
            Style::default()
                .fg(crate::ui::theme::text_primary())
                .add_modifier(Modifier::BOLD),
        ),
    ]));

    lines.push(Line::raw(""));
    lines.push(header_kv(
        "Engine",
        &format!("{} {}", db.engine, db.engine_version),
    ));
    lines.push(header_kv("Class", &db.db_instance_class));
    lines.push(header_kv("Status", &db.status));
    if let Some(addr) = &db.endpoint_address {
        let port_str = db.endpoint_port.map(|p| format!(":{}", p)).unwrap_or_default();
        lines.push(header_kv("Endpoint", &format!("{}{}", addr, port_str)));
    }
    lines.push(header_kv("AZ", &db.availability_zone));

    if !db.pending_modifications.is_empty() {
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(
                format!(
                    "⚠ {} pending modification{}",
                    db.pending_modifications.len(),
                    if db.pending_modifications.len() == 1 { "" } else { "s" },
                ),
                Style::default().fg(theme::warning()),
            ),
        ]));
    }

    lines.push(Line::raw(""));
    lines
}

pub(super) fn render_rds_section_tabs(app: &App, area: Rect, frame: &mut Frame) {
    render_section_tab_bar(
        app,
        area,
        frame,
        &descriptor_tabs(app, &crate::aws::services::rds::RDS_INSTANCE_SECTIONS),
    );
}

pub fn rds_instance_section_lines(
    db: &RdsInstance,
    section: RdsInstanceDetailSection,
    pi_state: Option<&crate::lazy::Lazy<crate::aws::services::rds::RdsPiData>>,
    snapshots: &[&RdsSnapshot],
    maint_state: Option<&crate::lazy::Lazy<Vec<crate::aws::services::rds::RdsPendingAction>>>,
    events_state: Option<&crate::lazy::Lazy<Vec<crate::aws::services::rds::RdsEvent>>>,
    logs_state: Option<
        &crate::lazy::Lazy<(Vec<crate::aws::services::rds::RdsLogFile>, usize)>,
    >,
) -> Vec<(String, String)> {
    match section {
        RdsInstanceDetailSection::Config => rds_config_lines(db),
        RdsInstanceDetailSection::Storage => rds_storage_lines(db),
        RdsInstanceDetailSection::Network => rds_network_lines(db),
        RdsInstanceDetailSection::PerfInsights => rds_pi_lines(db, pi_state),
        RdsInstanceDetailSection::Backups => rds_instance_backups_lines(db, snapshots),
        RdsInstanceDetailSection::Maintenance => rds_maintenance_lines(
            &db.preferred_maintenance_window,
            db.auto_minor_version_upgrade,
            db.ca_certificate_identifier.as_deref(),
            db.ca_valid_till.as_deref(),
            &db.pending_modifications,
            maint_state,
        ),
        RdsInstanceDetailSection::Events => rds_events_lines(events_state),
        RdsInstanceDetailSection::Logs => rds_logs_lines(logs_state),
        RdsInstanceDetailSection::Tags => rds_tags_lines(db),
    }
}

/// The instance Logs section: native DB log files (error/, postgresql.log.*)
/// from `DescribeDBLogFiles` — `e` on a "Log File" row downloads its tail
/// into `$EDITOR`.
pub(super) fn rds_logs_lines(
    logs_state: Option<&crate::lazy::Lazy<(Vec<crate::aws::services::rds::RdsLogFile>, usize)>>,
) -> Vec<(String, String)> {
    let mut rows = vec![(String::new(), String::new())];

    match logs_state {
        None | Some(crate::lazy::Lazy::Loading) => {
            rows.push(("  Loading…".to_string(), String::new()));
        }
        Some(crate::lazy::Lazy::Error(e)) => rows.extend(error_rows(e)),
        Some(crate::lazy::Lazy::Loaded((files, total))) => {
            let header = if *total > files.len() {
                format!("Log Files — latest {} of {}", files.len(), total)
            } else {
                format!("Log Files ({})", files.len())
            };
            rows.push((header, String::new()));
            rows.push((String::new(), String::new()));
            if files.is_empty() {
                rows.push(("  No log files reported".to_string(), String::new()));
            } else {
                rows.push((
                    "  · e on a row opens the file's tail in $EDITOR".to_string(),
                    String::new(),
                ));
                rows.push((String::new(), String::new()));
                for f in files {
                    rows.push(("  Log File".to_string(), f.name.clone()));
                    let mut info = format!("    · {}", fmt_bytes(f.size_bytes));
                    if let Some(w) = &f.last_written {
                        info.push_str(&format!(" · last written {}", w));
                    }
                    rows.push((info, String::new()));
                }
            }
        }
    }

    rows.push((String::new(), String::new()));
    rows
}

/// The sibling-snapshot listing both Backups sections share — one jumpable
/// "Snapshot" row per snapshot (newest first) plus a dim annotation line.
pub(super) fn rds_snapshot_rows(snapshots: &[&RdsSnapshot]) -> Vec<(String, String)> {
    let mut rows = vec![
        (String::new(), String::new()),
        (format!("Snapshots ({})", snapshots.len()), String::new()),
        (String::new(), String::new()),
    ];
    if snapshots.is_empty() {
        rows.push(("  No snapshots for this database".to_string(), String::new()));
        return rows;
    }
    let mut sorted: Vec<&&RdsSnapshot> = snapshots.iter().collect();
    sorted.sort_by(|a, b| b.created.cmp(&a.created));
    for s in sorted {
        rows.push(("  Snapshot".to_string(), s.id.clone()));
        let mut info = format!("    · {} · {}", s.snapshot_type, s.status);
        if s.allocated_storage > 0 {
            info.push_str(&format!(" · {} GiB", s.allocated_storage));
        }
        if let Some(c) = &s.created {
            info.push_str(&format!(" · {}", c));
        }
        rows.push((info, String::new()));
    }
    rows
}

/// The Maintenance section both RDS panes share: window + auto-upgrade + CA
/// certificate, the eager pending-modification rows, and the lazy pending
/// maintenance actions.
pub(super) fn rds_maintenance_lines(
    maintenance_window: &str,
    auto_minor_upgrade: bool,
    ca_cert: Option<&str>,
    ca_valid_till: Option<&str>,
    pending_modifications: &[(String, String)],
    maint_state: Option<&crate::lazy::Lazy<Vec<crate::aws::services::rds::RdsPendingAction>>>,
) -> Vec<(String, String)> {
    let mut rows = vec![(String::new(), String::new())];

    rows.push(("Maintenance Window".to_string(), maintenance_window.to_string()));
    rows.push((
        "Auto Minor Upgrade".to_string(),
        if auto_minor_upgrade { "✓ enabled".to_string() } else { "disabled".to_string() },
    ));
    if let Some(cert) = ca_cert {
        rows.push(("CA Certificate".to_string(), cert.to_string()));
        if let Some(till) = ca_valid_till {
            rows.push(("  Valid Until".to_string(), till.to_string()));
        }
    }

    rows.push((String::new(), String::new()));
    rows.push((
        format!("Pending Modifications ({})", pending_modifications.len()),
        String::new(),
    ));
    rows.push((String::new(), String::new()));
    if pending_modifications.is_empty() {
        rows.push(("  · none".to_string(), String::new()));
    } else {
        for (k, v) in pending_modifications {
            rows.push((format!("  ⚠ {}", k), v.clone()));
        }
    }

    rows.push((String::new(), String::new()));
    rows.push(("Pending Maintenance Actions".to_string(), String::new()));
    rows.push((String::new(), String::new()));
    match maint_state {
        None | Some(crate::lazy::Lazy::Loading) => {
            rows.push(("  Loading…".to_string(), String::new()));
        }
        Some(crate::lazy::Lazy::Error(e)) => rows.extend(error_rows(e)),
        Some(crate::lazy::Lazy::Loaded(actions)) => {
            if actions.is_empty() {
                rows.push(("  · none scheduled".to_string(), String::new()));
            } else {
                for a in actions {
                    rows.push((format!("  ⚠ {}", a.action), String::new()));
                    if let Some(d) = &a.description {
                        rows.push((format!("    · {}", d), String::new()));
                    }
                    if let Some(d) = &a.current_apply_date {
                        rows.push(("    Applies".to_string(), d.clone()));
                    }
                    if let Some(d) = &a.auto_applied_after {
                        rows.push(("    Auto-Applied After".to_string(), d.clone()));
                    }
                    if let Some(d) = &a.forced_apply_date {
                        rows.push(("    Forced By".to_string(), d.clone()));
                    }
                    if let Some(s) = &a.opt_in_status {
                        if !s.is_empty() {
                            rows.push(("    Opt-In Status".to_string(), s.clone()));
                        }
                    }
                    rows.push((String::new(), String::new()));
                }
            }
        }
    }

    rows.push((String::new(), String::new()));
    rows
}

/// The lazy Events section both RDS panes share (last 14 days, newest first).
pub(super) fn rds_events_lines(
    events_state: Option<&crate::lazy::Lazy<Vec<crate::aws::services::rds::RdsEvent>>>,
) -> Vec<(String, String)> {
    let mut rows = vec![(String::new(), String::new())];

    match events_state {
        None | Some(crate::lazy::Lazy::Loading) => {
            rows.push(("  Loading…".to_string(), String::new()));
        }
        Some(crate::lazy::Lazy::Error(e)) => rows.extend(error_rows(e)),
        Some(crate::lazy::Lazy::Loaded(events)) => {
            rows.push((
                format!("Events — last 14 days ({})", events.len()),
                String::new(),
            ));
            rows.push((String::new(), String::new()));
            if events.is_empty() {
                rows.push(("  No events in the window".to_string(), String::new()));
            } else {
                for e in events {
                    // "failure"-category events get the warning glyph so they pop.
                    let warn = e.categories.iter().any(|c| c.contains("failure"));
                    let msg = if warn {
                        format!("⚠ {}", e.message)
                    } else {
                        e.message.clone()
                    };
                    rows.push((format!("  {}", e.date), msg));
                    if !e.categories.is_empty() {
                        rows.push((format!("    · {}", e.categories.join(", ")), String::new()));
                    }
                }
            }
        }
    }

    rows.push((String::new(), String::new()));
    rows
}

/// The instance Backups section: automated-backup posture + sibling snapshots.
pub(super) fn rds_instance_backups_lines(db: &RdsInstance, snapshots: &[&RdsSnapshot]) -> Vec<(String, String)> {
    let mut rows = vec![(String::new(), String::new())];

    rows.push(("Automated Backups".to_string(), String::new()));
    rows.push((String::new(), String::new()));
    if db.backup_retention_days > 0 {
        rows.push(("  Retention".to_string(), format!("{} days", db.backup_retention_days)));
        rows.push(("  Backup Window".to_string(), db.preferred_backup_window.clone()));
        if let Some(t) = &db.latest_restorable_time {
            rows.push(("  Latest Restorable".to_string(), t.clone()));
        }
    } else {
        rows.push((
            "  ⚠ Automated backups are disabled (retention 0)".to_string(),
            String::new(),
        ));
    }
    rows.push((
        "  Copy Tags to Snapshot".to_string(),
        if db.copy_tags_to_snapshot { "✓ yes".to_string() } else { "no".to_string() },
    ));
    if let Some(t) = &db.backup_target {
        if t != "region" {
            rows.push(("  Backup Target".to_string(), t.clone()));
        }
    }

    rows.extend(rds_snapshot_rows(snapshots));
    rows.push((String::new(), String::new()));
    rows
}

pub(super) fn rds_pi_lines(
    db: &RdsInstance,
    pi_state: Option<&crate::lazy::Lazy<crate::aws::services::rds::RdsPiData>>,
) -> Vec<(String, String)> {
    let mut rows = vec![("".to_string(), "".to_string())];

    if !db.performance_insights_enabled {
        rows.push(("  Performance Insights is not enabled".to_string(), "".to_string()));
        rows.push(("".to_string(), "".to_string()));
        rows.push((
            "  Enable it on the instance to see the average active-session".to_string(),
            "".to_string(),
        ));
        rows.push((
            "  load broken down by wait event.".to_string(),
            "".to_string(),
        ));
        return rows;
    }

    match pi_state {
        None | Some(crate::lazy::Lazy::Loading) => {
            rows.push(("  Loading Performance Insights…".to_string(), "".to_string()));
        }
        Some(crate::lazy::Lazy::Error(e)) => {
            rows.extend(error_rows(e));
        }
        Some(crate::lazy::Lazy::Loaded(data)) => {
            rows.push((
                "DB Load — avg active sessions (last 1h)".to_string(),
                "".to_string(),
            )); // group header
            rows.push(("Total Load".to_string(), format!("{:.2}", data.total_load)));
            rows.push(("".to_string(), "".to_string()));
            if data.waits.is_empty() {
                rows.push(("  No wait-event activity in the window".to_string(), "".to_string()));
            } else {
                rows.push(("Top Wait Events".to_string(), "".to_string())); // group header
                for (name, load) in &data.waits {
                    rows.push((format!("  {}", name), format!("{:.3}", load)));
                }
            }
        }
    }

    rows.push(("".to_string(), "".to_string()));
    rows
}

pub(super) fn rds_config_lines(db: &RdsInstance) -> Vec<(String, String)> {
    let mut rows = vec![];

    rows.push(("Identifier".to_string(), db.db_identifier.clone()));
    rows.push((
        "Engine".to_string(),
        format!("{} {}", db.engine, db.engine_version),
    ));
    rows.push(("Instance Class".to_string(), db.db_instance_class.clone()));
    rows.push(("Status".to_string(), db.status.clone()));
    rows.push(("".to_string(), "".to_string()));

    rows.push((
        "Multi-AZ".to_string(),
        if db.multi_az { "Yes".to_string() } else { "No".to_string() },
    ));
    rows.push(("Availability Zone".to_string(), db.availability_zone.clone()));

    if let Some(cluster) = &db.cluster_identifier {
        rows.push(("".to_string(), "".to_string()));
        rows.push(("Cluster".to_string(), cluster.clone()));
    }

    rows.push(("".to_string(), "".to_string()));
    rows.push(("Master Username".to_string(), db.master_username.clone()));
    if let Some(db_name) = &db.db_name {
        rows.push(("DB Name".to_string(), db_name.clone()));
    }

    rows.push(("".to_string(), "".to_string()));
    rows.push((
        "IAM Auth".to_string(),
        if db.iam_auth_enabled { "Enabled".to_string() } else { "Disabled".to_string() },
    ));
    rows.push((
        "Deletion Protection".to_string(),
        if db.deletion_protection { "On".to_string() } else { "Off".to_string() },
    ));
    rows.push((
        "Publicly Accessible".to_string(),
        if db.publicly_accessible { "Yes".to_string() } else { "No".to_string() },
    ));
    if let Some(lm) = &db.license_model {
        rows.push(("License Model".to_string(), lm.clone()));
    }

    if !db.parameter_groups.is_empty() || !db.option_groups.is_empty() {
        rows.push(("".to_string(), "".to_string()));
        // Apply status is the reboot-needed signal: "pending-reboot" means a
        // static parameter changed but hasn't taken effect yet.
        for (name, status) in &db.parameter_groups {
            let status = if status == "in-sync" {
                status.clone()
            } else {
                format!("⚠ {}", status)
            };
            rows.push(("Parameter Group".to_string(), format!("{} ({})", name, status)));
        }
        for (name, status) in &db.option_groups {
            rows.push(("Option Group".to_string(), format!("{} ({})", name, status)));
        }
    }

    if db.replica_source.is_some() || !db.replica_ids.is_empty() {
        rows.push(("".to_string(), "".to_string()));
        rows.push(("Replication".to_string(), "".to_string()));
        if let Some(src) = &db.replica_source {
            rows.push(("  Replica Source".to_string(), src.clone()));
        }
        if let Some(mode) = &db.replica_mode {
            rows.push(("  Replica Mode".to_string(), mode.clone()));
        }
        for id in &db.replica_ids {
            rows.push(("  Read Replica".to_string(), id.clone()));
        }
    }

    rows.push(("".to_string(), "".to_string()));
    rows.push(("Monitoring".to_string(), "".to_string()));
    rows.push((
        "  Enhanced Monitoring".to_string(),
        if db.monitoring_interval > 0 {
            format!("✓ every {}s", db.monitoring_interval)
        } else {
            "disabled".to_string()
        },
    ));
    if let Some(role) = &db.monitoring_role_arn {
        rows.push(("  Monitoring Role".to_string(), role.clone()));
    }
    rows.push((
        "  Performance Insights".to_string(),
        if db.performance_insights_enabled {
            match db.pi_retention_days {
                Some(d) => format!("✓ enabled · {} days retention", d),
                None => "✓ enabled".to_string(),
            }
        } else {
            "disabled".to_string()
        },
    ));
    if !db.enabled_log_exports.is_empty() {
        rows.push((
            "  Log Exports".to_string(),
            db.enabled_log_exports.join(", "),
        ));
    }

    if !db.create_time.is_empty() {
        rows.push(("".to_string(), "".to_string()));
        rows.push(("Created".to_string(), db.create_time.clone()));
    }

    rows.push(("".to_string(), "".to_string()));
    rows
}

pub(super) fn rds_storage_lines(db: &RdsInstance) -> Vec<(String, String)> {
    let mut rows = vec![];

    rows.push(("Storage Type".to_string(), db.storage_type.clone()));
    rows.push(("Allocated Storage".to_string(), format!("{} GiB", db.allocated_storage_gb)));
    match db.max_allocated_storage_gb {
        Some(max) => rows.push((
            "Storage Autoscaling".to_string(),
            format!("✓ up to {} GiB", max),
        )),
        None => rows.push(("Storage Autoscaling".to_string(), "disabled".to_string())),
    }
    if let Some(iops) = db.iops {
        rows.push(("Provisioned IOPS".to_string(), format!("{}", iops)));
    }
    if let Some(tp) = db.storage_throughput {
        rows.push(("Storage Throughput".to_string(), format!("{} MiB/s", tp)));
    }
    rows.push(("".to_string(), "".to_string()));
    rows.push((
        "Encrypted".to_string(),
        if db.storage_encrypted { "✓ Yes".to_string() } else { "✗ No".to_string() },
    ));
    if let Some(kms) = &db.kms_key_id {
        rows.push(("KMS Key".to_string(), kms.clone()));
    }

    rows.push(("".to_string(), "".to_string()));
    rows
}

pub(super) fn rds_network_lines(db: &RdsInstance) -> Vec<(String, String)> {
    let mut rows = vec![];

    rows.push(("Endpoint".to_string(), db.endpoint_display()));
    if let Some(port) = db.endpoint_port {
        rows.push(("Port".to_string(), format!("{}", port)));
    }
    if let Some(nt) = &db.network_type {
        rows.push(("Network Type".to_string(), nt.clone()));
    }
    rows.push(("Availability Zone".to_string(), db.availability_zone.clone()));
    if let Some(az) = &db.secondary_availability_zone {
        rows.push(("Secondary AZ".to_string(), az.clone()));
    }

    rows.push(("".to_string(), "".to_string()));
    if let Some(vpc) = &db.vpc_id {
        rows.push(("VPC".to_string(), vpc.clone()));
    }
    if let Some(sg) = &db.subnet_group {
        rows.push(("Subnet Group".to_string(), sg.clone()));
    }
    for subnet in &db.subnet_ids {
        rows.push(("  Subnet".to_string(), subnet.clone()));
    }

    if !db.vpc_security_groups.is_empty() {
        rows.push(("".to_string(), "".to_string()));
        rows.push((
            "Security Groups".to_string(),
            format!("{} attached", db.vpc_security_groups.len()),
        ));
        for sg_id in &db.vpc_security_groups {
            rows.push((format!("  {}", sg_id), "".to_string()));
        }
    }

    rows.push(("".to_string(), "".to_string()));
    rows
}

pub(super) fn rds_tags_lines(db: &RdsInstance) -> Vec<(String, String)> {
    let mut rows = vec![("".to_string(), "".to_string())];

    if db.tags.is_empty() {
        rows.push(("  No tags".to_string(), "".to_string()));
    } else {
        let mut sorted: Vec<(&String, &String)> = db.tags.iter().collect();
        sorted.sort_by_key(|(k, _)| k.as_str());
        for (key, value) in sorted {
            rows.push((format!("  {}", key), value.clone()));
        }
    }

    rows.push(("".to_string(), "".to_string()));
    rows
}

// ── RDS Cluster split pane ────────────────────────────────────────────────

pub(super) fn render_rds_cluster_split(app: &App, c: &RdsCluster, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 7, "");

    let mut block = theme::pane_block("RDS Cluster", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut header_lines: Vec<Line<'static>> = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                c.cluster_identifier.clone(),
                Style::default().fg(crate::ui::theme::text_primary()).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::raw(""),
    ];
    header_lines.push(header_kv("Engine", &format!("{} {}", c.engine, c.engine_version)));
    header_lines.push(header_kv("Mode", &c.engine_mode));
    header_lines.push(header_kv("Status", &c.status));
    header_lines.push(header_kv("Members", &c.members.len().to_string()));
    if !c.pending_modifications.is_empty() {
        header_lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(
                format!(
                    "⚠ {} pending modification{}",
                    c.pending_modifications.len(),
                    if c.pending_modifications.len() == 1 { "" } else { "s" },
                ),
                Style::default().fg(theme::warning()),
            ),
        ]));
    }
    header_lines.push(Line::raw(""));
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
    render_rds_cluster_section_tabs(app, chunks[2], frame);
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub(super) fn render_rds_cluster_section_tabs(app: &App, area: Rect, frame: &mut Frame) {
    render_section_tab_bar(
        app,
        area,
        frame,
        &descriptor_tabs(app, &crate::aws::services::rds::RDS_CLUSTER_SECTIONS),
    );
}

pub fn rds_cluster_section_lines(
    c: &RdsCluster,
    section: RdsClusterDetailSection,
    snapshots: &[&RdsSnapshot],
    maint_state: Option<&crate::lazy::Lazy<Vec<crate::aws::services::rds::RdsPendingAction>>>,
    events_state: Option<&crate::lazy::Lazy<Vec<crate::aws::services::rds::RdsEvent>>>,
) -> Vec<(String, String)> {
    let mut rows = vec![("".to_string(), "".to_string())];
    match section {
        RdsClusterDetailSection::Maintenance => {
            return rds_maintenance_lines(
                &c.preferred_maintenance_window,
                c.auto_minor_version_upgrade,
                None,
                None,
                &c.pending_modifications,
                maint_state,
            );
        }
        RdsClusterDetailSection::Events => return rds_events_lines(events_state),
        RdsClusterDetailSection::Config => {
            rows.push(("Cluster ID".to_string(), c.cluster_identifier.clone()));
            rows.push(("Engine".to_string(), format!("{} {}", c.engine, c.engine_version)));
            rows.push(("Mode".to_string(), c.engine_mode.clone()));
            rows.push(("Status".to_string(), c.status.clone()));
            rows.push(("Master User".to_string(), c.master_username.clone()));
            if let Some(db) = &c.db_name {
                rows.push(("Database".to_string(), db.clone()));
            }
            rows.push((
                "Multi-AZ".to_string(),
                if c.multi_az { "Yes" } else { "No" }.to_string(),
            ));
            if let Some((min, max)) = c.serverless_v2_acu {
                rows.push((
                    "Serverless v2".to_string(),
                    format!("{} – {} ACU", min, max),
                ));
            }
            if let Some(pg) = &c.cluster_parameter_group {
                rows.push(("Parameter Group".to_string(), pg.clone()));
            }

            rows.push(("".to_string(), "".to_string()));
            rows.push(("Storage".to_string(), "".to_string()));
            if let Some(st) = &c.storage_type {
                rows.push(("  Storage Type".to_string(), st.clone()));
            }
            if let Some(gb) = c.allocated_storage_gb {
                // Aurora reports 1 (storage is dynamic); only Multi-AZ DB
                // clusters carry a real allocation.
                if gb > 1 {
                    rows.push(("  Allocated Storage".to_string(), format!("{} GiB", gb)));
                }
            }
            if let Some(iops) = c.iops {
                rows.push(("  Provisioned IOPS".to_string(), iops.to_string()));
            }
            rows.push((
                "  Encrypted".to_string(),
                if c.storage_encrypted { "✓ Yes" } else { "✗ No" }.to_string(),
            ));
            if let Some(kms) = &c.kms_key_id {
                rows.push(("  KMS Key".to_string(), kms.clone()));
            }

            rows.push(("".to_string(), "".to_string()));
            rows.push((
                "IAM Auth".to_string(),
                if c.iam_auth_enabled { "Enabled" } else { "Disabled" }.to_string(),
            ));
            rows.push((
                "Deletion Protection".to_string(),
                if c.deletion_protection { "✓ On" } else { "✗ Off" }.to_string(),
            ));
            if !c.enabled_log_exports.is_empty() {
                rows.push(("Log Exports".to_string(), c.enabled_log_exports.join(", ")));
            }

            if c.replication_source.is_some()
                || !c.read_replica_identifiers.is_empty()
                || c.global_write_forwarding_status.is_some()
            {
                rows.push(("".to_string(), "".to_string()));
                rows.push(("Replication".to_string(), "".to_string()));
                if let Some(src) = &c.replication_source {
                    rows.push(("  Replication Source".to_string(), src.clone()));
                }
                for id in &c.read_replica_identifiers {
                    rows.push(("  Read Replica".to_string(), id.clone()));
                }
                if let Some(fw) = &c.global_write_forwarding_status {
                    rows.push(("  Write Forwarding".to_string(), fw.clone()));
                }
            }

            if !c.create_time.is_empty() {
                rows.push(("".to_string(), "".to_string()));
                rows.push(("Created".to_string(), c.create_time.clone()));
            }
        }
        RdsClusterDetailSection::Endpoints => {
            let port = c.port.map(|p| format!(":{}", p)).unwrap_or_default();
            rows.push(("Endpoints".to_string(), "".to_string()));
            rows.push(("".to_string(), "".to_string()));
            match &c.endpoint {
                Some(w) => rows.push(("  Writer".to_string(), format!("{}{}", w, port))),
                None => rows.push(("  Writer".to_string(), "—".to_string())),
            }
            match &c.reader_endpoint {
                Some(r) => rows.push(("  Reader".to_string(), format!("{}{}", r, port))),
                None => rows.push(("  Reader".to_string(), "—".to_string())),
            }
            if let Some(nt) = &c.network_type {
                rows.push(("  Network Type".to_string(), nt.clone()));
            }
            rows.push(("".to_string(), "".to_string()));
            if let Some(sg) = &c.subnet_group {
                rows.push(("Subnet Group".to_string(), sg.clone()));
            }
            if !c.vpc_security_groups.is_empty() {
                rows.push((
                    "Security Groups".to_string(),
                    format!("{} attached", c.vpc_security_groups.len()),
                ));
                for sg_id in &c.vpc_security_groups {
                    rows.push((format!("  {}", sg_id), "".to_string()));
                }
            }
            rows.push(("".to_string(), "".to_string()));
            rows.push(("Availability Zones".to_string(), "".to_string()));
            rows.push(("".to_string(), "".to_string()));
            if c.availability_zones.is_empty() {
                rows.push(("  (none)".to_string(), "".to_string()));
            } else {
                for az in &c.availability_zones {
                    rows.push((format!("  {}", az), "".to_string()));
                }
            }
        }
        RdsClusterDetailSection::Members => {
            rows.push((format!("Members ({})", c.members.len()), "".to_string()));
            rows.push(("".to_string(), "".to_string()));
            if c.members.is_empty() {
                rows.push(("  (none)".to_string(), "".to_string()));
            } else {
                // Writer first; member ids jump to the Instances sub-tab via
                // the label-keyed rds_row_jump_target.
                let mut sorted: Vec<&(String, bool)> = c.members.iter().collect();
                sorted.sort_by_key(|(_, writer)| !writer);
                for (id, is_writer) in sorted {
                    let role = if *is_writer { "  Writer" } else { "  Reader" };
                    rows.push((role.to_string(), id.clone()));
                }
            }
        }
        RdsClusterDetailSection::Backups => {
            rows.push(("Automated Backups".to_string(), "".to_string()));
            rows.push(("".to_string(), "".to_string()));
            rows.push((
                "  Retention".to_string(),
                format!("{} days", c.backup_retention_days),
            ));
            if !c.preferred_backup_window.is_empty() {
                rows.push(("  Backup Window".to_string(), c.preferred_backup_window.clone()));
            }
            if let Some(t) = &c.earliest_restorable_time {
                rows.push(("  Earliest Restorable".to_string(), t.clone()));
            }
            if let Some(t) = &c.latest_restorable_time {
                rows.push(("  Latest Restorable".to_string(), t.clone()));
            }
            rows.push((
                "  Copy Tags to Snapshot".to_string(),
                if c.copy_tags_to_snapshot { "✓ yes".to_string() } else { "no".to_string() },
            ));
            match c.backtrack_window_secs {
                Some(secs) if secs > 0 => {
                    rows.push(("  Backtrack".to_string(), format!("✓ {} h window", secs / 3600)));
                }
                _ => {}
            }
            rows.extend(rds_snapshot_rows(snapshots));
        }
        RdsClusterDetailSection::Tags => {
            if c.tags.is_empty() {
                rows.push(("  No tags".to_string(), "".to_string()));
            } else {
                let mut sorted: Vec<(&String, &String)> = c.tags.iter().collect();
                sorted.sort_by_key(|(k, _)| k.as_str());
                for (k, v) in sorted {
                    rows.push((format!("  {}", k), v.clone()));
                }
            }
        }
    }
    rows.push(("".to_string(), "".to_string()));
    rows
}
