use super::*;
use crate::aws::services::datasync::{
    DataSyncAgent, DataSyncAgentDetailSection, DataSyncExecution, DataSyncExecutions, DataSyncLocation,
    DataSyncLocationDetailSection, DataSyncTask, DataSyncTaskDetailSection, LocationConfig, LocationKind,
    DATASYNC_AGENT_SECTIONS, DATASYNC_LOCATION_SECTIONS, DATASYNC_TASK_SECTIONS, MAX_EXECUTIONS,
};

// ── DataSync split panes ────────────────────────────────────────────────────

pub(super) fn render_datasync_task_split(app: &App, t: &DataSyncTask, area: Rect, frame: &mut Frame) {
    let mut subtitle = t.state_label();
    if !t.mode.is_empty() {
        subtitle = format!("{subtitle} · {}", t.mode.to_lowercase());
    }
    if let Some(s) = &t.schedule {
        subtitle = format!("{subtitle} · {s}");
    }
    render_simple_split(
        app,
        area,
        frame,
        "DataSync Task",
        t.name(),
        &subtitle,
        &descriptor_tabs(app, &DATASYNC_TASK_SECTIONS),
    );
}

pub(super) fn render_datasync_location_split(app: &App, l: &DataSyncLocation, area: Rect, frame: &mut Frame) {
    let subtitle = format!("{} · {}", l.kind.label(), l.location_id());
    render_simple_split(
        app,
        area,
        frame,
        "DataSync Location",
        l.name(),
        &subtitle,
        &descriptor_tabs(app, &DATASYNC_LOCATION_SECTIONS),
    );
}

pub(super) fn render_datasync_agent_split(app: &App, a: &DataSyncAgent, area: Rect, frame: &mut Frame) {
    let mut subtitle = a.state_label();
    if let Some(e) = &a.endpoint_type {
        subtitle = format!("{subtitle} · {}", e.to_lowercase());
    }
    render_simple_split(
        app,
        area,
        frame,
        "DataSync Agent",
        a.name(),
        &subtitle,
        &descriptor_tabs(app, &DATASYNC_AGENT_SECTIONS),
    );
}

/// Most rows a growing Overview block shows before pointing at its tab.
const DATASYNC_PREVIEW_CAP: usize = 8;

fn datasync_bytes(b: i64) -> String {
    crate::aws::services::backup::fmt_bytes(b)
}

/// Source / destination block: the URI (an `s3://` one jumps to its bucket),
/// then the location ARN row, which `⏎` follows to the Locations tab.
fn datasync_location_rows(rows: &mut Vec<(String, String)>, heading: &str, uri: &Option<String>, arn: &str) {
    if arn.is_empty() {
        return;
    }
    rows.push((heading.to_string(), String::new()));
    rows.push(kv(
        "  URI",
        uri.clone().unwrap_or_else(|| "· not in the locations list".to_string()),
    ));
    rows.push(kv("  Location ARN", arn));
}

