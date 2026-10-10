use super::*;

// ── SSM parameter split pane ──────────────────────────────────────────────

pub(super) fn render_ssm_parameter_split(app: &App, param: &SsmParameter, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 4, "x reveal · Y copy value");

    let mut block = theme::pane_block("SSM Parameter", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let header_lines = build_ssm_param_header_lines(param);
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
    render_ssm_param_section_tabs(app, chunks[2], frame);
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub(super) fn build_ssm_param_header_lines(param: &SsmParameter) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = vec![Line::raw("")];

    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            param.name.clone(),
            Style::default()
                .fg(crate::ui::theme::text_primary())
                .add_modifier(Modifier::BOLD),
        ),
    ]));

    lines.push(Line::raw(""));
    lines.push(header_kv(
        "Type",
        &format!("{} · {} · v{}", param.param_type, param.tier, param.version),
    ));
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            "x reveal value · Y copy to clipboard",
            Style::default().fg(theme::text_dim()),
        ),
    ]));

    lines.push(Line::raw(""));
    lines
}

pub(super) fn render_ssm_param_section_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let tabs = descriptor_tabs(app, &crate::aws::services::ssm::SSM_PARAMETER_SECTIONS);
    render_section_tab_bar(app, area, frame, &tabs);
}

pub fn ssm_parameter_section_lines(
    param: &SsmParameter,
    section: SsmParameterDetailSection,
    detail: Option<&Lazy<SsmParamDetail>>,
) -> Vec<(String, String)> {
    match section {
        SsmParameterDetailSection::Details => ssm_param_details_lines(param),
        SsmParameterDetailSection::Value => ssm_param_value_lines(param),
        SsmParameterDetailSection::History => ssm_param_history_lines(param, detail),
        SsmParameterDetailSection::Tags => ssm_param_tags_lines(detail),
    }
}

pub(super) fn ssm_param_details_lines(param: &SsmParameter) -> Vec<(String, String)> {
    let mut rows = vec![
        ("Name".to_string(), param.name.clone()),
        ("Type".to_string(), param.param_type.clone()),
        ("Tier".to_string(), param.tier.clone()),
        ("Version".to_string(), param.version.to_string()),
    ];
    if let Some(dt) = &param.data_type {
        rows.push(("Data Type".to_string(), dt.clone()));
    }
    rows.push(("".to_string(), "".to_string()));
    if !param.last_modified.is_empty() {
        rows.push(("Last Modified".to_string(), param.last_modified.clone()));
    }
    if let Some(user) = &param.last_modified_user {
        rows.push(("Modified By".to_string(), user.clone()));
    }
    rows.push(("".to_string(), "".to_string()));
    let encryption = if param.is_secure() {
        param
            .kms_key_id
            .as_ref()
            .map(|k| format!("SecureString · KMS ({})", k))
            .unwrap_or_else(|| "SecureString · aws/ssm (default)".to_string())
    } else {
        "None (plaintext)".to_string()
    };
    rows.push(("Encryption".to_string(), encryption));
    if let Some(desc) = &param.description {
        rows.push(("Description".to_string(), desc.clone()));
    }
    if let Some(pattern) = &param.allowed_pattern {
        rows.push(("Allowed Pattern".to_string(), pattern.clone()));
    }
    if !param.arn.is_empty() {
        rows.push(("".to_string(), "".to_string()));
        rows.push(("ARN".to_string(), param.arn.clone()));
    }
    rows.push(("".to_string(), "".to_string()));
    rows
}

pub(super) fn ssm_param_value_lines(param: &SsmParameter) -> Vec<(String, String)> {
    let mut rows = vec![("".to_string(), "".to_string())];
    rows.push(("Current Value".to_string(), "x reveal · Y copy".to_string()));
    rows.push(("".to_string(), "".to_string()));
    if param.is_secure() {
        rows.push((
            "  SecureString".to_string(),
            "value is KMS-encrypted; revealed on demand only".to_string(),
        ));
    } else {
        rows.push((
            "  Plaintext".to_string(),
            "press x to view the current value in $EDITOR".to_string(),
        ));
    }
    rows.push(("".to_string(), "".to_string()));
    rows.push((
        "Version".to_string(),
        format!("v{} (see History for older versions)", param.version),
    ));
    rows.push(("".to_string(), "".to_string()));
    rows
}

pub(super) fn ssm_param_history_lines(
    param: &SsmParameter,
    detail: Option<&Lazy<SsmParamDetail>>,
) -> Vec<(String, String)> {
    let mut rows = vec![("".to_string(), "".to_string())];
    match detail {
        None | Some(Lazy::Loading) => {
            rows.push(("  Loading history…".to_string(), "".to_string()));
        }
        Some(Lazy::Error(e)) => {
            rows.extend(error_rows(e));
        }
        Some(Lazy::Loaded((history, _))) => {
            if history.is_empty() {
                rows.push(("  No history".to_string(), "".to_string()));
            } else {
                for v in history {
                    // Group header per version.
                    let mut head = format!("v{}", v.version);
                    if v.version == param.version {
                        head.push_str(" (current)");
                    }
                    if v.changed {
                        head.push_str("  · changed");
                    }
                    rows.push((head, "".to_string()));
                    if !v.last_modified.is_empty() {
                        rows.push(("  Modified".to_string(), v.last_modified.clone()));
                    }
                    if let Some(u) = &v.user {
                        rows.push(("  By".to_string(), u.clone()));
                    }
                    if !v.param_type.is_empty() {
                        rows.push(("  Type".to_string(), v.param_type.clone()));
                    }
                    if !v.labels.is_empty() {
                        rows.push(("  Labels".to_string(), v.labels.join(", ")));
                    }
                    if let Some(val) = &v.value {
                        rows.push(("  Value".to_string(), val.clone()));
                    }
                    if let Some(desc) = &v.description {
                        rows.push(("  Description".to_string(), desc.clone()));
                    }
                    rows.push(("".to_string(), "".to_string()));
                }
            }
        }
    }
    rows
}

pub(super) fn ssm_param_tags_lines(detail: Option<&Lazy<SsmParamDetail>>) -> Vec<(String, String)> {
    let mut rows = vec![("".to_string(), "".to_string())];
    match detail {
        None | Some(Lazy::Loading) => {
            rows.push(("  Loading tags…".to_string(), "".to_string()));
        }
        Some(Lazy::Error(e)) => {
            rows.extend(error_rows(e));
        }
        Some(Lazy::Loaded((_, tags))) => {
            if tags.is_empty() {
                rows.push(("  No tags".to_string(), "".to_string()));
            } else {
                for (key, value) in tags {
                    rows.push((format!("  {}", key), value.clone()));
                }
            }
        }
    }
    rows.push(("".to_string(), "".to_string()));
    rows
}

// ── SSM document split pane ───────────────────────────────────────────────

pub(super) fn render_ssm_document_split(app: &App, doc: &SsmDocument, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 3, "e edit content");

    let mut block = theme::pane_block("SSM Document", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let header_lines = build_ssm_doc_header_lines(doc);
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
    render_ssm_doc_section_tabs(app, chunks[2], frame);
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub(super) fn build_ssm_doc_header_lines(doc: &SsmDocument) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = vec![Line::raw("")];
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            doc.name.clone(),
            Style::default()
                .fg(crate::ui::theme::text_primary())
                .add_modifier(Modifier::BOLD),
        ),
    ]));
    lines.push(Line::raw(""));
    lines.push(header_kv(
        "Type",
        &format!("{} · {} · {}", doc.doc_type, doc.format, doc.owner),
    ));
    lines.push(Line::raw(""));
    lines
}

