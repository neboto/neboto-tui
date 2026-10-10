use super::*;

// ── DMS split panes ─────────────────────────────────────────────────────────

pub(super) fn render_dms_task_split(
    app: &App,
    t: &crate::aws::services::dms::DmsTask,
    area: Rect,
    frame: &mut Frame,
) {
    let subtitle = format!("{} · {}", t.migration_type, t.state_label());
    render_simple_split(
        app,
        area,
        frame,
        "DMS Replication Task",
        &t.identifier,
        &subtitle,
        &descriptor_tabs(app, &crate::aws::services::dms::DMS_TASK_SECTIONS),
    );
}

pub(super) fn render_dms_instance_split(
    app: &App,
    i: &crate::aws::services::dms::DmsInstance,
    area: Rect,
    frame: &mut Frame,
) {
    let subtitle = format!("{} · {}", i.class, i.status);
    render_simple_split(
        app,
        area,
        frame,
        "DMS Replication Instance",
        &i.identifier,
        &subtitle,
        &descriptor_tabs(app, &crate::aws::services::dms::DMS_INSTANCE_SECTIONS),
    );
}

pub(super) fn render_dms_endpoint_split(
    app: &App,
    e: &crate::aws::services::dms::DmsEndpoint,
    area: Rect,
    frame: &mut Frame,
) {
    let subtitle = format!("{} · {} · {}", e.endpoint_type, e.engine_label(), e.state_label());
    render_simple_split(
        app,
        area,
        frame,
        "DMS Endpoint",
        &e.identifier,
        &subtitle,
        &descriptor_tabs(app, &crate::aws::services::dms::DMS_ENDPOINT_SECTIONS),
    );
}

pub(super) fn render_dms_serverless_split(
    app: &App,
    s: &crate::aws::services::dms::DmsServerless,
    area: Rect,
    frame: &mut Frame,
) {
    let subtitle = format!("{} · {}", s.replication_type, s.state_label());
    render_simple_split(
        app,
        area,
        frame,
        "DMS Serverless Replication",
        &s.identifier,
        &subtitle,
        &descriptor_tabs(app, &crate::aws::services::dms::DMS_SERVERLESS_SECTIONS),
    );
}

/// "2h 14m" / "37s" from milliseconds.
pub(super) fn dms_elapsed(ms: i64) -> String {
    let secs = ms / 1000;
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    if h > 0 {
        format!("{h}h {m}m")
    } else if m > 0 {
        format!("{m}m {s}s")
    } else {
        format!("{s}s")
    }
}

/// Source/target endpoint block: name + engine, then the ARN row (which is
/// what `⏎` follows to the Endpoints tab).
pub(super) fn dms_endpoint_rows(
    rows: &mut Vec<(String, String)>,
    heading: &str,
    resolved: &Option<(String, String)>,
    arn: &str,
) {
    if arn.is_empty() {
        return;
    }
    rows.push((heading.to_string(), String::new()));
    match resolved {
        Some((name, engine)) => rows.push(kv("  Endpoint", format!("{name} ({engine})"))),
        None => rows.push(kv("  Endpoint", "· not in the endpoints list")),
    }
    rows.push(kv("  Endpoint ARN", arn));
}

/// Progress block shared by provisioned tasks and serverless replications.
pub(super) fn dms_stats_rows(rows: &mut Vec<(String, String)>, stats: &crate::aws::services::dms::DmsStats) {
    rows.push((String::new(), String::new()));
    rows.push(("Progress".to_string(), String::new()));
    rows.push(kv("  Full Load", format!("{}%", stats.full_load_pct)));
    let tables = format!(
        "{} loaded · {} loading · {} queued · {} errored",
        stats.tables_loaded, stats.tables_loading, stats.tables_queued, stats.tables_errored
    );
    rows.push(kv(
        "  Tables",
        if stats.tables_errored > 0 {
            format!("✗ {tables}")
        } else {
            tables
        },
    ));
    if stats.elapsed_ms > 0 {
        rows.push(kv("  Elapsed", dms_elapsed(stats.elapsed_ms)));
    }
    for (k, v) in [
        ("  Started", &stats.started),
        ("  Stopped", &stats.stopped),
        ("  Full Load Started", &stats.full_load_started),
        ("  Full Load Finished", &stats.full_load_finished),
    ] {
        if let Some(v) = v {
            rows.push(kv(k, v.clone()));
        }
    }
}

