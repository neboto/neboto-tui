use super::*;

// ── Athena split panes ──────────────────────────────────────────────────────

/// Shared chrome for the four Athena split panes: header (name + subtitle +
/// optional colored status) | rule | section tab bar | rule | scrollable body.
#[allow(clippy::too_many_arguments)]
pub(super) fn render_athena_chrome(
    app: &App,
    title: &str,
    name: &str,
    subtitle: &str,
    status: &str,
    status_color: Color,
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

    let mut subtitle_spans = vec![Span::raw("  ")];
    if !subtitle.is_empty() {
        subtitle_spans.push(Span::styled(
            subtitle.to_string(),
            Style::default().fg(theme::text_dim()),
        ));
    }
    if !status.is_empty() {
        if !subtitle.is_empty() {
            subtitle_spans.push(Span::styled("  ·  ", Style::default().fg(theme::text_dim())));
        }
        subtitle_spans.push(Span::styled(
            status.to_string(),
            Style::default().fg(status_color),
        ));
    }
    let header = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                name.to_string(),
                Style::default().fg(crate::ui::theme::text_primary()).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(subtitle_spans),
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

/// Push the SQL query as plain (leading-space) content lines so `style_detail_row`
/// renders it verbatim rather than as group headers. `e`/copy get the full text.
pub(super) fn push_sql_lines(rows: &mut Vec<(String, String)>, sql: &str) {
    if sql.trim().is_empty() {
        rows.push((String::new(), "No query text".to_string()));
        return;
    }
    for line in sql.lines() {
        let clean = line.replace('\t', "    ");
        rows.push((format!(" {}", clean), String::new()));
    }
}

pub(super) fn render_athena_workgroup_split(app: &App, w: &AthenaWorkgroup, area: Rect, frame: &mut Frame) {
    let (status, color) = match w.state.as_str() {
        "ENABLED" => ("enabled", theme::success()),
        "DISABLED" => ("disabled", theme::text_dim()),
        _ => (w.state.as_str(), theme::text_dim()),
    };
    render_athena_chrome(
        app,
        "Athena Workgroup",
        &w.name,
        w.effective_engine.as_deref().unwrap_or("Athena"),
        status,
        color,
        &descriptor_tabs(app, &crate::aws::services::athena::ATHENA_WORKGROUP_SECTIONS),
        area,
        frame,
    );
}

pub fn athena_workgroup_section_lines(
    w: &AthenaWorkgroup,
    section: AthenaWorkgroupDetailSection,
    tags: Option<&crate::lazy::Lazy<Vec<(String, String)>>>,
) -> Vec<(String, String)> {
    match section {
        AthenaWorkgroupDetailSection::Overview => {
            let mut rows = vec![
                ("Name".to_string(), w.name.clone()),
                ("State".to_string(), w.state.clone()),
            ];
            if let Some(d) = &w.description {
                rows.push(("Description".to_string(), d.clone()));
            }
            rows.push((String::new(), String::new()));
            rows.push(("Engine".to_string(), String::new())); // group header
            rows.push((
                "  Effective Version".to_string(),
                w.effective_engine.clone().unwrap_or_else(|| "—".to_string()),
            ));
            if let Some(sel) = &w.selected_engine {
                rows.push(("  Selected Version".to_string(), sel.clone()));
            }
            if let Some(c) = &w.created {
                rows.push((String::new(), String::new()));
                rows.push(("Created".to_string(), c.clone()));
            }
            rows
        }
        AthenaWorkgroupDetailSection::Configuration => {
            let mut rows = Vec::new();
            rows.push(("Query Results".to_string(), String::new())); // group header
            rows.push((
                "  Output Location".to_string(),
                w.output_location.clone().unwrap_or_else(|| "—".to_string()),
            ));
            if let Some(o) = &w.expected_bucket_owner {
                rows.push(("  Expected Bucket Owner".to_string(), o.clone()));
            }
            rows.push((
                "  Encryption".to_string(),
                match &w.encryption_option {
                    Some(e) => format!("✓ {}", e),
                    None => "✗ Not enabled".to_string(),
                },
            ));
            if let Some(k) = &w.kms_key {
                rows.push(("  KMS Key".to_string(), k.clone())); // jumpable when ARN
            }

            rows.push((String::new(), String::new()));
            rows.push(("Controls".to_string(), String::new())); // group header
            rows.push((
                "  Enforce WG Config".to_string(),
                if w.enforce_config { "✓ Yes".to_string() } else { "✗ No".to_string() },
            ));
            rows.push((
                "  Publish CW Metrics".to_string(),
                if w.publish_cw_metrics { "✓ Yes".to_string() } else { "✗ No".to_string() },
            ));
            rows.push((
                "  Requester Pays".to_string(),
                if w.requester_pays { "✓ Yes".to_string() } else { "✗ No".to_string() },
            ));
            rows.push((
                "  Bytes Scanned Cutoff".to_string(),
                w.bytes_scanned_cutoff
                    .map(fmt_bytes)
                    .unwrap_or_else(|| "—".to_string()),
            ));
            if let Some(r) = &w.execution_role {
                rows.push(("  Execution Role".to_string(), r.clone())); // jumpable (IAM ARN)
            }
            if let Some(a) = &w.additional_config {
                rows.push(("  Additional Config".to_string(), a.clone()));
            }
            rows
        }
        AthenaWorkgroupDetailSection::Tags => match tags {
            None | Some(crate::lazy::Lazy::Loading) => {
                vec![("".to_string(), "Loading tags…".to_string())]
            }
            Some(crate::lazy::Lazy::Error(e)) => error_rows(e),
            Some(crate::lazy::Lazy::Loaded(list)) => {
                if list.is_empty() {
                    return vec![("".to_string(), "No tags".to_string())];
                }
                list.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
            }
        },
    }
}

pub(super) fn render_athena_database_split(app: &App, d: &AthenaDatabase, area: Rect, frame: &mut Frame) {
    render_athena_chrome(
        app,
        "Athena Database",
        &d.db_name,
        &d.catalog,
        "",
        theme::text_dim(),
        &descriptor_tabs(app, &crate::aws::services::athena::ATHENA_DATABASE_SECTIONS),
        area,
        frame,
    );
}

pub fn athena_database_section_lines(
    d: &AthenaDatabase,
    section: AthenaDatabaseDetailSection,
    tables: Option<&crate::lazy::Lazy<Vec<crate::aws::services::athena::AthenaTable>>>,
) -> Vec<(String, String)> {
    match section {
        AthenaDatabaseDetailSection::Overview => {
            let mut rows = vec![
                ("Name".to_string(), d.db_name.clone()),
                ("Catalog".to_string(), d.catalog.clone()),
            ];
            if let Some(desc) = &d.description {
                rows.push(("Description".to_string(), desc.clone()));
            }
            if !d.parameters.is_empty() {
                rows.push((String::new(), String::new()));
                rows.push(("Parameters".to_string(), String::new())); // group header
                for (k, v) in &d.parameters {
                    rows.push((format!("  {}", k), v.clone()));
                }
            }
            rows
        }
        AthenaDatabaseDetailSection::Tables => match tables {
            None | Some(crate::lazy::Lazy::Loading) => {
                vec![("".to_string(), "Loading tables…".to_string())]
            }
            Some(crate::lazy::Lazy::Error(e)) => error_rows(e),
            Some(crate::lazy::Lazy::Loaded(list)) => {
                if list.is_empty() {
                    return vec![("".to_string(), "No tables".to_string())];
                }
                let mut rows = Vec::new();
                for t in list {
                    rows.push((t.name.clone(), String::new())); // group header
                    if let Some(tt) = &t.table_type {
                        rows.push(("  Type".to_string(), tt.clone()));
                    }
                    rows.push(("  Columns".to_string(), t.columns.len().to_string()));
                    // Column list (name : type), a handful shown inline.
                    for (cn, ct) in t.columns.iter().take(40) {
                        rows.push((format!("    {}", cn), ct.clone()));
                    }
                    if t.columns.len() > 40 {
                        rows.push((
                            format!("    … {} more columns", t.columns.len() - 40),
                            String::new(),
                        ));
                    }
                    if !t.partition_keys.is_empty() {
                        let keys = t
                            .partition_keys
                            .iter()
                            .map(|(n, _)| n.clone())
                            .collect::<Vec<_>>()
                            .join(", ");
                        rows.push(("  Partition Keys".to_string(), keys));
                    }
                    if let Some(loc) = &t.location {
                        rows.push(("  Location".to_string(), loc.clone()));
                    }
                    if let Some(c) = &t.created {
                        rows.push(("  Created".to_string(), c.clone()));
                    }
                    rows.push((String::new(), String::new()));
                }
                rows
            }
        },
    }
}

pub(super) fn render_athena_query_split(app: &App, q: &AthenaQueryExecution, area: Rect, frame: &mut Frame) {
    let color = match q.state.as_str() {
        "SUCCEEDED" => theme::success(),
        "RUNNING" | "QUEUED" => theme::warning(),
        "FAILED" => theme::error(),
        _ => theme::text_dim(),
    };
    let subtitle = q
        .data_scanned
        .map(|b| format!("{} scanned", fmt_bytes(b)))
        .unwrap_or_default();
    render_athena_chrome(
        app,
        "Athena Query",
        &q.id,
        &subtitle,
        &q.state,
        color,
        &descriptor_tabs(app, &crate::aws::services::athena::ATHENA_QUERY_SECTIONS),
        area,
        frame,
    );
}

pub fn athena_query_section_lines(
    q: &AthenaQueryExecution,
    section: AthenaQueryDetailSection,
) -> Vec<(String, String)> {
    match section {
        AthenaQueryDetailSection::Overview => {
            let mut rows = vec![
                ("Query ID".to_string(), q.id.clone()),
                ("State".to_string(), q.state.clone()),
            ];
            if let Some(st) = &q.statement_type {
                rows.push(("Statement".to_string(), st.clone()));
            }
            if let Some(wg) = &q.workgroup {
                rows.push(("Workgroup".to_string(), wg.clone()));
            }
            if let Some(db) = &q.database {
                rows.push(("Database".to_string(), db.clone()));
            }
            if let Some(cat) = &q.catalog {
                rows.push(("Catalog".to_string(), cat.clone()));
            }
            if let Some(ev) = &q.engine_version {
                rows.push(("Engine".to_string(), ev.clone()));
            }

            rows.push((String::new(), String::new()));
            rows.push(("Run".to_string(), String::new())); // group header
            rows.push((
                "  Data Scanned".to_string(),
                q.data_scanned.map(fmt_bytes).unwrap_or_else(|| "—".to_string()),
            ));
            rows.push((
                "  Runtime".to_string(),
                q.total_time_ms.map(fmt_millis).unwrap_or_else(|| "—".to_string()),
            ));
            if q.result_reused {
                rows.push(("  Result Reuse".to_string(), "✓ Reused previous result".to_string()));
            }
            if let Some(s) = &q.submitted {
                rows.push(("  Submitted".to_string(), s.clone()));
            }
            if let Some(c) = &q.completed {
                rows.push(("  Completed".to_string(), c.clone()));
            }
            if let Some(loc) = &q.output_location {
                rows.push(("  Output Location".to_string(), loc.clone()));
            }

            if q.state == "FAILED" || q.error_message.is_some() || q.state_reason.is_some() {
                rows.push((String::new(), String::new()));
                rows.push(("Error".to_string(), String::new())); // group header
                if let Some(r) = &q.state_reason {
                    rows.push(("  Reason".to_string(), format!("✗ {}", r)));
                }
                if let Some(m) = &q.error_message {
                    rows.push(("  Message".to_string(), m.clone()));
                }
            }
            rows
        }
        AthenaQueryDetailSection::Query => {
            let mut rows = Vec::new();
            rows.push(("SQL".to_string(), String::new())); // group header
            rows.push((String::new(), String::new()));
            push_sql_lines(&mut rows, &q.query);
            rows
        }
        AthenaQueryDetailSection::Statistics => {
            let stat = |label: &str, ms: Option<i64>| {
                (
                    label.to_string(),
                    ms.map(fmt_millis).unwrap_or_else(|| "—".to_string()),
                )
            };
            vec![
                (
                    "Data Scanned".to_string(),
                    q.data_scanned.map(fmt_bytes).unwrap_or_else(|| "—".to_string()),
                ),
                (String::new(), String::new()),
                ("Timing".to_string(), String::new()), // group header
                stat("  Total Execution", q.total_time_ms),
                stat("  Engine Execution", q.engine_time_ms),
                stat("  Query Queue", q.queue_time_ms),
                stat("  Query Planning", q.planning_time_ms),
                stat("  Service Processing", q.service_processing_ms),
            ]
        }
    }
}

pub(super) fn render_athena_saved_query_split(
    app: &App,
    q: &AthenaNamedQuery,
    area: Rect,
    frame: &mut Frame,
) {
    render_athena_chrome(
        app,
        "Athena Saved Query",
        &q.name,
        &q.database,
        "",
        theme::text_dim(),
        &descriptor_tabs(app, &crate::aws::services::athena::ATHENA_SAVED_QUERY_SECTIONS),
        area,
        frame,
    );
}

pub fn athena_saved_query_section_lines(
    q: &AthenaNamedQuery,
    section: AthenaSavedQueryDetailSection,
) -> Vec<(String, String)> {
    match section {
        AthenaSavedQueryDetailSection::Overview => {
            let mut rows = vec![
                ("Name".to_string(), q.name.clone()),
                ("Database".to_string(), q.database.clone()),
            ];
            if let Some(wg) = &q.workgroup {
                rows.push(("Workgroup".to_string(), wg.clone()));
            }
            if let Some(d) = &q.description {
                rows.push(("Description".to_string(), d.clone()));
            }
            rows.push(("Query ID".to_string(), q.id.clone()));
            rows
        }
        AthenaSavedQueryDetailSection::Query => {
            let mut rows = Vec::new();
            rows.push(("SQL".to_string(), String::new())); // group header
            rows.push((String::new(), String::new()));
            push_sql_lines(&mut rows, &q.query);
            rows
        }
    }
}