pub(super) fn render_ssm_doc_section_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let tabs = descriptor_tabs(app, &crate::aws::services::ssm::SSM_DOCUMENT_SECTIONS);
    render_section_tab_bar(app, area, frame, &tabs);
}

pub fn ssm_document_section_lines(
    doc: &SsmDocument,
    section: SsmDocumentDetailSection,
    content: Option<&Lazy<SsmDocContent>>,
) -> Vec<(String, String)> {
    match section {
        SsmDocumentDetailSection::Details => ssm_doc_details_lines(doc),
        SsmDocumentDetailSection::Content => ssm_doc_content_lines(content),
        SsmDocumentDetailSection::Tags => ssm_doc_tags_lines(doc),
    }
}

pub(super) fn ssm_doc_details_lines(doc: &SsmDocument) -> Vec<(String, String)> {
    let mut rows = vec![
        ("Name".to_string(), doc.name.clone()),
        ("Type".to_string(), doc.doc_type.clone()),
        ("Format".to_string(), doc.format.clone()),
        ("Owner".to_string(), doc.owner.clone()),
    ];
    if !doc.platforms.is_empty() {
        rows.push(("Platforms".to_string(), doc.platforms.clone()));
    }
    rows.push(("".to_string(), "".to_string()));
    if !doc.default_version.is_empty() {
        rows.push(("Default Version".to_string(), doc.default_version.clone()));
    }
    if let Some(vn) = &doc.version_name {
        rows.push(("Version Name".to_string(), vn.clone()));
    }
    if !doc.schema_version.is_empty() {
        rows.push(("Schema Version".to_string(), doc.schema_version.clone()));
    }
    if let Some(tt) = &doc.target_type {
        rows.push(("Target Type".to_string(), tt.clone()));
    }
    rows.push(("".to_string(), "".to_string()));
    rows.push(("Content".to_string(), "2 view · e edit".to_string()));
    rows.push(("".to_string(), "".to_string()));
    rows
}

pub(super) fn ssm_doc_content_lines(content: Option<&Lazy<SsmDocContent>>) -> Vec<(String, String)> {
    let mut rows = vec![("".to_string(), "".to_string())];
    match content {
        None | Some(Lazy::Loading) => {
            rows.push(("  Loading content…".to_string(), "".to_string()));
        }
        Some(Lazy::Error(e)) => {
            rows.extend(error_rows(e));
        }
        Some(Lazy::Loaded((content, _))) => {
            if content.is_empty() {
                rows.push(("  (empty document)".to_string(), "".to_string()));
            } else {
                // Plain content lines (leading space → rendered raw, no colon).
                for line in content.lines() {
                    rows.push((format!(" {}", line), "".to_string()));
                }
            }
        }
    }
    rows.push(("".to_string(), "".to_string()));
    rows
}

pub(super) fn ssm_doc_tags_lines(doc: &SsmDocument) -> Vec<(String, String)> {
    let mut rows = vec![("".to_string(), "".to_string())];
    if doc.tags.is_empty() {
        rows.push(("  No tags".to_string(), "".to_string()));
    } else {
        let mut sorted: Vec<(&String, &String)> = doc.tags.iter().collect();
        sorted.sort_by_key(|(k, _)| k.as_str());
        for (key, value) in sorted {
            rows.push((format!("  {}", key), value.clone()));
        }
    }
    rows.push(("".to_string(), "".to_string()));
    rows
}

// ── SSM association split pane (State Manager) ────────────────────────────

pub(super) fn render_ssm_association_split(app: &App, assoc: &SsmAssociation, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 4, "");

    let mut block = theme::pane_block("SSM Association", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let status = assoc.status.as_deref().unwrap_or("no runs");
    let (state_color, state_dot) = match status.to_ascii_lowercase().as_str() {
        "success" => (theme::success(), "● "),
        "failed" => (theme::error(), "● "),
        "pending" => (theme::warning(), "◌ "),
        _ => (theme::text_dim(), "○ "),
    };
    let header_lines = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                assoc.name().to_string(),
                Style::default()
                    .fg(crate::ui::theme::text_primary())
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(state_dot.to_string(), Style::default().fg(state_color)),
            Span::styled(
                status.to_string(),
                Style::default().fg(state_color).add_modifier(Modifier::BOLD),
            ),
            Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
            Span::styled(
                assoc.document_name.clone(),
                Style::default().fg(theme::aws_orange()),
            ),
        ]),
        Line::raw(""),
    ];
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
    let tabs = descriptor_tabs(app, &crate::aws::services::ssm::SSM_ASSOCIATION_SECTIONS);
    render_section_tab_bar(app, chunks[2], frame, &tabs);
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub fn ssm_association_section_lines(
    assoc: &SsmAssociation,
    section: SsmAssociationDetailSection,
    detail: Option<&Lazy<SsmAssocDetail>>,
) -> Vec<(String, String)> {
    match section {
        SsmAssociationDetailSection::Overview => ssm_assoc_overview_lines(assoc),
        SsmAssociationDetailSection::Targets => ssm_assoc_targets_lines(assoc),
        SsmAssociationDetailSection::Executions => ssm_assoc_executions_lines(detail),
        SsmAssociationDetailSection::Tags => ssm_assoc_tags_lines(detail),
    }
}

pub(super) fn ssm_assoc_overview_lines(assoc: &SsmAssociation) -> Vec<(String, String)> {
    let mut rows = vec![(
        "Association ID".to_string(),
        assoc.association_id.clone(),
    )];
    if let Some(n) = &assoc.association_name {
        rows.push(("Name".to_string(), n.clone()));
    }
    rows.push(("Document".to_string(), assoc.document_name.clone()));
    if let Some(v) = &assoc.document_version {
        rows.push(("Document Version".to_string(), v.clone()));
    }
    if let Some(v) = &assoc.association_version {
        rows.push(("Association Version".to_string(), v.clone()));
    }
    rows.push(("".to_string(), "".to_string()));
    rows.push((
        "Status".to_string(),
        assoc.status.clone().unwrap_or_else(|| "no runs yet".to_string()),
    ));
    if let Some(d) = &assoc.detailed_status {
        rows.push(("Detailed Status".to_string(), d.clone()));
    }
    if !assoc.status_counts.is_empty() {
        let counts = assoc
            .status_counts
            .iter()
            .map(|(k, v)| format!("{} {}", v, k))
            .collect::<Vec<_>>()
            .join(", ");
        rows.push(("Resources".to_string(), counts));
    }
    if !assoc.last_execution.is_empty() {
        rows.push(("Last Execution".to_string(), assoc.last_execution.clone()));
    }
    rows.push(("".to_string(), "".to_string()));
    if let Some(s) = &assoc.schedule {
        rows.push(("Schedule".to_string(), s.clone()));
    }
    if let Some(o) = assoc.schedule_offset {
        rows.push(("Schedule Offset".to_string(), o.to_string()));
    }
    if let Some(i) = &assoc.instance_id {
        rows.push(("Instance ID".to_string(), i.clone()));
    }
    rows.push(("".to_string(), "".to_string()));
    rows
}