/// Table-statistics body (tasks and serverless share it): errored tables
/// first and marked `✗`, one fixed-width line per table.
pub(super) fn dms_table_stats_rows(
    stats: Option<&crate::lazy::Lazy<crate::aws::services::dms::DmsTableStats>>,
) -> Vec<(String, String)> {
    use crate::lazy::Lazy;
    let data = match stats {
        None | Some(Lazy::Loading) => return vec![("".to_string(), "Loading…".to_string())],
        Some(Lazy::Error(e)) => return error_rows(e),
        Some(Lazy::Loaded(d)) => d,
    };
    if data.tables.is_empty() {
        return vec![(
            "".to_string(),
            "No table statistics yet — the task hasn't started loading tables".to_string(),
        )];
    }
    let errored = data.tables.iter().filter(|t| t.is_error()).count();
    let mut rows = vec![kv("Tables", data.tables.len().to_string())];
    rows.push(kv(
        "Errored",
        if errored > 0 {
            format!("✗ {errored}")
        } else {
            "✓ 0".to_string()
        },
    ));
    if data.truncated {
        rows.push(kv(
            "",
            format!(
                "· first {} tables shown (cap)",
                crate::aws::services::dms::MAX_TABLE_STATS
            ),
        ));
    }
    rows.push((String::new(), String::new()));
    rows.push((
        format!(
            "    {:<22} {:<40} {:>10} {:>9} {:>9} {:>9}  {}",
            "STATE", "TABLE", "FULL LOAD", "INSERTS", "UPDATES", "DELETES", "VALIDATION"
        ),
        String::new(),
    ));
    for t in &data.tables {
        let mark = if t.is_error() { "✗ " } else { "  " };
        let mut validation = t.validation_state.clone().unwrap_or_default();
        if t.validation_failed > 0 {
            validation = format!("{validation} ({} failed)", t.validation_failed);
        }
        let mut full = t.full_load_rows.to_string();
        if t.full_load_error_rows > 0 {
            full = format!("{full} ({} err)", t.full_load_error_rows);
        }
        rows.push((
            format!(
                "  {mark}{:<22} {:<40} {:>10} {:>9} {:>9} {:>9}  {}",
                truncate_chars(&t.state, 22),
                truncate_chars(&format!("{}.{}", t.schema, t.table), 40),
                full,
                t.inserts,
                t.updates,
                t.deletes,
                validation
            ),
            String::new(),
        ));
    }
    rows
}

/// Pretty JSON as plain content lines, under a group header.
pub(super) fn dms_json_rows(rows: &mut Vec<(String, String)>, heading: &str, json: Option<&str>) {
    rows.push((heading.to_string(), String::new()));
    match json {
        Some(j) if !j.trim().is_empty() => {
            for line in crate::aws::services::dms::pretty_json(j).lines() {
                rows.push((format!("  {line}"), String::new()));
            }
        }
        _ => rows.push(("".to_string(), "· none".to_string())),
    }
}

