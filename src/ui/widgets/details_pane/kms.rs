use super::*;

// ── KMS Key split pane ─────────────────────────────────────────────────────────

pub(super) fn render_kms_key_split(app: &App, key: &KmsKey, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 4, "");

    let mut block = theme::pane_block("KMS Key", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let header_lines = build_kms_key_header_lines(key);
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
    render_kms_section_tabs(app, chunks[2], frame);
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub(super) fn build_kms_key_header_lines(key: &KmsKey) -> Vec<Line<'static>> {
    let mut lines = vec![Line::raw("")];

    let display_name = key
        .aliases
        .first()
        .cloned()
        .unwrap_or_else(|| key.key_id.clone());
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            display_name,
            Style::default()
                .fg(crate::ui::theme::text_primary())
                .add_modifier(Modifier::BOLD),
        ),
    ]));

    let state_color = match key.state.as_str() {
        "Enabled" => theme::success(),
        "Disabled" => theme::text_dim(),
        "PendingDeletion" => theme::error(),
        "PendingImport" => theme::warning(),
        _ => theme::text_dim(),
    };
    let manager_label = if key.manager == "AWS" {
        "AWS managed"
    } else {
        "Customer managed"
    };
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(key.key_id.clone(), Style::default().fg(theme::text_dim())),
        Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
        Span::styled(key.state.clone(), Style::default().fg(state_color)),
        Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
        Span::styled(manager_label.to_string(), Style::default().fg(theme::text_dim())),
    ]));

    lines.push(Line::raw(""));
    lines
}

pub(super) fn render_kms_section_tabs(app: &App, area: Rect, frame: &mut Frame) {
    render_section_tab_bar(
        app,
        area,
        frame,
        &descriptor_tabs(app, &crate::aws::services::kms::KMS_KEY_SECTIONS),
    );
}

pub fn kms_key_section_lines(
    key: &KmsKey,
    section: KmsKeyDetailSection,
    rotation: Option<&Lazy<Option<bool>>>,
    policy: Option<&Lazy<String>>,
    grants: Option<&Lazy<Vec<KmsGrant>>>,
    tags: Option<&Lazy<std::collections::HashMap<String, String>>>,
) -> Vec<(String, String)> {
    match section {
        KmsKeyDetailSection::Details => kms_details_lines(key, rotation),
        KmsKeyDetailSection::Policy => kms_policy_lines(policy),
        KmsKeyDetailSection::Grants => kms_grants_lines(grants),
        KmsKeyDetailSection::Tags => kms_tags_lines(tags),
    }
}

pub(super) fn kms_details_lines(
    key: &KmsKey,
    rotation: Option<&Lazy<Option<bool>>>,
) -> Vec<(String, String)> {
    let mut rows = vec![
        ("Key ID".to_string(), key.key_id.clone()),
        ("ARN".to_string(), key.arn.clone()),
        ("State".to_string(), key.state.clone()),
        ("".to_string(), "".to_string()),
        ("Key Usage".to_string(), key.key_usage.clone()),
        ("Key Spec".to_string(), key.key_spec.clone()),
        ("Origin".to_string(), key.origin.clone()),
        ("Key Manager".to_string(), key.manager.clone()),
        (
            "Multi-Region".to_string(),
            if key.multi_region { "Yes" } else { "No" }.to_string(),
        ),
        ("".to_string(), "".to_string()),
    ];

    if !key.description.is_empty() {
        rows.push(("Description".to_string(), key.description.clone()));
    }

    if !key.aliases.is_empty() {
        rows.push(("Aliases".to_string(), key.aliases.join(", ")));
    }

    let rotation_text = match rotation {
        None | Some(Lazy::Loading) => "…".to_string(),
        Some(Lazy::Loaded(Some(true))) => "Enabled".to_string(),
        Some(Lazy::Loaded(Some(false))) => "Disabled".to_string(),
        Some(Lazy::Loaded(None)) | Some(Lazy::Error(_)) => "n/a".to_string(),
    };
    rows.push(("Key Rotation".to_string(), rotation_text));

    if let Some(created) = &key.creation_date {
        rows.push(("Created".to_string(), created.clone()));
    }
    if let Some(deletion) = &key.deletion_date {
        rows.push(("Scheduled Deletion".to_string(), deletion.clone()));
    }
    rows.push(("".to_string(), "".to_string()));
    rows
}

pub(super) fn kms_policy_lines(state: Option<&Lazy<String>>) -> Vec<(String, String)> {
    match state {
        None | Some(Lazy::Loading) => {
            vec![("".to_string(), "Loading key policy…".to_string())]
        }
        Some(Lazy::Error(e)) => error_rows(e),
        Some(Lazy::Loaded(policy)) => {
            if policy.is_empty() {
                return vec![("".to_string(), "No policy".to_string())];
            }
            // Pretty-print if valid JSON
            let formatted = serde_json::from_str::<serde_json::Value>(policy)
                .ok()
                .and_then(|v| serde_json::to_string_pretty(&v).ok())
                .unwrap_or_else(|| policy.clone());

            formatted
                .lines()
                .map(|line| (format!(" {}", line), "".to_string()))
                .collect()
        }
    }
}

pub(super) fn kms_grants_lines(state: Option<&Lazy<Vec<KmsGrant>>>) -> Vec<(String, String)> {
    match state {
        None | Some(Lazy::Loading) => {
            vec![("".to_string(), "Loading grants…".to_string())]
        }
        Some(Lazy::Error(e)) => error_rows(e),
        Some(Lazy::Loaded(grants)) => {
            if grants.is_empty() {
                return vec![("".to_string(), "No grants".to_string())];
            }
            let mut rows = vec![];
            for g in grants {
                let name = if g.name.is_empty() {
                    "(unnamed)".to_string()
                } else {
                    g.name.clone()
                };
                rows.push((name, "".to_string()));
                rows.push(("Grantee".to_string(), g.grantee_principal.clone()));
                rows.push(("Operations".to_string(), g.operations.clone()));
                if !g.retiring_principal.is_empty() {
                    rows.push(("Retiring Principal".to_string(), g.retiring_principal.clone()));
                }
                rows.push(("".to_string(), "".to_string()));
            }
            rows
        }
    }
}

pub(super) fn kms_tags_lines(
    state: Option<&Lazy<std::collections::HashMap<String, String>>>,
) -> Vec<(String, String)> {
    match state {
        None | Some(Lazy::Loading) => {
            vec![("".to_string(), "Loading tags…".to_string())]
        }
        Some(Lazy::Error(e)) => error_rows(e),
        Some(Lazy::Loaded(tags)) => {
            if tags.is_empty() {
                return vec![("".to_string(), "No tags".to_string())];
            }
            let mut sorted: Vec<(&String, &String)> = tags.iter().collect();
            sorted.sort_by_key(|(k, _)| k.as_str());
            let mut rows: Vec<(String, String)> = sorted
                .into_iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            rows.push(("".to_string(), "".to_string()));
            rows
        }
    }
}