pub(super) fn ssm_assoc_targets_lines(assoc: &SsmAssociation) -> Vec<(String, String)> {
    let mut rows = vec![("".to_string(), "".to_string())];
    if let Some(i) = &assoc.instance_id {
        rows.push(("Targets".to_string(), "".to_string()));
        rows.push(("  Instance".to_string(), i.clone()));
    } else if assoc.targets.is_empty() {
        rows.push(("  No target expressions".to_string(), "".to_string()));
    } else {
        rows.push(("Targets".to_string(), "".to_string()));
        for (key, values) in &assoc.targets {
            push_ssm_target_rows(&mut rows, key, values);
        }
    }
    rows.push(("".to_string(), "".to_string()));
    if assoc.schedule.is_some() || assoc.schedule_offset.is_some() {
        rows.push(("Schedule".to_string(), "".to_string()));
        if let Some(s) = &assoc.schedule {
            rows.push(("  Expression".to_string(), s.clone()));
        }
        if let Some(o) = assoc.schedule_offset {
            rows.push(("  Offset".to_string(), o.to_string()));
        }
        rows.push(("".to_string(), "".to_string()));
    }
    rows
}

pub(super) fn ssm_assoc_executions_lines(detail: Option<&Lazy<SsmAssocDetail>>) -> Vec<(String, String)> {
    let mut rows = vec![("".to_string(), "".to_string())];
    match detail {
        None | Some(Lazy::Loading) => {
            rows.push(("  Loading executions…".to_string(), "".to_string()));
        }
        Some(Lazy::Error(e)) => {
            rows.extend(error_rows(e));
        }
        Some(Lazy::Loaded((executions, _))) => {
            if executions.is_empty() {
                rows.push(("  No executions yet".to_string(), "".to_string()));
            } else {
                rows.push((
                    format!(
                        "Executions (latest {})",
                        executions.len()
                    ),
                    "".to_string(),
                ));
                rows.push(("".to_string(), "".to_string()));
                for x in executions {
                    // Group header per run.
                    rows.push((format!("{} · {}", x.created, x.status), "".to_string()));
                    if let Some(d) = &x.detailed_status {
                        if d != &x.status {
                            rows.push(("  Detailed Status".to_string(), d.clone()));
                        }
                    }
                    if let Some(c) = &x.resource_counts {
                        rows.push(("  Resources".to_string(), c.clone()));
                    }
                    if !x.execution_id.is_empty() {
                        rows.push(("  Execution ID".to_string(), x.execution_id.clone()));
                    }
                    rows.push(("".to_string(), "".to_string()));
                }
            }
        }
    }
    rows
}

pub(super) fn ssm_assoc_tags_lines(detail: Option<&Lazy<SsmAssocDetail>>) -> Vec<(String, String)> {
    let mut rows = vec![("".to_string(), "".to_string())];
    match detail {
        None | Some(Lazy::Loading) => {
            rows.push(("  Loading tags…".to_string(), "".to_string()));
        }
        Some(Lazy::Error(e)) => {
            rows.extend(error_rows(e));
        }
        Some(Lazy::Loaded((_, tags))) => {
            if tags.is_empty() {
                rows.push(("  No tags".to_string(), "".to_string()));
            } else {
                for (key, value) in tags {
                    rows.push((format!("  {}", key), value.clone()));
                }
            }
        }
    }
    rows.push(("".to_string(), "".to_string()));
    rows
}

// ── SSM managed instance (Fleet) split pane ───────────────────────────────

pub(super) fn render_ssm_fleet_split(app: &App, inst: &SsmManagedInstance, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 4, "s session");

    let mut block = theme::pane_block("SSM Managed Instance", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let (state_color, state_dot) = match inst.ping_status.to_ascii_lowercase().as_str() {
        "online" => (theme::success(), "● "),
        "connectionlost" => (theme::error(), "● "),
        "inactive" => (theme::text_dim(), "○ "),
        _ => (theme::warning(), "◌ "),
    };
    let platform = match (&inst.platform_name, &inst.platform_version) {
        (Some(n), Some(v)) => format!("{} {}", n, v),
        (Some(n), None) => n.clone(),
        _ => inst.platform_type.clone(),
    };
    let header_lines = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                inst.name().to_string(),
                Style::default()
                    .fg(crate::ui::theme::text_primary())
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(state_dot.to_string(), Style::default().fg(state_color)),
            Span::styled(
                inst.ping_status.clone(),
                Style::default().fg(state_color).add_modifier(Modifier::BOLD),
            ),
            Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
            Span::styled(platform, Style::default().fg(theme::aws_orange())),
        ]),
        Line::raw(""),
    ];
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
    let tabs = descriptor_tabs(app, &crate::aws::services::ssm::SSM_FLEET_SECTIONS);
    render_section_tab_bar(app, chunks[2], frame, &tabs);
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub fn ssm_fleet_section_lines(
    inst: &SsmManagedInstance,
    section: SsmFleetDetailSection,
    assocs: Option<&Lazy<Vec<SsmInstanceAssoc>>>,
    inventory: Option<&Lazy<Box<SsmInventoryData>>>,
    patches: Option<&Lazy<SsmInstancePatches>>,
) -> Vec<(String, String)> {
    match section {
        SsmFleetDetailSection::Overview => ssm_fleet_overview_lines(inst),
        SsmFleetDetailSection::Associations => ssm_fleet_assocs_lines(assocs),
        SsmFleetDetailSection::Inventory => ssm_fleet_inventory_lines(inventory),
        SsmFleetDetailSection::Patches => ssm_fleet_patches_lines(patches),
    }
}

pub(super) fn ssm_fleet_overview_lines(inst: &SsmManagedInstance) -> Vec<(String, String)> {
    let mut rows = vec![
        ("Instance ID".to_string(), inst.instance_id.clone()),
        ("Kind".to_string(), inst.instance_kind.clone()),
        ("Ping Status".to_string(), inst.ping_status.clone()),
    ];
    if !inst.last_ping.is_empty() {
        rows.push(("Last Ping".to_string(), inst.last_ping.clone()));
    }
    rows.push(("".to_string(), "".to_string()));
    let agent = if inst.is_latest {
        format!("{} (latest)", inst.agent_version)
    } else {
        format!("{} ⚠ update available", inst.agent_version)
    };
    rows.push(("Agent Version".to_string(), agent));
    if let Some(a) = &inst.association_status {
        rows.push(("Associations".to_string(), format!("{} (2 for detail)", a)));
    }
    rows.push(("".to_string(), "".to_string()));
    rows.push(("Platform".to_string(), "".to_string()));
    let platform = match (&inst.platform_name, &inst.platform_version) {
        (Some(n), Some(v)) => format!("{} {} ({})", n, v, inst.platform_type),
        (Some(n), None) => format!("{} ({})", n, inst.platform_type),
        _ => inst.platform_type.clone(),
    };
    rows.push(("  OS".to_string(), platform));
    if let Some(c) = &inst.computer_name {
        rows.push(("  Computer Name".to_string(), c.clone()));
    }
    if let Some(ip) = &inst.ip_address {
        rows.push(("  IP Address".to_string(), ip.clone()));
    }
    if let Some(r) = &inst.iam_role {
        rows.push(("  IAM Role".to_string(), r.clone()));
    }
    rows.push(("".to_string(), "".to_string()));
    rows
}