pub fn dms_task_section_lines(
    t: &crate::aws::services::dms::DmsTask,
    section: crate::aws::services::dms::DmsTaskDetailSection,
    tables: Option<&crate::lazy::Lazy<crate::aws::services::dms::DmsTableStats>>,
    assessments: Option<&crate::lazy::Lazy<Vec<crate::aws::services::dms::DmsAssessmentRun>>>,
) -> Vec<(String, String)> {
    use crate::aws::services::dms::DmsTaskDetailSection as S;
    use crate::lazy::Lazy;
    match section {
        S::Overview => {
            let mut rows = vec![
                kv("Task", t.identifier.clone()),
                kv("Status", t.state_label()),
                kv("Migration Type", t.migration_type.clone()),
            ];
            if let Some(r) = &t.stop_reason {
                let v = if t.stopped_on_error() { format!("✗ {r}") } else { r.clone() };
                rows.push(kv("Stop Reason", v));
            }
            if let Some(f) = &t.last_failure {
                rows.push(kv("Last Failure", format!("✗ {f}")));
            }
            if let Some(stats) = &t.stats {
                dms_stats_rows(&mut rows, stats);
            }
            rows.push((String::new(), String::new()));
            dms_endpoint_rows(&mut rows, "Source", &t.source_endpoint, &t.source_endpoint_arn);
            dms_endpoint_rows(&mut rows, "Target", &t.target_endpoint, &t.target_endpoint_arn);
            if !t.instance_arn.is_empty() {
                rows.push(("Replication Instance".to_string(), String::new()));
                rows.push(kv(
                    "  Instance",
                    t.instance_id.clone().unwrap_or_else(|| "· not in the instances list".to_string()),
                ));
                rows.push(kv("  Instance ARN", t.instance_arn.clone()));
            }
            rows.push((String::new(), String::new()));
            rows.push(("CDC".to_string(), String::new()));
            for (k, v) in [
                ("  Start Position", &t.cdc_start_position),
                ("  Stop Position", &t.cdc_stop_position),
                ("  Recovery Checkpoint", &t.recovery_checkpoint),
            ] {
                rows.push(kv(k, v.clone().unwrap_or_else(|| "—".to_string())));
            }
            rows.push((String::new(), String::new()));
            rows.push(("Other".to_string(), String::new()));
            rows.push(kv(
                "  CloudWatch Logging",
                match (t.logging_enabled, t.log_group()) {
                    (Some(true), Some(g)) => format!("✓ {g} (t to tail)"),
                    (Some(true), None) => "✓ enabled".to_string(),
                    (Some(false), _) => "✗ disabled — no task log to tail".to_string(),
                    (None, _) => "unknown".to_string(),
                },
            ));
            if let Some(c) = &t.created {
                rows.push(kv("  Created", c.clone()));
            }
            if let Some(s) = &t.started {
                rows.push(kv("  Last Started", s.clone()));
            }
            rows.push(kv("  ARN", t.arn.clone()));
            rows
        }
        S::Tables => dms_table_stats_rows(tables),
        S::Assessments => match assessments {
            None | Some(Lazy::Loading) => vec![("".to_string(), "Loading…".to_string())],
            Some(Lazy::Error(e)) => error_rows(e),
            Some(Lazy::Loaded(runs)) if runs.is_empty() => {
                vec![("".to_string(), "No premigration assessment runs for this task".to_string())]
            }
            Some(Lazy::Loaded(runs)) => {
                let mut rows = Vec::new();
                for (i, r) in runs.iter().enumerate() {
                    if i > 0 {
                        rows.push((String::new(), String::new()));
                    }
                    let latest = if r.latest { " (latest)" } else { "" };
                    rows.push((format!("{}{}", r.name, latest), String::new()));
                    rows.push(kv("  Status", r.status.clone()));
                    if let Some(c) = &r.created {
                        rows.push(kv("  Created", c.clone()));
                    }
                    if let Some((done, total)) = r.progress {
                        rows.push(kv("  Progress", format!("{done}/{total} assessments")));
                    }
                    let outcome = format!(
                        "{} passed · {} failed · {} error · {} warning · {} skipped",
                        r.passed, r.failed, r.error, r.warning, r.skipped
                    );
                    rows.push(kv(
                        "  Results",
                        if r.failed + r.error > 0 {
                            format!("✗ {outcome}")
                        } else if r.warning > 0 {
                            format!("⚠ {outcome}")
                        } else {
                            outcome
                        },
                    ));
                    if let Some(f) = &r.last_failure {
                        rows.push(kv("  Failure", format!("✗ {f}")));
                    }
                    if let Some(loc) = &r.result_location {
                        rows.push(kv("  Report", loc.clone()));
                    }
                }
                rows
            }
        },
        S::Settings => {
            let mut rows = Vec::new();
            dms_json_rows(&mut rows, "Table Mappings", t.table_mappings.as_deref());
            rows.push((String::new(), String::new()));
            dms_json_rows(&mut rows, "Task Settings", t.settings.as_deref());
            rows
        }
        S::Tags => tag_rows(&t.tags),
    }
}

