use super::*;

// ── Glue split panes ─────────────────────────────────────────────────────────
// All four reuse `render_athena_chrome` (a generic header/tab/body chrome).

pub(super) fn render_glue_table_split(app: &App, t: &GlueTable, area: Rect, frame: &mut Frame) {
    render_athena_chrome(
        app,
        "Glue Table",
        &t.name,
        &t.database,
        t.table_type.as_deref().unwrap_or(""),
        theme::text_dim(),
        &descriptor_tabs(app, &crate::aws::services::glue::GLUE_TABLE_SECTIONS),
        area,
        frame,
    );
}

pub fn glue_table_section_lines(
    t: &GlueTable,
    section: GlueTableDetailSection,
) -> Vec<(String, String)> {
    match section {
        GlueTableDetailSection::Overview => {
            let mut rows = vec![
                ("Name".to_string(), t.name.clone()),
                ("Database".to_string(), t.database.clone()),
            ];
            if let Some(tt) = &t.table_type {
                rows.push(("Type".to_string(), tt.clone()));
            }
            if let Some(o) = &t.owner {
                rows.push(("Owner".to_string(), o.clone()));
            }
            if let Some(d) = &t.description {
                rows.push(("Description".to_string(), d.clone()));
            }
            rows.push((String::new(), String::new()));
            rows.push(("Schema".to_string(), String::new())); // group header
            rows.push(("  Columns".to_string(), t.columns.len().to_string()));
            rows.push((
                "  Partition Keys".to_string(),
                t.partition_keys.len().to_string(),
            ));
            if t.created.is_some() || t.updated.is_some() {
                rows.push((String::new(), String::new()));
                if let Some(c) = &t.created {
                    rows.push(("Created".to_string(), c.clone()));
                }
                if let Some(u) = &t.updated {
                    rows.push(("Updated".to_string(), u.clone()));
                }
            }
            rows
        }
        GlueTableDetailSection::Schema => {
            let mut rows = Vec::new();
            rows.push(("Columns".to_string(), String::new())); // group header
            if t.columns.is_empty() {
                rows.push((String::new(), "No columns".to_string()));
            }
            for (name, ty, comment) in &t.columns {
                let val = match comment {
                    Some(c) if !c.is_empty() => format!("{}  — {}", ty, c),
                    _ => ty.clone(),
                };
                rows.push((format!("  {}", name), val));
            }
            if !t.partition_keys.is_empty() {
                rows.push((String::new(), String::new()));
                rows.push(("Partition Keys".to_string(), String::new())); // group header
                for (name, ty) in &t.partition_keys {
                    rows.push((format!("  {}", name), ty.clone()));
                }
            }
            rows
        }
        GlueTableDetailSection::Storage => {
            let mut rows = Vec::new();
            if let Some(loc) = &t.location {
                rows.push(("Location".to_string(), loc.clone())); // s3:// jumpable
            }
            rows.push((
                "Compressed".to_string(),
                if t.compressed { "✓ Yes".to_string() } else { "✗ No".to_string() },
            ));
            if let Some(f) = &t.input_format {
                rows.push(("Input Format".to_string(), f.clone()));
            }
            if let Some(f) = &t.output_format {
                rows.push(("Output Format".to_string(), f.clone()));
            }
            if let Some(s) = &t.serde_library {
                rows.push(("SerDe Library".to_string(), s.clone()));
            }
            if !t.storage_parameters.is_empty() {
                rows.push((String::new(), String::new()));
                rows.push(("SerDe Parameters".to_string(), String::new())); // group header
                for (k, v) in &t.storage_parameters {
                    rows.push((format!("  {}", k), v.clone()));
                }
            }
            rows
        }
    }
}

pub(super) fn render_glue_crawler_split(app: &App, c: &GlueCrawler, area: Rect, frame: &mut Frame) {
    let (status, color) = match c.state.as_str() {
        "READY" => ("ready", theme::success()),
        "RUNNING" => ("running", theme::warning()),
        "STOPPING" => ("stopping", theme::warning()),
        other => (other, theme::text_dim()),
    };
    render_athena_chrome(
        app,
        "Glue Crawler",
        &c.name,
        c.database_name.as_deref().unwrap_or(""),
        status,
        color,
        &descriptor_tabs(app, &crate::aws::services::glue::GLUE_CRAWLER_SECTIONS),
        area,
        frame,
    );
}