pub(super) fn ssm_fleet_assocs_lines(assocs: Option<&Lazy<Vec<SsmInstanceAssoc>>>) -> Vec<(String, String)> {
    let mut rows = vec![("".to_string(), "".to_string())];
    match assocs {
        None | Some(Lazy::Loading) => {
            rows.push(("  Loading associations…".to_string(), "".to_string()));
        }
        Some(Lazy::Error(e)) => {
            rows.extend(error_rows(e));
        }
        Some(Lazy::Loaded(list)) => {
            if list.is_empty() {
                rows.push((
                    "  No associations target this instance".to_string(),
                    "".to_string(),
                ));
            } else {
                for a in list {
                    // Group header per association.
                    let head = a
                        .association_name
                        .clone()
                        .filter(|n| !n.is_empty())
                        .unwrap_or_else(|| a.doc_name.clone());
                    rows.push((head, "".to_string()));
                    rows.push(("  Association ID".to_string(), a.association_id.clone()));
                    rows.push(("  Document".to_string(), a.doc_name.clone()));
                    let status = match &a.detailed_status {
                        Some(d) if d != &a.status => format!("{} ({})", a.status, d),
                        _ => a.status.clone(),
                    };
                    rows.push(("  Status".to_string(), status));
                    if !a.execution_date.is_empty() {
                        rows.push(("  Last Run".to_string(), a.execution_date.clone()));
                    }
                    if let Some(s) = &a.execution_summary {
                        rows.push(("  Summary".to_string(), s.clone()));
                    }
                    if let Some(c) = &a.error_code {
                        rows.push(("  ⚠ Error Code".to_string(), c.clone()));
                    }
                    rows.push(("".to_string(), "".to_string()));
                }
            }
        }
    }
    rows
}

pub(super) fn ssm_fleet_inventory_lines(inventory: Option<&Lazy<Box<SsmInventoryData>>>) -> Vec<(String, String)> {
    let mut rows = vec![("".to_string(), "".to_string())];
    match inventory {
        None | Some(Lazy::Loading) => {
            rows.push(("  Loading inventory…".to_string(), "".to_string()));
        }
        Some(Lazy::Error(e)) => {
            rows.extend(error_rows(e));
        }
        Some(Lazy::Loaded(data)) => {
            if data.total_apps == 0 && data.detail.is_empty() {
                rows.push((
                    "  No inventory collected — the AWS-GatherSoftwareInventory".to_string(),
                    "".to_string(),
                ));
                rows.push((
                    "  association is not gathering on this instance".to_string(),
                    "".to_string(),
                ));
                rows.push(("".to_string(), "".to_string()));
                return rows;
            }
            if !data.detail.is_empty() {
                rows.push(("System".to_string(), "".to_string()));
                for (k, v) in &data.detail {
                    rows.push((format!("  {}", k), v.clone()));
                }
                rows.push(("".to_string(), "".to_string()));
            }
            let head = if data.total_apps > data.apps.len() {
                format!(
                    "Installed Applications (first {} of {})",
                    data.apps.len(),
                    data.total_apps
                )
            } else {
                format!("Installed Applications ({})", data.total_apps)
            };
            rows.push((head, "".to_string()));
            if let Some(t) = &data.capture_time {
                rows.push(("  Captured".to_string(), t.clone()));
            }
            rows.push(("".to_string(), "".to_string()));
            for app in &data.apps {
                let mut v = app.version.clone();
                if !app.publisher.is_empty() {
                    v = format!("{} · {}", v, app.publisher);
                }
                rows.push((format!("  {}", app.name), v));
            }
            rows.push(("".to_string(), "".to_string()));
        }
    }
    rows
}

pub(super) fn ssm_fleet_patches_lines(patches: Option<&Lazy<SsmInstancePatches>>) -> Vec<(String, String)> {
    let mut rows = vec![("".to_string(), "".to_string())];
    match patches {
        None | Some(Lazy::Loading) => {
            rows.push(("  Loading patch state…".to_string(), "".to_string()));
        }
        Some(Lazy::Error(e)) => {
            rows.extend(error_rows(e));
        }
        Some(Lazy::Loaded((state, patches, truncated))) => {
            let Some(s) = state else {
                rows.push((
                    "  Never patched by Patch Manager — no patch state recorded".to_string(),
                    "".to_string(),
                ));
                rows.push(("".to_string(), "".to_string()));
                return rows;
            };
            rows.push(("Patch State".to_string(), "".to_string()));
            rows.push(("  Baseline".to_string(), s.baseline_id.clone()));
            if !s.patch_group.is_empty() {
                rows.push(("  Patch Group".to_string(), s.patch_group.clone()));
            }
            rows.push((
                "  Last Operation".to_string(),
                format!("{} · {}", s.operation, s.operation_end),
            ));
            if let Some(r) = &s.reboot_option {
                rows.push(("  Reboot Option".to_string(), r.clone()));
            }
            rows.push(("".to_string(), "".to_string()));
            rows.push(("Compliance Counts".to_string(), "".to_string()));
            rows.push(("  Installed".to_string(), s.installed.to_string()));
            if s.installed_other > 0 {
                rows.push(("  Installed (other)".to_string(), s.installed_other.to_string()));
            }
            if s.installed_pending_reboot > 0 {
                rows.push((
                    "  ⚠ Pending Reboot".to_string(),
                    s.installed_pending_reboot.to_string(),
                ));
            }
            if s.installed_rejected > 0 {
                rows.push(("  ⚠ Rejected".to_string(), s.installed_rejected.to_string()));
            }
            if s.missing > 0 {
                rows.push(("  ⚠ Missing".to_string(), s.missing.to_string()));
            }
            if s.failed > 0 {
                rows.push(("  ✗ Failed".to_string(), s.failed.to_string()));
            }
            rows.push(("  Not Applicable".to_string(), s.not_applicable.to_string()));
            if let Some(c) = s.critical_non_compliant.filter(|c| *c > 0) {
                rows.push(("  ✗ Critical Non-Compliant".to_string(), c.to_string()));
            }
            if let Some(c) = s.security_non_compliant.filter(|c| *c > 0) {
                rows.push(("  ⚠ Security Non-Compliant".to_string(), c.to_string()));
            }
            rows.push(("".to_string(), "".to_string()));
            if patches.is_empty() {
                rows.push((
                    "  ✓ No missing, failed, or reboot-pending patches".to_string(),
                    "".to_string(),
                ));
            } else {
                let head = if *truncated {
                    format!("Actionable Patches (first {})", MAX_INSTANCE_PATCH_ROWS)
                } else {
                    format!("Actionable Patches ({})", patches.len())
                };
                rows.push((head, "".to_string()));
                rows.push(("".to_string(), "".to_string()));
                for p in patches {
                    let state = match p.state.as_str() {
                        "MISSING" => "⚠ Missing",
                        "FAILED" => "✗ Failed",
                        "INSTALLED_PENDING_REBOOT" => "⚠ Pending Reboot",
                        "INSTALLED_REJECTED" => "⚠ Rejected",
                        "AVAILABLE_SECURITY_UPDATE" => "⚠ Security Update Available",
                        other => other,
                    };
                    rows.push((format!("  {}", p.title), state.to_string()));
                    let mut meta = p.classification.clone();
                    if !p.severity.is_empty() {
                        meta = format!("{} · {}", meta, p.severity);
                    }
                    if !p.kb_id.is_empty() && p.kb_id != p.title {
                        meta = format!("{} · {}", meta, p.kb_id);
                    }
                    if !meta.is_empty() {
                        rows.push((format!("   {}", meta), "".to_string()));
                    }
                    if let Some(c) = &p.cve_ids {
                        rows.push((format!("   {}", c), "".to_string()));
                    }
                }
            }
            rows.push(("".to_string(), "".to_string()));
        }
    }
    rows
}

/// Push one key-value row per target value so each instance id / tag value is
/// individually Enter-jumpable — a comma-joined value only ever resolves to
/// its first token.
pub(super) fn push_ssm_target_rows(rows: &mut Vec<(String, String)>, key: &str, values: &[String]) {
    for v in values {
        rows.push((format!("  {}", key), v.clone()));
    }
}

