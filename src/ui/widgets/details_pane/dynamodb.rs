use super::*;

// ── DynamoDB table split pane ──────────────────────────────────────────────────

pub(super) fn render_ddb_table_split(app: &App, table: &DdbTable, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 4, "i items");
    let mut block = theme::pane_block("DynamoDB Table", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let state_color = match table.status.as_str() {
        "ACTIVE" => theme::success(),
        "CREATING" | "UPDATING" => theme::warning(),
        "DELETING" => theme::error(),
        _ => theme::text_dim(),
    };
    let header = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                table.name.clone(),
                Style::default()
                    .fg(crate::ui::theme::text_primary())
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(table.status.clone(), Style::default().fg(state_color)),
            Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
            Span::styled(table.billing_mode.clone(), Style::default().fg(theme::text_dim())),
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
    render_section_tab_bar(
        app,
        chunks[2],
        frame,
        &descriptor_tabs(app, &crate::aws::services::dynamodb::DDB_TABLE_SECTIONS),
    );
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub fn ddb_table_section_lines(
    table: &DdbTable,
    section: DdbTableDetailSection,
    ttl: Option<&crate::lazy::Lazy<(bool, Option<String>)>>,
    tags: Option<&crate::lazy::Lazy<std::collections::HashMap<String, String>>>,
) -> Vec<(String, String)> {
    match section {
        DdbTableDetailSection::Details => {
            let mut rows = vec![
                ("Name".to_string(), table.name.clone()),
                ("Status".to_string(), table.status.clone()),
                ("Billing Mode".to_string(), table.billing_mode.clone()),
                (
                    "Items (approx)".to_string(),
                    format!("{}  (≈, ~6h stale)", table.item_count),
                ),
                ("Size".to_string(), fmt_bytes(table.size_bytes)),
                (String::new(), String::new()),
                ("Partition Key".to_string(), format!("{} ({})", table.partition_key, table.partition_key_type)),
            ];
            if let (Some(sk), Some(skt)) = (&table.sort_key, &table.sort_key_type) {
                rows.push(("Sort Key".to_string(), format!("{} ({})", sk, skt)));
            }
            rows.push((String::new(), String::new()));
            rows.push((
                "Stream".to_string(),
                if table.stream_enabled {
                    table
                        .stream_view_type
                        .clone()
                        .unwrap_or_else(|| "enabled".to_string())
                } else {
                    "disabled".to_string()
                },
            ));
            let ttl_text = match ttl {
                None | Some(crate::lazy::Lazy::Loading) => "…".to_string(),
                Some(crate::lazy::Lazy::Error(e)) => format!("⚠ {}", e),
                Some(crate::lazy::Lazy::Loaded((enabled, attribute))) => {
                    if *enabled {
                        format!("enabled ({})", attribute.clone().unwrap_or_default())
                    } else {
                        "disabled".to_string()
                    }
                }
            };
            rows.push(("TTL".to_string(), ttl_text));
            if let Some(c) = &table.created {
                rows.push(("Created".to_string(), c.clone()));
            }
            rows.push(("ARN".to_string(), table.arn.clone()));
            rows
        }
        DdbTableDetailSection::Indexes => {
            let mut rows = Vec::new();
            rows.push((format!("Global Secondary Indexes ({})", table.gsis.len()), String::new()));
            rows.push((String::new(), String::new()));
            if table.gsis.is_empty() {
                rows.push(("  none".to_string(), String::new()));
            } else {
                for g in &table.gsis {
                    rows.push((g.name.clone(), String::new())); // group header
                    rows.push(("Keys".to_string(), g.keys.clone()));
                    rows.push(("Projection".to_string(), g.projection.clone()));
                    if !g.status.is_empty() {
                        rows.push(("Status".to_string(), g.status.clone()));
                    }
                    if let (Some(r), Some(w)) = (g.read_capacity, g.write_capacity) {
                        rows.push(("Capacity".to_string(), format!("{} RCU / {} WCU", r, w)));
                    }
                    rows.push((String::new(), String::new()));
                }
            }
            rows.push((format!("Local Secondary Indexes ({})", table.lsis.len()), String::new()));
            rows.push((String::new(), String::new()));
            if table.lsis.is_empty() {
                rows.push(("  none".to_string(), String::new()));
            } else {
                for l in &table.lsis {
                    rows.push((l.name.clone(), String::new()));
                    rows.push(("Keys".to_string(), l.keys.clone()));
                    rows.push(("Projection".to_string(), l.projection.clone()));
                    rows.push((String::new(), String::new()));
                }
            }
            rows
        }
        DdbTableDetailSection::Capacity => {
            let mut rows = vec![("Billing Mode".to_string(), table.billing_mode.clone())];
            if table.billing_mode == "PAY_PER_REQUEST" {
                rows.push((
                    "Mode".to_string(),
                    "On-demand — capacity scales automatically".to_string(),
                ));
            } else {
                rows.push((
                    "Table RCU".to_string(),
                    table.read_capacity.map(|v| v.to_string()).unwrap_or_else(|| "—".to_string()),
                ));
                rows.push((
                    "Table WCU".to_string(),
                    table.write_capacity.map(|v| v.to_string()).unwrap_or_else(|| "—".to_string()),
                ));
                let provisioned_gsis: Vec<&crate::aws::services::dynamodb::DdbIndex> = table
                    .gsis
                    .iter()
                    .filter(|g| g.read_capacity.is_some())
                    .collect();
                if !provisioned_gsis.is_empty() {
                    rows.push((String::new(), String::new()));
                    rows.push(("Per-GSI capacity".to_string(), String::new())); // group header
                    for g in provisioned_gsis {
                        rows.push((
                            format!("  {}", g.name),
                            format!(
                                "{} RCU / {} WCU",
                                g.read_capacity.unwrap_or(0),
                                g.write_capacity.unwrap_or(0)
                            ),
                        ));
                    }
                }
            }
            rows
        }
        DdbTableDetailSection::Tags => match tags {
            None | Some(crate::lazy::Lazy::Loading) => {
                vec![("".to_string(), "Loading tags…".to_string())]
            }
            Some(crate::lazy::Lazy::Error(e)) => error_rows(e),
            Some(crate::lazy::Lazy::Loaded(t)) => tag_rows(t),
        },
    }
}