pub fn glue_crawler_section_lines(
    c: &GlueCrawler,
    section: GlueCrawlerDetailSection,
) -> Vec<(String, String)> {
    match section {
        GlueCrawlerDetailSection::Overview => {
            let mut rows = vec![
                ("Name".to_string(), c.name.clone()),
                ("State".to_string(), c.state.clone()),
            ];
            if let Some(db) = &c.database_name {
                rows.push(("Database".to_string(), db.clone()));
            }
            if let Some(r) = &c.role {
                rows.push(("Role".to_string(), r.clone())); // jumpable when IAM ARN
            }
            if let Some(p) = &c.table_prefix {
                rows.push(("Table Prefix".to_string(), p.clone()));
            }
            if let Some(d) = &c.description {
                rows.push(("Description".to_string(), d.clone()));
            }

            rows.push((String::new(), String::new()));
            rows.push(("Schedule".to_string(), String::new())); // group header
            rows.push((
                "  Expression".to_string(),
                c.schedule_expression.clone().unwrap_or_else(|| "On demand".to_string()),
            ));
            if let Some(s) = &c.schedule_state {
                rows.push(("  State".to_string(), s.clone()));
            }

            rows.push((String::new(), String::new()));
            rows.push(("Last Crawl".to_string(), String::new())); // group header
            rows.push((
                "  Status".to_string(),
                match c.last_crawl_status.as_deref() {
                    Some(s) if s.contains("SUCCEEDED") => format!("✓ {}", s),
                    Some(s) if s.contains("FAILED") || s.contains("CANCELLED") => format!("✗ {}", s),
                    Some(s) => s.to_string(),
                    None => "—".to_string(),
                },
            ));
            if let Some(t) = &c.last_crawl_start {
                rows.push(("  Started".to_string(), t.clone()));
            }
            if let Some(e) = &c.last_crawl_error {
                rows.push(("  Error".to_string(), format!("⚠ {}", e)));
            }

            if c.created.is_some() || c.last_updated.is_some() {
                rows.push((String::new(), String::new()));
                if let Some(t) = &c.created {
                    rows.push(("Created".to_string(), t.clone()));
                }
                if let Some(t) = &c.last_updated {
                    rows.push(("Last Updated".to_string(), t.clone()));
                }
            }
            rows
        }
        GlueCrawlerDetailSection::Targets => {
            if c.target_rows.is_empty() {
                return vec![(String::new(), "No targets".to_string())];
            }
            // Pre-flattened (label, value) rows; S3 paths render as s3://… so the
            // generic classifier makes them jumpable.
            c.target_rows.clone()
        }
        GlueCrawlerDetailSection::Configuration => {
            let mut rows = Vec::new();
            rows.push((
                "Classifiers".to_string(),
                if c.classifiers.is_empty() {
                    "Default (built-in)".to_string()
                } else {
                    c.classifiers.join(", ")
                },
            ));
            if let Some(b) = &c.recrawl_behavior {
                rows.push(("Recrawl Behavior".to_string(), b.clone()));
            }
            rows.push((String::new(), String::new()));
            rows.push(("Schema Change Policy".to_string(), String::new())); // group header
            rows.push((
                "  Update Behavior".to_string(),
                c.update_behavior.clone().unwrap_or_else(|| "—".to_string()),
            ));
            rows.push((
                "  Delete Behavior".to_string(),
                c.delete_behavior.clone().unwrap_or_else(|| "—".to_string()),
            ));
            if let Some(s) = &c.security_configuration {
                rows.push((String::new(), String::new()));
                rows.push(("Security Configuration".to_string(), s.clone()));
            }
            rows
        }
    }
}

pub(super) fn render_glue_job_split(app: &App, j: &GlueJob, area: Rect, frame: &mut Frame) {
    render_athena_chrome(
        app,
        "Glue Job",
        &j.name,
        j.command_name.as_deref().unwrap_or(""),
        j.glue_version.as_deref().unwrap_or(""),
        theme::text_dim(),
        &descriptor_tabs(app, &crate::aws::services::glue::GLUE_JOB_SECTIONS),
        area,
        frame,
    );
}