/// One execution: a group header (id + status), then what it moved. The
/// Overview's "last run" block and the Executions tab share it.
fn datasync_execution_rows(rows: &mut Vec<(String, String)>, e: &DataSyncExecution) {
    rows.push((e.execution_id().to_string(), String::new()));
    let status = e.status.to_lowercase();
    rows.push(kv(
        "  Status",
        if e.is_error() {
            format!("✗ {status}")
        } else if e.is_active() {
            status
        } else {
            format!("✓ {status}")
        },
    ));
    if let Some(c) = &e.error_code {
        rows.push(kv("  Error", format!("✗ {c}")));
    }
    if let Some(d) = &e.error_detail {
        rows.push(kv("  Error Detail", format!("✗ {d}")));
    }
    if let Some(s) = &e.started {
        rows.push(kv("  Started", s.clone()));
    }
    if let Some(s) = &e.ended {
        rows.push(kv("  Ended", s.clone()));
    }
    if let Some(ms) = e.total_duration_ms {
        rows.push(kv("  Duration", dms_elapsed(ms)));
    }
    let of = |done: String, est: i64, est_text: String| {
        if est > 0 {
            format!("{done} of {est_text} estimated")
        } else {
            done
        }
    };
    rows.push(kv(
        "  Files",
        of(e.files_transferred.to_string(), e.files_estimated, e.files_estimated.to_string()),
    ));
    rows.push(kv(
        "  Data",
        of(datasync_bytes(e.bytes_transferred), e.bytes_estimated, datasync_bytes(e.bytes_estimated)),
    ));
    if e.bytes_written > 0 && e.bytes_written != e.bytes_transferred {
        rows.push(kv("  Written", datasync_bytes(e.bytes_written)));
    }
    let mut other = Vec::new();
    for (n, what) in [
        (e.files_verified, "verified"),
        (e.files_skipped, "skipped"),
        (e.files_deleted, "deleted"),
    ] {
        if n > 0 {
            other.push(format!("{n} {what}"));
        }
    }
    if !other.is_empty() {
        rows.push(kv("  Also", other.join(" · ")));
    }
    if e.files_failed > 0 {
        rows.push(kv("  Failed", format!("✗ {} files", e.files_failed)));
    }
    if !e.phases.is_empty() {
        let phases: Vec<String> = e
            .phases
            .iter()
            .map(|(name, status, dur)| {
                let mark = match status.as_str() {
                    "SUCCESS" => "✓",
                    "ERROR" => "✗",
                    _ => "…",
                };
                match dur {
                    Some(ms) => format!("{mark} {name} {}", dms_elapsed(*ms)),
                    None => format!("{mark} {name}"),
                }
            })
            .collect();
        let line = phases.join(" · ");
        rows.push(kv(
            "  Phases",
            // A leading ✗ colours the row; keep it only when a phase failed.
            if e.phases.iter().any(|p| p.1 == "ERROR") {
                format!("✗ {}", line.replace("✗ ", ""))
            } else {
                line
            },
        ));
    }
    match (&e.report_status, &e.report_error) {
        (_, Some(err)) => rows.push(kv("  Report", format!("✗ {err}"))),
        (Some(s), None) => rows.push(kv("  Report", s.to_lowercase())),
        _ => {}
    }
    rows.push(kv("  Execution ARN", e.arn.clone()));
}