pub fn dms_instance_section_lines(
    i: &crate::aws::services::dms::DmsInstance,
    section: crate::aws::services::dms::DmsInstanceDetailSection,
    tasks: &[&crate::aws::services::dms::DmsTask],
) -> Vec<(String, String)> {
    use crate::aws::services::dms::DmsInstanceDetailSection as S;
    match section {
        S::Overview => {
            let mut rows = vec![
                kv("Instance", i.identifier.clone()),
                kv("Status", i.state_label()),
                kv("Class", i.class.clone()),
                kv("Allocated Storage", format!("{} GB", i.allocated_storage_gb)),
                kv("Engine Version", i.engine_version.clone().unwrap_or_default()),
                kv("Multi-AZ", if i.multi_az { "✓ yes" } else { "no" }),
                kv("Auto Minor Upgrade", if i.auto_minor_upgrade { "yes" } else { "no" }),
            ];
            if let Some(w) = &i.maintenance_window {
                rows.push(kv("Maintenance Window", w.clone()));
            }
            if !i.pending.is_empty() {
                rows.push((String::new(), String::new()));
                rows.push(("Pending Changes".to_string(), String::new()));
                for p in &i.pending {
                    rows.push((format!("  ⚠ {p}"), String::new()));
                }
            }
            rows.push((String::new(), String::new()));
            rows.push(("Other".to_string(), String::new()));
            if let Some(k) = &i.kms_key_id {
                rows.push(kv("  KMS Key", k.clone()));
            }
            if let Some(c) = &i.created {
                rows.push(kv("  Created", c.clone()));
            }
            rows.push(kv("  ARN", i.arn.clone()));
            rows
        }
        S::Network => {
            let mut rows = Vec::new();
            if let Some(v) = &i.vpc_id {
                rows.push(kv("VPC", v.clone()));
            }
            if let Some(g) = &i.subnet_group {
                rows.push(kv("Subnet Group", g.clone()));
            }
            if let Some(az) = &i.availability_zone {
                rows.push(kv("Availability Zone", az.clone()));
            }
            if let Some(az) = &i.secondary_az {
                rows.push(kv("Standby AZ", az.clone()));
            }
            rows.push(kv(
                "Publicly Accessible",
                if i.publicly_accessible { "⚠ yes" } else { "no" },
            ));
            if let Some(n) = &i.network_type {
                rows.push(kv("Network Type", n.clone()));
            }
            if !i.private_ips.is_empty() {
                rows.push(kv("Private IPs", i.private_ips.join(", ")));
            }
            if !i.public_ips.is_empty() {
                rows.push(kv("Public IPs", i.public_ips.join(", ")));
            }
            if !i.security_groups.is_empty() {
                rows.push((String::new(), String::new()));
                rows.push(("Security Groups".to_string(), String::new()));
                for (sg, status) in &i.security_groups {
                    rows.push((format!("  {sg}"), status.clone()));
                }
            }
            if !i.subnets.is_empty() {
                rows.push((String::new(), String::new()));
                rows.push(("Subnets".to_string(), String::new()));
                for (sn, az) in &i.subnets {
                    rows.push((format!("  {sn}"), az.clone()));
                }
            }
            rows
        }
        S::Tasks => {
            if tasks.is_empty() {
                return vec![("".to_string(), "No replication tasks on this instance".to_string())];
            }
            let mut rows = Vec::new();
            for t in tasks {
                rows.push((t.identifier.clone(), String::new()));
                rows.push(kv("  Status", t.state_label()));
                rows.push(kv("  Type", t.migration_type.clone()));
                rows.push(kv("  Task ARN", t.arn.clone()));
            }
            rows
        }
        S::Tags => tag_rows(&i.tags),
    }
}