pub fn glue_job_section_lines(
    j: &GlueJob,
    section: GlueJobDetailSection,
) -> Vec<(String, String)> {
    match section {
        GlueJobDetailSection::Overview => {
            let mut rows = vec![("Name".to_string(), j.name.clone())];
            if let Some(d) = &j.description {
                rows.push(("Description".to_string(), d.clone()));
            }
            if let Some(r) = &j.role {
                rows.push(("Role".to_string(), r.clone())); // jumpable when IAM ARN
            }
            if let Some(v) = &j.glue_version {
                rows.push(("Glue Version".to_string(), v.clone()));
            }
            if let Some(e) = &j.execution_class {
                rows.push(("Execution Class".to_string(), e.clone()));
            }

            rows.push((String::new(), String::new()));
            rows.push(("Capacity".to_string(), String::new())); // group header
            if let Some(w) = &j.worker_type {
                rows.push(("  Worker Type".to_string(), w.clone()));
            }
            if let Some(n) = j.number_of_workers {
                rows.push(("  Number of Workers".to_string(), n.to_string()));
            }
            if let Some(m) = j.max_capacity {
                rows.push(("  Max Capacity".to_string(), format!("{} DPU", m)));
            }

            rows.push((String::new(), String::new()));
            rows.push(("Execution".to_string(), String::new())); // group header
            rows.push((
                "  Timeout".to_string(),
                j.timeout_min
                    .map(|t| format!("{} min", t))
                    .unwrap_or_else(|| "—".to_string()),
            ));
            rows.push(("  Max Retries".to_string(), j.max_retries.to_string()));
            if let Some(m) = j.max_concurrent_runs {
                rows.push(("  Max Concurrent Runs".to_string(), m.to_string()));
            }
            if !j.connections.is_empty() {
                rows.push(("  Connections".to_string(), j.connections.join(", ")));
            }

            if j.created.is_some() || j.last_modified.is_some() {
                rows.push((String::new(), String::new()));
                if let Some(t) = &j.created {
                    rows.push(("Created".to_string(), t.clone()));
                }
                if let Some(t) = &j.last_modified {
                    rows.push(("Last Modified".to_string(), t.clone()));
                }
            }
            rows
        }
        GlueJobDetailSection::Command => {
            let mut rows = Vec::new();
            rows.push((
                "Job Type".to_string(),
                j.command_name.clone().unwrap_or_else(|| "—".to_string()),
            ));
            if let Some(s) = &j.script_location {
                rows.push(("Script Location".to_string(), s.clone())); // s3:// jumpable
            }
            if let Some(p) = &j.python_version {
                rows.push(("Python Version".to_string(), p.clone()));
            }
            if let Some(r) = &j.runtime {
                rows.push(("Runtime".to_string(), r.clone()));
            }
            rows
        }
        GlueJobDetailSection::Arguments => {
            if j.default_arguments.is_empty() {
                return vec![(String::new(), "No default arguments".to_string())];
            }
            let mut rows = vec![("Default Arguments".to_string(), String::new())]; // group header
            for (k, v) in &j.default_arguments {
                rows.push((format!("  {}", k), v.clone()));
            }
            rows
        }
    }
}

pub(super) fn render_glue_job_run_split(app: &App, r: &GlueJobRun, area: Rect, frame: &mut Frame) {
    let color = match r.state.as_str() {
        "SUCCEEDED" => theme::success(),
        "STARTING" | "RUNNING" | "STOPPING" | "WAITING" => theme::warning(),
        "FAILED" | "ERROR" | "TIMEOUT" => theme::error(),
        _ => theme::text_dim(),
    };
    let subtitle = r
        .dpu_hours()
        .map(|h| format!("{:.4} DPU-hours", h))
        .unwrap_or_default();
    render_athena_chrome(
        app,
        "Glue Job Run",
        &r.job_name,
        &subtitle,
        &r.state,
        color,
        &descriptor_tabs(app, &crate::aws::services::glue::GLUE_JOB_RUN_SECTIONS),
        area,
        frame,
    );
}

pub fn glue_job_run_section_lines(
    r: &GlueJobRun,
    section: GlueJobRunDetailSection,
) -> Vec<(String, String)> {
    match section {
        GlueJobRunDetailSection::Overview => {
            let mut rows = vec![
                ("Job".to_string(), r.job_name.clone()),
                ("State".to_string(), r.state.clone()),
                ("Run ID".to_string(), r.id.clone()),
                ("Attempt".to_string(), r.attempt.to_string()),
            ];
            if let Some(t) = &r.trigger_name {
                rows.push(("Trigger".to_string(), t.clone()));
            }

            rows.push((String::new(), String::new()));
            rows.push(("Timing".to_string(), String::new())); // group header
            if let Some(t) = &r.started {
                rows.push(("  Started".to_string(), t.clone()));
            }
            if let Some(t) = &r.completed {
                rows.push(("  Completed".to_string(), t.clone()));
            }
            rows.push((
                "  Duration".to_string(),
                fmt_secs(r.execution_time_secs as i64),
            ));

            rows.push((String::new(), String::new()));
            rows.push(("Capacity".to_string(), String::new())); // group header
            if let Some(h) = r.dpu_hours() {
                rows.push(("  DPU-hours".to_string(), format!("{:.4}", h)));
            }
            if let Some(w) = &r.worker_type {
                rows.push(("  Worker Type".to_string(), w.clone()));
            }
            if let Some(n) = r.number_of_workers {
                rows.push(("  Number of Workers".to_string(), n.to_string()));
            }
            if let Some(m) = r.max_capacity {
                rows.push(("  Max Capacity".to_string(), format!("{} DPU", m)));
            }
            if let Some(v) = &r.glue_version {
                rows.push(("  Glue Version".to_string(), v.clone()));
            }
            if let Some(e) = &r.execution_class {
                rows.push(("  Execution Class".to_string(), e.clone()));
            }

            if let Some(e) = &r.error_message {
                rows.push((String::new(), String::new()));
                rows.push(("Error".to_string(), String::new())); // group header
                rows.push((format!(" {}", e), String::new()));
            }
            rows
        }
        GlueJobRunDetailSection::Arguments => {
            if r.arguments.is_empty() {
                return vec![(String::new(), "No run arguments".to_string())];
            }
            let mut rows = vec![("Run Arguments".to_string(), String::new())]; // group header
            for (k, v) in &r.arguments {
                rows.push((format!("  {}", k), v.clone()));
            }
            rows
        }
    }
}