pub fn datasync_task_section_lines(
    t: &DataSyncTask,
    section: DataSyncTaskDetailSection,
    executions: Option<&crate::lazy::Lazy<DataSyncExecutions>>,
) -> Vec<(String, String)> {
    use crate::lazy::Lazy;
    use DataSyncTaskDetailSection as S;
    match section {
        S::Overview => {
            let mut rows = vec![
                kv("Task", t.name().to_string()),
                kv("Task ID", t.task_id().to_string()),
                kv("Status", t.state_label()),
            ];
            if !t.mode.is_empty() {
                rows.push(kv("Mode", t.mode.to_lowercase()));
            }
            if !t.described {
                rows.push(kv("", "· details unavailable (DescribeTask failed) — r to retry"));
            }
            if let Some(c) = &t.error_code {
                rows.push(kv("Error", format!("✗ {c}")));
            }
            if let Some(d) = &t.error_detail {
                rows.push(kv("Error Detail", format!("✗ {d}")));
            }
            if let Some(exec) = &t.current_execution_arn {
                rows.push(kv("Running Now", exec.clone()));
            }

            rows.push((String::new(), String::new()));
            rows.push(("Schedule".to_string(), String::new()));
            match &t.schedule {
                None => rows.push(kv("  Expression", "· none — runs on demand")),
                Some(expr) => {
                    rows.push(kv("  Expression", expr.clone()));
                    let status = t.schedule_status.as_deref().unwrap_or("").to_lowercase();
                    rows.push(kv(
                        "  Status",
                        match (status.as_str(), t.schedule_disabled_by_service()) {
                            ("disabled", true) => "✗ disabled by DataSync".to_string(),
                            ("disabled", false) => "⚠ disabled".to_string(),
                            ("", _) => "—".to_string(),
                            (s, _) => format!("✓ {s}"),
                        },
                    ));
                    if let Some(r) = &t.schedule_disabled_reason {
                        rows.push(kv("  Disabled Reason", r.clone()));
                    }
                }
            }

            rows.push((String::new(), String::new()));
            datasync_location_rows(&mut rows, "Source", &t.source_uri, &t.source_arn);
            datasync_location_rows(&mut rows, "Destination", &t.destination_uri, &t.destination_arn);

            rows.push((String::new(), String::new()));
            rows.push(("Last Execution".to_string(), String::new()));
            match executions {
                None | Some(Lazy::Loading) => rows.push(("".to_string(), "Loading…".to_string())),
                Some(Lazy::Error(e)) => rows.extend(error_rows(e)),
                Some(Lazy::Loaded(x)) => match x.runs.first() {
                    None => rows.push(kv("", "· never run")),
                    Some(last) => {
                        datasync_execution_rows(&mut rows, last);
                        if x.total > 1 {
                            rows.push(kv("", format!("· {} more — 2 Executions", x.total - 1)));
                        }
                    }
                },
            }

            rows.push((String::new(), String::new()));
            rows.push(("Other".to_string(), String::new()));
            rows.push(kv(
                "  CloudWatch Logs",
                match (&t.log_group_arn, t.log_group()) {
                    (Some(_), Some(g)) => format!("✓ {g} (t to tail)"),
                    (Some(arn), None) => arn.clone(),
                    (None, _) => "✗ no log group — nothing to tail".to_string(),
                },
            ));
            if let Some(c) = &t.created {
                rows.push(kv("  Created", c.clone()));
            }
            rows.push(kv("  ARN", t.arn.clone()));
            rows
        }
        S::Executions => match executions {
            None | Some(Lazy::Loading) => vec![("".to_string(), "Loading…".to_string())],
            Some(Lazy::Error(e)) => error_rows(e),
            Some(Lazy::Loaded(x)) if x.runs.is_empty() => {
                vec![("".to_string(), "This task has never run".to_string())]
            }
            Some(Lazy::Loaded(x)) => {
                let failed = x.runs.iter().filter(|r| r.is_error()).count();
                let mut rows = vec![kv("Executions", x.total.to_string())];
                rows.push(kv(
                    "Failed",
                    if failed > 0 {
                        format!("✗ {failed} of the {} shown", x.runs.len())
                    } else {
                        "✓ 0".to_string()
                    },
                ));
                if x.total > x.runs.len() {
                    rows.push(kv("", format!("· most recent {MAX_EXECUTIONS} shown")));
                }
                for r in &x.runs {
                    rows.push((String::new(), String::new()));
                    datasync_execution_rows(&mut rows, r);
                }
                rows
            }
        },
        S::Options => {
            let mut rows = vec![("Transfer".to_string(), String::new())];
            if t.options.is_empty() {
                rows.push(kv("", "· none returned"));
            }
            for (k, v) in &t.options {
                rows.push(kv(&format!("  {k}"), v.clone()));
            }
            for (heading, patterns) in [("Includes", &t.includes), ("Excludes", &t.excludes)] {
                rows.push((String::new(), String::new()));
                rows.push((heading.to_string(), String::new()));
                if patterns.is_empty() {
                    rows.push(kv("", "· none"));
                }
                for p in patterns {
                    rows.push((format!("  {p}"), String::new()));
                }
            }
            if let Some(m) = &t.manifest {
                rows.push((String::new(), String::new()));
                rows.push(kv("Manifest", m.clone()));
            }
            if let Some((level, output, bucket, prefix)) = &t.report {
                rows.push((String::new(), String::new()));
                rows.push(("Task Report".to_string(), String::new()));
                rows.push(kv("  Level", level.clone()));
                rows.push(kv("  Output", output.clone()));
                if !bucket.is_empty() {
                    rows.push(kv("  Bucket", bucket.clone()));
                }
                if !prefix.is_empty() {
                    rows.push(kv("  Prefix", prefix.clone()));
                }
            }
            if !t.source_enis.is_empty() || !t.destination_enis.is_empty() {
                rows.push((String::new(), String::new()));
                rows.push(("Network Interfaces".to_string(), String::new()));
                for e in &t.source_enis {
                    rows.push(kv("  Source", crate::aws::services::datasync::arn_tail(e).to_string()));
                }
                for e in &t.destination_enis {
                    rows.push(kv("  Destination", crate::aws::services::datasync::arn_tail(e).to_string()));
                }
            }
            rows
        }
        S::Tags => tag_rows(&t.tags),
    }
}

/// Tasks using a location, as `name (source|destination)` + the ARN row
/// `⏎` follows.
fn datasync_task_rows(rows: &mut Vec<(String, String)>, loc_arn: &str, tasks: &[&DataSyncTask], cap: Option<usize>) {
    for t in tasks.iter().take(cap.unwrap_or(usize::MAX)) {
        let role = match (t.source_arn == loc_arn, t.destination_arn == loc_arn) {
            (true, true) => "source + destination",
            (true, false) => "source",
            _ => "destination",
        };
        rows.push((format!("{} ({role})", t.name()), String::new()));
        rows.push(kv("  Status", t.state_label()));
        rows.push(kv("  Task ARN", t.arn.clone()));
    }
}