// ── SSM Run Command split pane ────────────────────────────────────────────

pub(super) fn ssm_run_state_style(status: &str) -> (Color, &'static str) {
    match status.to_ascii_lowercase().as_str() {
        "success" => (theme::success(), "● "),
        "failed" | "timedout" | "completedwithfailure" | "rejected" => (theme::error(), "● "),
        "cancelled" | "cancelling" => (theme::text_dim(), "○ "),
        _ => (theme::warning(), "◌ "),
    }
}

pub(super) fn render_ssm_command_split(app: &App, cmd: &SsmCommand, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 2, "");

    let mut block = theme::pane_block("SSM Command", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let (state_color, state_dot) = ssm_run_state_style(&cmd.status);
    let header_lines = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                cmd.document_name.clone(),
                Style::default()
                    .fg(crate::ui::theme::text_primary())
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(state_dot.to_string(), Style::default().fg(state_color)),
            Span::styled(
                cmd.status.clone(),
                Style::default().fg(state_color).add_modifier(Modifier::BOLD),
            ),
            Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
            Span::styled(cmd.requested.clone(), Style::default().fg(theme::aws_orange())),
        ]),
        Line::raw(""),
    ];
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
    let tabs = descriptor_tabs(app, &crate::aws::services::ssm::SSM_COMMAND_SECTIONS);
    render_section_tab_bar(app, chunks[2], frame, &tabs);
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub fn ssm_command_section_lines(
    cmd: &SsmCommand,
    section: SsmCommandDetailSection,
    invocations: Option<&Lazy<Vec<SsmCmdInvocation>>>,
) -> Vec<(String, String)> {
    match section {
        SsmCommandDetailSection::Overview => ssm_command_overview_lines(cmd),
        SsmCommandDetailSection::Invocations => ssm_command_invocations_lines(invocations),
    }
}

pub(super) fn ssm_command_overview_lines(cmd: &SsmCommand) -> Vec<(String, String)> {
    let mut rows = vec![
        ("Command ID".to_string(), cmd.command_id.clone()),
        ("Document".to_string(), cmd.document_name.clone()),
    ];
    if let Some(c) = &cmd.comment {
        rows.push(("Comment".to_string(), c.clone()));
    }
    rows.push(("Status".to_string(), cmd.status.clone()));
    if let Some(d) = &cmd.status_details {
        if d != &cmd.status {
            rows.push(("Status Details".to_string(), d.clone()));
        }
    }
    rows.push(("Requested".to_string(), cmd.requested.clone()));
    rows.push(("".to_string(), "".to_string()));
    rows.push(("Targets".to_string(), "".to_string()));
    for i in &cmd.instance_ids {
        rows.push(("  Instance".to_string(), i.clone()));
    }
    for (key, values) in &cmd.targets {
        push_ssm_target_rows(&mut rows, key, values);
    }
    rows.push((
        "  Progress".to_string(),
        format!(
            "{} of {} completed · {} errors{}",
            cmd.completed_count,
            cmd.target_count,
            cmd.error_count,
            if cmd.delivery_timed_out_count > 0 {
                format!(" · {} delivery timeouts", cmd.delivery_timed_out_count)
            } else {
                String::new()
            }
        ),
    ));
    rows.push(("".to_string(), "".to_string()));
    rows.push(("Execution".to_string(), "".to_string()));
    if let Some(m) = &cmd.max_concurrency {
        rows.push(("  Max Concurrency".to_string(), m.clone()));
    }
    if let Some(m) = &cmd.max_errors {
        rows.push(("  Max Errors".to_string(), m.clone()));
    }
    if let Some(t) = cmd.timeout_seconds {
        rows.push(("  Timeout".to_string(), fmt_secs(t as i64)));
    }
    if cmd.output_s3.is_some() || cmd.cw_log_group.is_some() {
        rows.push(("".to_string(), "".to_string()));
        rows.push(("Output".to_string(), "".to_string()));
        if let Some(s3) = &cmd.output_s3 {
            rows.push(("  Output S3".to_string(), s3.clone()));
        }
        if let Some(g) = &cmd.cw_log_group {
            rows.push(("  Log Group".to_string(), g.clone()));
        }
    }
    if !cmd.parameters.is_empty() {
        rows.push(("".to_string(), "".to_string()));
        rows.push(("Parameters".to_string(), "".to_string()));
        for (k, v) in &cmd.parameters {
            rows.push((format!("  {}", k), v.clone()));
        }
    }
    rows.push(("".to_string(), "".to_string()));
    rows
}

pub(super) fn ssm_command_invocations_lines(
    invocations: Option<&Lazy<Vec<SsmCmdInvocation>>>,
) -> Vec<(String, String)> {
    let mut rows = vec![("".to_string(), "".to_string())];
    match invocations {
        None | Some(Lazy::Loading) => {
            rows.push(("  Loading invocations…".to_string(), "".to_string()));
        }
        Some(Lazy::Error(e)) => {
            rows.extend(error_rows(e));
        }
        Some(Lazy::Loaded(list)) => {
            if list.is_empty() {
                rows.push(("  No invocations".to_string(), "".to_string()));
            } else {
                for inv in list {
                    // Group header per instance.
                    let head = match &inv.instance_name {
                        Some(n) => format!("{} ({})", inv.instance_id, n),
                        None => inv.instance_id.clone(),
                    };
                    rows.push((head, "".to_string()));
                    let status = match &inv.status_details {
                        Some(d) if d != &inv.status => format!("{} ({})", inv.status, d),
                        _ => inv.status.clone(),
                    };
                    rows.push(("  Status".to_string(), status));
                    if !inv.requested.is_empty() {
                        rows.push(("  Requested".to_string(), inv.requested.clone()));
                    }
                    for (name, status, code, output) in &inv.plugins {
                        rows.push((
                            format!("  {}", name),
                            format!("{} (exit {})", status, code),
                        ));
                        if let Some(o) = output {
                            for line in o.lines().take(12) {
                                rows.push((format!("    {}", line), "".to_string()));
                            }
                        }
                    }
                    rows.push(("".to_string(), "".to_string()));
                }
            }
        }
    }
    rows
}

// ── SSM automation execution split pane ───────────────────────────────────

pub(super) fn render_ssm_automation_split(
    app: &App,
    auto: &SsmAutomationExecution,
    area: Rect,
    frame: &mut Frame,
) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 2, "");

    let mut block = theme::pane_block("SSM Automation", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let (state_color, state_dot) = ssm_run_state_style(&auto.status);
    let header_lines = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                auto.document_name.clone(),
                Style::default()
                    .fg(crate::ui::theme::text_primary())
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(state_dot.to_string(), Style::default().fg(state_color)),
            Span::styled(
                auto.status.clone(),
                Style::default().fg(state_color).add_modifier(Modifier::BOLD),
            ),
            Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
            Span::styled(auto.start.clone(), Style::default().fg(theme::aws_orange())),
        ]),
        Line::raw(""),
    ];
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
    let tabs = descriptor_tabs(app, &crate::aws::services::ssm::SSM_AUTOMATION_SECTIONS);
    render_section_tab_bar(app, chunks[2], frame, &tabs);
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub fn ssm_automation_section_lines(
    auto: &SsmAutomationExecution,
    section: SsmAutomationDetailSection,
    steps: Option<&Lazy<Vec<SsmAutomationStep>>>,
) -> Vec<(String, String)> {
    match section {
        SsmAutomationDetailSection::Overview => ssm_automation_overview_lines(auto),
        SsmAutomationDetailSection::Steps => ssm_automation_steps_lines(steps),
    }
}