pub fn dms_endpoint_section_lines(
    e: &crate::aws::services::dms::DmsEndpoint,
    section: crate::aws::services::dms::DmsEndpointDetailSection,
    tasks: &[&crate::aws::services::dms::DmsTask],
    serverless: &[&crate::aws::services::dms::DmsServerless],
) -> Vec<(String, String)> {
    use crate::aws::services::dms::DmsEndpointDetailSection as S;
    match section {
        S::Overview => {
            let mut rows = vec![
                kv("Endpoint", e.identifier.clone()),
                kv("Type", e.endpoint_type.clone()),
                kv("Engine", e.engine_label()),
                kv("Status", e.state_label()),
            ];
            rows.push((String::new(), String::new()));
            rows.push(("Connection".to_string(), String::new()));
            if let Some(s) = &e.server {
                let port = e.port.map(|p| format!(":{p}")).unwrap_or_default();
                rows.push(kv("  Server", format!("{s}{port}")));
            }
            if let Some(d) = &e.database {
                rows.push(kv("  Database", d.clone()));
            }
            if let Some(u) = &e.username {
                rows.push(kv("  Username", u.clone()));
            }
            if let Some(m) = &e.ssl_mode {
                rows.push(kv(
                    "  SSL Mode",
                    if m == "none" { format!("⚠ {m}") } else { m.clone() },
                ));
            }
            if let Some(c) = &e.certificate_arn {
                rows.push(kv("  Certificate", c.clone()));
            }
            if let Some(a) = &e.extra_connection_attributes {
                rows.push(kv("  Extra Attributes", a.clone()));
            }
            rows.push((String::new(), String::new()));
            rows.push(("Access".to_string(), String::new()));
            // A reference only — never resolved (see the Secrets rules).
            rows.push(kv(
                "  Credentials Secret",
                e.secret_ref.clone().unwrap_or_else(|| "· inline credentials (not shown)".to_string()),
            ));
            if let Some(r) = &e.service_access_role {
                rows.push(kv("  Service Access Role", r.clone()));
            }
            if let Some(k) = &e.kms_key_id {
                rows.push(kv("  KMS Key", k.clone()));
            }
            rows.push(kv("  ARN", e.arn.clone()));
            rows
        }
        S::Connections => {
            if e.connections.is_empty() {
                return vec![(
                    "".to_string(),
                    "No connection tests recorded — run one from the console, or `C` for the command"
                        .to_string(),
                )];
            }
            let mut rows = Vec::new();
            for c in &e.connections {
                let status = match c.status.as_str() {
                    "successful" => format!("✓ {}", c.status),
                    "failed" => format!("✗ {}", c.status),
                    _ => c.status.clone(),
                };
                rows.push((c.instance_id.clone(), String::new()));
                rows.push(kv("  Status", status));
                if let Some(f) = &c.last_failure {
                    rows.push(kv("  Last Failure", format!("✗ {f}")));
                }
                rows.push(kv("  Instance ARN", c.instance_arn.clone()));
            }
            rows
        }
        S::UsedBy => {
            if tasks.is_empty() && serverless.is_empty() {
                return vec![("".to_string(), "No task or serverless replication uses this endpoint".to_string())];
            }
            let mut rows = Vec::new();
            for t in tasks {
                let role = if t.source_endpoint_arn == e.arn { "source" } else { "target" };
                rows.push((t.identifier.clone(), String::new()));
                rows.push(kv("  Role", role));
                rows.push(kv("  Status", t.state_label()));
                rows.push(kv("  Task ARN", t.arn.clone()));
            }
            for s in serverless {
                let role = if s.source_endpoint_arn == e.arn { "source" } else { "target" };
                rows.push((format!("{} (serverless)", s.identifier), String::new()));
                rows.push(kv("  Role", role));
                rows.push(kv("  Status", s.state_label()));
                rows.push(kv("  Replication ARN", s.arn.clone()));
            }
            rows
        }
        S::Tags => tag_rows(&e.tags),
    }
}