pub fn datasync_location_section_lines(
    l: &DataSyncLocation,
    section: DataSyncLocationDetailSection,
    config: Option<&crate::lazy::Lazy<LocationConfig>>,
    tasks: &[&DataSyncTask],
) -> Vec<(String, String)> {
    use crate::lazy::Lazy;
    use DataSyncLocationDetailSection as S;
    match section {
        S::Overview => {
            let mut rows = vec![
                kv("URI", l.uri.clone()),
                kv("Location ID", l.location_id().to_string()),
                kv("Type", l.kind.label()),
                kv("ARN", l.arn.clone()),
            ];
            rows.push((String::new(), String::new()));
            rows.push(kv(
                "Used By",
                if tasks.is_empty() {
                    "⚠ no task uses this location".to_string()
                } else {
                    format!("{} task{}", tasks.len(), if tasks.len() == 1 { "" } else { "s" })
                },
            ));
            if !tasks.is_empty() {
                rows.push((String::new(), String::new()));
                datasync_task_rows(&mut rows, &l.arn, tasks, Some(DATASYNC_PREVIEW_CAP));
                if tasks.len() > DATASYNC_PREVIEW_CAP {
                    rows.push(kv(
                        "",
                        format!("· {} more — 3 Tasks", tasks.len() - DATASYNC_PREVIEW_CAP),
                    ));
                }
            }
            rows
        }
        S::Configuration => {
            if l.kind == LocationKind::Other {
                return vec![(
                    "".to_string(),
                    "· this location type has no describe call in this build".to_string(),
                )];
            }
            match config {
                None | Some(Lazy::Loading) => vec![("".to_string(), "Loading…".to_string())],
                Some(Lazy::Error(e)) => error_rows(e),
                Some(Lazy::Loaded(c)) => {
                    let mut rows = vec![kv("Type", l.kind.label())];
                    rows.extend(c.iter().cloned());
                    rows
                }
            }
        }
        S::Tasks => {
            if tasks.is_empty() {
                return vec![("".to_string(), "No task uses this location".to_string())];
            }
            let mut rows = Vec::new();
            datasync_task_rows(&mut rows, &l.arn, tasks, None);
            rows
        }
        S::Tags => tag_rows(&l.tags),
    }
}

pub fn datasync_agent_section_lines(a: &DataSyncAgent, section: DataSyncAgentDetailSection) -> Vec<(String, String)> {
    use DataSyncAgentDetailSection as S;
    match section {
        S::Overview => {
            let mut rows = vec![
                kv("Agent", a.name().to_string()),
                kv("Agent ID", a.agent_id().to_string()),
                kv(
                    "Status",
                    match a.status.as_str() {
                        "ONLINE" => "✓ online".to_string(),
                        "OFFLINE" => "✗ offline".to_string(),
                        _ => a.state_label(),
                    },
                ),
                kv(
                    "Last Connection",
                    a.last_connection.clone().unwrap_or_else(|| "—".to_string()),
                ),
            ];
            if let Some(v) = &a.platform_version {
                rows.push(kv("Platform Version", v.clone()));
            }
            if let Some(c) = &a.created {
                rows.push(kv("Created", c.clone()));
            }
            rows.push(kv("ARN", a.arn.clone()));

            rows.push((String::new(), String::new()));
            rows.push(("Endpoint".to_string(), String::new()));
            rows.push(kv(
                "  Type",
                a.endpoint_type
                    .as_deref()
                    .map(str::to_lowercase)
                    .unwrap_or_else(|| "—".to_string()),
            ));
            if let Some(v) = &a.vpc_endpoint_id {
                rows.push(kv("  VPC Endpoint", v.clone()));
            }
            if let Some(v) = &a.private_link_endpoint {
                rows.push(kv("  Address", v.clone()));
            }
            for s in &a.subnets {
                rows.push(kv("  Subnet", s.clone()));
            }
            for s in &a.security_groups {
                rows.push(kv("  Security Group", s.clone()));
            }
            rows
        }
        S::Tags => tag_rows(&a.tags),
    }
}