pub(super) fn ssm_automation_overview_lines(auto: &SsmAutomationExecution) -> Vec<(String, String)> {
    let mut rows = vec![
        ("Execution ID".to_string(), auto.execution_id.clone()),
        ("Document".to_string(), auto.document_name.clone()),
    ];
    if let Some(v) = &auto.document_version {
        rows.push(("Document Version".to_string(), v.clone()));
    }
    if let Some(t) = &auto.automation_type {
        rows.push(("Type".to_string(), t.clone()));
    }
    if let Some(m) = &auto.mode {
        rows.push(("Mode".to_string(), m.clone()));
    }
    rows.push(("".to_string(), "".to_string()));
    rows.push(("Status".to_string(), auto.status.clone()));
    if let Some(s) = &auto.current_step {
        rows.push(("Current Step".to_string(), s.clone()));
    }
    if let Some(a) = &auto.current_action {
        rows.push(("Current Action".to_string(), a.clone()));
    }
    if let Some(m) = &auto.failure_message {
        rows.push((format!("  ✗ {}", m), "".to_string()));
    }
    if !auto.start.is_empty() {
        rows.push(("Started".to_string(), auto.start.clone()));
    }
    if !auto.end.is_empty() {
        rows.push(("Ended".to_string(), auto.end.clone()));
    }
    if let Some(e) = &auto.executed_by {
        rows.push(("Executed By".to_string(), e.clone()));
    }
    if let Some(p) = &auto.parent_id {
        rows.push(("Parent Execution".to_string(), p.clone()));
    }
    if auto.target.is_some() || !auto.targets.is_empty() {
        rows.push(("".to_string(), "".to_string()));
        rows.push(("Targets".to_string(), "".to_string()));
        if let Some(t) = &auto.target {
            rows.push(("  Target".to_string(), t.clone()));
        }
        for (key, values) in &auto.targets {
            push_ssm_target_rows(&mut rows, key, values);
        }
    }
    if !auto.outputs.is_empty() {
        rows.push(("".to_string(), "".to_string()));
        rows.push(("Outputs".to_string(), "".to_string()));
        for (k, v) in &auto.outputs {
            rows.push((format!("  {}", k), v.clone()));
        }
    }
    rows.push(("".to_string(), "".to_string()));
    rows
}

pub(super) fn ssm_automation_steps_lines(steps: Option<&Lazy<Vec<SsmAutomationStep>>>) -> Vec<(String, String)> {
    let mut rows = vec![("".to_string(), "".to_string())];
    match steps {
        None | Some(Lazy::Loading) => {
            rows.push(("  Loading steps…".to_string(), "".to_string()));
        }
        Some(Lazy::Error(e)) => {
            rows.extend(error_rows(e));
        }
        Some(Lazy::Loaded(list)) => {
            if list.is_empty() {
                rows.push(("  No steps recorded".to_string(), "".to_string()));
            } else {
                for (i, s) in list.iter().enumerate() {
                    // Group header per step.
                    rows.push((format!("{}. {}", i + 1, s.name), "".to_string()));
                    rows.push(("  Action".to_string(), s.action.clone()));
                    rows.push(("  Status".to_string(), s.status.clone()));
                    if !s.start.is_empty() {
                        rows.push(("  Started".to_string(), s.start.clone()));
                    }
                    if let Some(d) = s.duration_secs {
                        rows.push(("  Duration".to_string(), fmt_secs(d)));
                    }
                    if let Some(m) = &s.failure_message {
                        rows.push((format!("  ✗ {}", m), "".to_string()));
                    }
                    rows.push(("".to_string(), "".to_string()));
                }
            }
        }
    }
    rows
}

// ── SSM maintenance window split pane ─────────────────────────────────────

pub(super) fn render_ssm_maint_window_split(app: &App, mw: &SsmMaintWindow, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 4, "");

    let mut block = theme::pane_block("SSM Maintenance Window", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let (state_color, state_label) = if mw.enabled {
        (theme::success(), "Enabled")
    } else {
        (theme::text_dim(), "Disabled")
    };
    let header_lines = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                mw.name().to_string(),
                Style::default()
                    .fg(crate::ui::theme::text_primary())
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("● ".to_string(), Style::default().fg(state_color)),
            Span::styled(
                state_label.to_string(),
                Style::default().fg(state_color).add_modifier(Modifier::BOLD),
            ),
            Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
            Span::styled(mw.schedule.clone(), Style::default().fg(theme::aws_orange())),
        ]),
        Line::raw(""),
    ];
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
    let tabs = descriptor_tabs(app, &crate::aws::services::ssm::SSM_MAINT_WINDOW_SECTIONS);
    render_section_tab_bar(app, chunks[2], frame, &tabs);
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub fn ssm_maint_window_section_lines(
    mw: &SsmMaintWindow,
    section: SsmMaintWindowDetailSection,
    detail: Option<&Lazy<SsmMaintWindowDetail>>,
) -> Vec<(String, String)> {
    match section {
        SsmMaintWindowDetailSection::Overview => ssm_mw_overview_lines(mw),
        SsmMaintWindowDetailSection::Targets => ssm_mw_targets_lines(detail),
        SsmMaintWindowDetailSection::Tasks => ssm_mw_tasks_lines(detail),
        SsmMaintWindowDetailSection::History => ssm_mw_history_lines(detail),
    }
}

pub(super) fn ssm_mw_overview_lines(mw: &SsmMaintWindow) -> Vec<(String, String)> {
    let mut rows = vec![
        ("Window ID".to_string(), mw.window_id.clone()),
        ("Name".to_string(), mw.window_name.clone()),
    ];
    if let Some(d) = &mw.description {
        rows.push(("Description".to_string(), d.clone()));
    }
    rows.push((
        "Enabled".to_string(),
        if mw.enabled { "✓ yes" } else { "✗ no" }.to_string(),
    ));
    rows.push(("".to_string(), "".to_string()));
    rows.push(("Schedule".to_string(), "".to_string()));
    rows.push(("  Expression".to_string(), mw.schedule.clone()));
    if let Some(tz) = &mw.schedule_timezone {
        rows.push(("  Timezone".to_string(), tz.clone()));
    }
    if let Some(o) = mw.schedule_offset {
        rows.push(("  Offset".to_string(), format!("{} cycles", o)));
    }
    if let Some(d) = mw.duration_hours {
        rows.push((
            "  Duration".to_string(),
            format!("{}h (cutoff {}h before end)", d, mw.cutoff_hours),
        ));
    }
    if let Some(n) = &mw.next_execution {
        rows.push(("  Next Execution".to_string(), n.clone()));
    }
    if let Some(s) = &mw.start_date {
        rows.push(("  Start Date".to_string(), s.clone()));
    }
    if let Some(e) = &mw.end_date {
        rows.push(("  End Date".to_string(), e.clone()));
    }
    rows.push(("".to_string(), "".to_string()));
    rows
}

pub(super) fn ssm_mw_targets_lines(detail: Option<&Lazy<SsmMaintWindowDetail>>) -> Vec<(String, String)> {
    let mut rows = vec![("".to_string(), "".to_string())];
    match detail {
        None | Some(Lazy::Loading) => {
            rows.push(("  Loading targets…".to_string(), "".to_string()));
        }
        Some(Lazy::Error(e)) => {
            rows.extend(error_rows(e));
        }
        Some(Lazy::Loaded((targets, _, _))) => {
            if targets.is_empty() {
                rows.push(("  No registered targets".to_string(), "".to_string()));
            } else {
                for (i, t) in targets.iter().enumerate() {
                    let head = t
                        .name
                        .clone()
                        .unwrap_or_else(|| format!("Target group {}", i + 1));
                    rows.push((head, "".to_string()));
                    rows.push(("  Resource Type".to_string(), t.resource_type.clone()));
                    for (key, values) in &t.targets {
                        push_ssm_target_rows(&mut rows, key, values);
                    }
                    rows.push(("".to_string(), "".to_string()));
                }
            }
        }
    }
    rows
}