pub fn dms_serverless_section_lines(
    s: &crate::aws::services::dms::DmsServerless,
    section: crate::aws::services::dms::DmsServerlessDetailSection,
    tables: Option<&crate::lazy::Lazy<crate::aws::services::dms::DmsTableStats>>,
) -> Vec<(String, String)> {
    use crate::aws::services::dms::DmsServerlessDetailSection as S;
    match section {
        S::Overview => {
            let mut rows = vec![
                kv("Replication", s.identifier.clone()),
                kv("Status", s.state_label()),
                kv("Replication Type", s.replication_type.clone()),
            ];
            if let Some(r) = &s.stop_reason {
                rows.push(kv("Stop Reason", r.clone()));
            }
            for f in &s.failures {
                rows.push(kv("Failure", format!("✗ {f}")));
            }
            if let Some(stats) = &s.stats {
                dms_stats_rows(&mut rows, stats);
            }
            rows.push((String::new(), String::new()));
            dms_endpoint_rows(&mut rows, "Source", &s.source_endpoint, &s.source_endpoint_arn);
            dms_endpoint_rows(&mut rows, "Target", &s.target_endpoint, &s.target_endpoint_arn);
            rows.push((String::new(), String::new()));
            rows.push(("Other".to_string(), String::new()));
            for (k, v) in [
                ("  CDC Start Position", &s.cdc_start_position),
                ("  Recovery Checkpoint", &s.recovery_checkpoint),
                ("  Last Stopped", &s.last_stop),
                ("  Created", &s.created),
            ] {
                if let Some(v) = v {
                    rows.push(kv(k, v.clone()));
                }
            }
            rows.push(kv("  ARN", s.arn.clone()));
            rows
        }
        S::Capacity => {
            let mut rows = vec![kv(
                "Capacity Range",
                match (s.min_dcu, s.max_dcu) {
                    (Some(min), Some(max)) => format!("{min}–{max} DCU"),
                    (None, Some(max)) => format!("up to {max} DCU"),
                    _ => "—".to_string(),
                },
            )];
            if let Some(p) = s.provisioned_dcu {
                rows.push(kv("Provisioned Now", format!("{p} DCU")));
            }
            if let Some(p) = &s.provision_state {
                rows.push(kv("Provision State", p.clone()));
            }
            if let Some(m) = s.multi_az {
                rows.push(kv("Multi-AZ", if m { "✓ yes" } else { "no" }));
            }
            if let Some(g) = &s.subnet_group {
                rows.push(kv("Subnet Group", g.clone()));
            }
            if !s.security_group_ids.is_empty() {
                rows.push((String::new(), String::new()));
                rows.push(("Security Groups".to_string(), String::new()));
                for sg in &s.security_group_ids {
                    rows.push((format!("  {sg}"), String::new()));
                }
            }
            if let Some(k) = &s.kms_key_id {
                rows.push((String::new(), String::new()));
                rows.push(kv("KMS Key", k.clone()));
            }
            rows
        }
        S::Tables => {
            if s.status.is_none() {
                return vec![("".to_string(), "Not started yet — no table statistics".to_string())];
            }
            dms_table_stats_rows(tables)
        }
        S::Settings => {
            let mut rows = Vec::new();
            dms_json_rows(&mut rows, "Table Mappings", s.table_mappings.as_deref());
            rows.push((String::new(), String::new()));
            dms_json_rows(&mut rows, "Replication Settings", s.settings.as_deref());
            rows
        }
        S::Tags => tag_rows(&s.tags),
    }
}