pub(super) fn ssm_mw_tasks_lines(detail: Option<&Lazy<SsmMaintWindowDetail>>) -> Vec<(String, String)> {
    let mut rows = vec![("".to_string(), "".to_string())];
    match detail {
        None | Some(Lazy::Loading) => {
            rows.push(("  Loading tasks…".to_string(), "".to_string()));
        }
        Some(Lazy::Error(e)) => {
            rows.extend(error_rows(e));
        }
        Some(Lazy::Loaded((_, tasks, _))) => {
            if tasks.is_empty() {
                rows.push(("  No registered tasks".to_string(), "".to_string()));
            } else {
                for t in tasks {
                    let head = match &t.name {
                        Some(n) => format!("{} (priority {})", n, t.priority),
                        None => format!("Priority {}", t.priority),
                    };
                    rows.push((head, "".to_string()));
                    rows.push(("  Type".to_string(), t.task_type.clone()));
                    rows.push(("  Task".to_string(), t.task_arn.clone()));
                    if let Some(m) = &t.max_concurrency {
                        rows.push(("  Max Concurrency".to_string(), m.clone()));
                    }
                    if let Some(m) = &t.max_errors {
                        rows.push(("  Max Errors".to_string(), m.clone()));
                    }
                    if let Some(c) = &t.cutoff_behavior {
                        rows.push(("  Cutoff Behavior".to_string(), c.clone()));
                    }
                    rows.push(("".to_string(), "".to_string()));
                }
            }
        }
    }
    rows
}

pub(super) fn ssm_mw_history_lines(detail: Option<&Lazy<SsmMaintWindowDetail>>) -> Vec<(String, String)> {
    let mut rows = vec![("".to_string(), "".to_string())];
    match detail {
        None | Some(Lazy::Loading) => {
            rows.push(("  Loading history…".to_string(), "".to_string()));
        }
        Some(Lazy::Error(e)) => {
            rows.extend(error_rows(e));
        }
        Some(Lazy::Loaded((_, _, executions))) => {
            if executions.is_empty() {
                rows.push(("  No executions yet".to_string(), "".to_string()));
            } else {
                rows.push((
                    format!("Executions (latest {})", executions.len()),
                    "".to_string(),
                ));
                rows.push(("".to_string(), "".to_string()));
                for x in executions {
                    rows.push((format!("{} · {}", x.start, x.status), "".to_string()));
                    if let Some(d) = &x.status_details {
                        if d != &x.status {
                            rows.push(("  Details".to_string(), d.clone()));
                        }
                    }
                    if !x.end.is_empty() {
                        rows.push(("  Ended".to_string(), x.end.clone()));
                    }
                    if !x.execution_id.is_empty() {
                        rows.push(("  Execution ID".to_string(), x.execution_id.clone()));
                    }
                    rows.push(("".to_string(), "".to_string()));
                }
            }
        }
    }
    rows
}

// ── SSM patch baseline split pane ─────────────────────────────────────────

pub(super) fn render_ssm_baseline_split(app: &App, b: &SsmPatchBaseline, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 2, "");

    let mut block = theme::pane_block("SSM Patch Baseline", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut status_spans = vec![
        Span::raw("  "),
        Span::styled(
            b.operating_system.clone(),
            Style::default().fg(theme::aws_orange()),
        ),
    ];
    if b.default_baseline {
        status_spans.push(Span::styled("  ·  ", Style::default().fg(theme::text_dim())));
        status_spans.push(Span::styled(
            "default".to_string(),
            Style::default().fg(theme::success()),
        ));
    }
    if b.is_aws_provided() {
        status_spans.push(Span::styled("  ·  ", Style::default().fg(theme::text_dim())));
        status_spans.push(Span::styled(
            "AWS-provided".to_string(),
            Style::default().fg(theme::text_dim()),
        ));
    }
    let header_lines = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                b.baseline_name.clone(),
                Style::default()
                    .fg(crate::ui::theme::text_primary())
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(status_spans),
        Line::raw(""),
    ];
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
    let tabs = descriptor_tabs(app, &crate::aws::services::ssm::SSM_BASELINE_SECTIONS);
    render_section_tab_bar(app, chunks[2], frame, &tabs);
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub fn ssm_baseline_section_lines(
    b: &SsmPatchBaseline,
    section: SsmBaselineDetailSection,
    detail: Option<&Lazy<Box<SsmBaselineDetail>>>,
) -> Vec<(String, String)> {
    match section {
        SsmBaselineDetailSection::Overview => ssm_baseline_overview_lines(b),
        SsmBaselineDetailSection::Rules => ssm_baseline_rules_lines(detail),
    }
}

pub(super) fn ssm_baseline_overview_lines(b: &SsmPatchBaseline) -> Vec<(String, String)> {
    let mut rows = vec![
        ("Baseline ID".to_string(), b.short_id.clone()),
        ("Name".to_string(), b.baseline_name.clone()),
        ("Operating System".to_string(), b.operating_system.clone()),
        (
            "Default".to_string(),
            if b.default_baseline { "✓ yes" } else { "no" }.to_string(),
        ),
        (
            "Owner".to_string(),
            if b.is_aws_provided() { "AWS" } else { "custom" }.to_string(),
        ),
    ];
    if let Some(d) = &b.description {
        rows.push(("Description".to_string(), d.clone()));
    }
    rows.push(("".to_string(), "".to_string()));
    rows.push(("Rules".to_string(), "2 view approval rules".to_string()));
    rows.push(("".to_string(), "".to_string()));
    rows
}

pub(super) fn ssm_baseline_rules_lines(detail: Option<&Lazy<Box<SsmBaselineDetail>>>) -> Vec<(String, String)> {
    let mut rows = vec![("".to_string(), "".to_string())];
    match detail {
        None | Some(Lazy::Loading) => {
            rows.push(("  Loading rules…".to_string(), "".to_string()));
        }
        Some(Lazy::Error(e)) => {
            rows.extend(error_rows(e));
        }
        Some(Lazy::Loaded(d)) => {
            if d.approval_rules.is_empty() {
                rows.push(("  No approval rules".to_string(), "".to_string()));
                rows.push(("".to_string(), "".to_string()));
            } else {
                for (i, r) in d.approval_rules.iter().enumerate() {
                    rows.push((format!("Approval Rule {}", i + 1), "".to_string()));
                    for (k, v) in &r.filters {
                        rows.push((format!("  {}", k), v.clone()));
                    }
                    if let Some(days) = r.approve_after_days {
                        rows.push(("  Approve After".to_string(), format!("{} days", days)));
                    }
                    if let Some(until) = &r.approve_until_date {
                        rows.push(("  Approve Until".to_string(), until.clone()));
                    }
                    if let Some(c) = &r.compliance_level {
                        rows.push(("  Compliance Level".to_string(), c.clone()));
                    }
                    if r.include_non_security {
                        rows.push((
                            "  Non-Security".to_string(),
                            "included".to_string(),
                        ));
                    }
                    rows.push(("".to_string(), "".to_string()));
                }
            }
            if !d.global_filters.is_empty() {
                rows.push(("Global Filters".to_string(), "".to_string()));
                for (k, v) in &d.global_filters {
                    rows.push((format!("  {}", k), v.clone()));
                }
                rows.push(("".to_string(), "".to_string()));
            }
            if !d.approved_patches.is_empty() {
                let mut head = format!("Approved Patches ({})", d.approved_patches.len());
                if let Some(l) = &d.approved_compliance_level {
                    head = format!("{} · {}", head, l);
                }
                rows.push((head, "".to_string()));
                for p in &d.approved_patches {
                    rows.push((format!("  {}", p), "".to_string()));
                }
                rows.push(("".to_string(), "".to_string()));
            }
            if !d.rejected_patches.is_empty() {
                let mut head = format!("Rejected Patches ({})", d.rejected_patches.len());
                if let Some(a) = &d.rejected_action {
                    head = format!("{} · {}", head, a);
                }
                rows.push((head, "".to_string()));
                for p in &d.rejected_patches {
                    rows.push((format!("  {}", p), "".to_string()));
                }
                rows.push(("".to_string(), "".to_string()));
            }
            if !d.patch_groups.is_empty() {
                rows.push(("Patch Groups".to_string(), "".to_string()));
                for g in &d.patch_groups {
                    rows.push((format!("  {}", g), "".to_string()));
                }
                rows.push(("".to_string(), "".to_string()));
            }
            if !d.sources.is_empty() {
                rows.push(("Custom Sources".to_string(), "".to_string()));
                for s in &d.sources {
                    rows.push((format!("  {}", s), "".to_string()));
                }
                rows.push(("".to_string(), "".to_string()));
            }
            if !d.created.is_empty() || !d.modified.is_empty() {
                rows.push(("Dates".to_string(), "".to_string()));
                if !d.created.is_empty() {
                    rows.push(("  Created".to_string(), d.created.clone()));
                }
                if !d.modified.is_empty() {
                    rows.push(("  Modified".to_string(), d.modified.clone()));
                }
                rows.push(("".to_string(), "".to_string()));
            }
        }
    }
    rows
}

// ── SSM OpsItem split pane ────────────────────────────────────────────────

pub(super) fn render_ssm_ops_item_split(app: &App, item: &SsmOpsItem, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 2, "");

    let mut block = theme::pane_block("SSM OpsItem", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let (state_color, state_dot) = match item.status.to_ascii_lowercase().as_str() {
        "resolved" | "closed" | "completedwithsuccess" | "approved" => (theme::success(), "● "),
        "failed" | "timedout" | "completedwithfailure" | "rejected" | "cancelled" => {
            (theme::error(), "● ")
        }
        _ => (theme::warning(), "◌ "),
    };
    let mut status_spans = vec![
        Span::raw("  "),
        Span::styled(state_dot.to_string(), Style::default().fg(state_color)),
        Span::styled(
            item.status.clone(),
            Style::default().fg(state_color).add_modifier(Modifier::BOLD),
        ),
    ];
    if let Some(sev) = &item.severity {
        status_spans.push(Span::styled("  ·  ", Style::default().fg(theme::text_dim())));
        status_spans.push(Span::styled(
            format!("severity {}", sev),
            Style::default().fg(theme::aws_orange()),
        ));
    }
    if let Some(src) = &item.source {
        status_spans.push(Span::styled("  ·  ", Style::default().fg(theme::text_dim())));
        status_spans.push(Span::styled(src.clone(), Style::default().fg(theme::text_dim())));
    }
    let header_lines = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                item.title.clone(),
                Style::default()
                    .fg(crate::ui::theme::text_primary())
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(status_spans),
        Line::raw(""),
    ];
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
    let tabs = descriptor_tabs(app, &crate::aws::services::ssm::SSM_OPS_ITEM_SECTIONS);
    render_section_tab_bar(app, chunks[2], frame, &tabs);
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub fn ssm_ops_item_section_lines(
    item: &SsmOpsItem,
    section: SsmOpsItemDetailSection,
    detail: Option<&Lazy<Box<SsmOpsItemDetail>>>,
) -> Vec<(String, String)> {
    match section {
        SsmOpsItemDetailSection::Overview => ssm_ops_item_overview_lines(item),
        SsmOpsItemDetailSection::Detail => ssm_ops_item_detail_lines(detail),
    }
}

pub(super) fn ssm_ops_item_overview_lines(item: &SsmOpsItem) -> Vec<(String, String)> {
    let mut rows = vec![
        ("OpsItem ID".to_string(), item.ops_item_id.clone()),
        ("Title".to_string(), item.title.clone()),
        ("Status".to_string(), item.status.clone()),
    ];
    if let Some(s) = &item.severity {
        rows.push(("Severity".to_string(), s.clone()));
    }
    if let Some(p) = item.priority {
        rows.push(("Priority".to_string(), p.to_string()));
    }
    if let Some(s) = &item.source {
        rows.push(("Source".to_string(), s.clone()));
    }
    if let Some(c) = &item.category {
        rows.push(("Category".to_string(), c.clone()));
    }
    if let Some(t) = &item.ops_item_type {
        rows.push(("Type".to_string(), t.clone()));
    }
    rows.push(("".to_string(), "".to_string()));
    if let Some(c) = &item.created_by {
        rows.push(("Created By".to_string(), c.clone()));
    }
    if !item.created.is_empty() {
        rows.push(("Created".to_string(), item.created.clone()));
    }
    if !item.last_modified.is_empty() {
        rows.push(("Last Modified".to_string(), item.last_modified.clone()));
    }
    rows.push(("".to_string(), "".to_string()));
    rows
}

pub(super) fn ssm_ops_item_detail_lines(detail: Option<&Lazy<Box<SsmOpsItemDetail>>>) -> Vec<(String, String)> {
    let mut rows = vec![("".to_string(), "".to_string())];
    match detail {
        None | Some(Lazy::Loading) => {
            rows.push(("  Loading detail…".to_string(), "".to_string()));
        }
        Some(Lazy::Error(e)) => {
            rows.extend(error_rows(e));
        }
        Some(Lazy::Loaded(d)) => {
            if d.description.is_empty() {
                rows.push(("  No description".to_string(), "".to_string()));
            } else {
                rows.push(("Description".to_string(), "".to_string()));
                for line in d.description.lines() {
                    rows.push((format!("  {}", line), "".to_string()));
                }
            }
            rows.push(("".to_string(), "".to_string()));
            if !d.related_resources.is_empty() {
                rows.push(("Related Resources".to_string(), "".to_string()));
                for arn in &d.related_resources {
                    rows.push(("  Resource".to_string(), arn.clone()));
                }
                rows.push(("".to_string(), "".to_string()));
            }
            if !d.operational_data.is_empty() {
                rows.push(("Operational Data".to_string(), "".to_string()));
                for (k, v) in &d.operational_data {
                    rows.push((format!("  {}", k), v.clone()));
                }
                rows.push(("".to_string(), "".to_string()));
            }
            if !d.related_ops_items.is_empty() {
                rows.push(("Related OpsItems".to_string(), "".to_string()));
                for id in &d.related_ops_items {
                    rows.push(("  OpsItem".to_string(), id.clone()));
                }
                rows.push(("".to_string(), "".to_string()));
            }
            if !d.notifications.is_empty() {
                rows.push(("Notifications".to_string(), "".to_string()));
                for arn in &d.notifications {
                    rows.push(("  SNS Topic".to_string(), arn.clone()));
                }
                rows.push(("".to_string(), "".to_string()));
            }
        }
    }
    rows
}
