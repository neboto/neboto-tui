use super::*;

// ── Secret split pane ─────────────────────────────────────────────────────

pub(super) fn render_secret_split(app: &App, secret: &SecretEntry, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 3, "x reveal · Y copy value");

    let mut block = theme::pane_block("Secret", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let header_lines = build_secret_header_lines(secret);
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
    render_secret_section_tabs(app, chunks[2], frame);
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub(super) fn build_secret_header_lines(secret: &SecretEntry) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = vec![Line::raw("")];

    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            secret.name.clone(),
            Style::default()
                .fg(crate::ui::theme::text_primary())
                .add_modifier(Modifier::BOLD),
        ),
    ]));

    lines.push(Line::raw(""));
    lines.push(header_kv(
        "Rotation",
        if secret.rotation_enabled { "Enabled" } else { "Disabled" },
    ));
    if !secret.last_changed.is_empty() {
        lines.push(header_kv("Last Changed", &secret.last_changed));
    }
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

pub(super) fn render_secret_section_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let tabs = descriptor_tabs(app, &crate::aws::services::config::SECRET_SECTIONS);
    render_section_tab_bar(app, area, frame, &tabs);
}

pub fn secret_section_lines(
    secret: &SecretEntry,
    section: SecretDetailSection,
) -> Vec<(String, String)> {
    match section {
        SecretDetailSection::Details => secret_details_lines(secret),
        SecretDetailSection::Rotation => secret_rotation_lines(secret),
        SecretDetailSection::Tags => secret_tags_lines(secret),
    }
}

pub(super) fn secret_details_lines(secret: &SecretEntry) -> Vec<(String, String)> {
    let mut rows = vec![];

    rows.push(("Name".to_string(), secret.name.clone()));
    if let Some(desc) = &secret.description {
        rows.push(("Description".to_string(), desc.clone()));
    }
    if !secret.arn.is_empty() {
        rows.push(("ARN".to_string(), secret.arn.clone()));
    }
    if let Some(svc) = &secret.owning_service {
        rows.push(("Managed By".to_string(), svc.clone()));
    }

    rows.push(("".to_string(), "".to_string()));
    let encryption = secret
        .kms_key_id
        .as_ref()
        .map(|k| format!("KMS ({})", k))
        .unwrap_or_else(|| "aws/secretsmanager (default)".to_string());
    rows.push(("Encryption".to_string(), encryption));

    rows.push(("".to_string(), "".to_string()));
    if !secret.created.is_empty() {
        rows.push(("Created".to_string(), secret.created.clone()));
    }
    if !secret.last_changed.is_empty() {
        rows.push(("Last Changed".to_string(), secret.last_changed.clone()));
    }
    if !secret.last_accessed.is_empty() {
        rows.push(("Last Accessed".to_string(), secret.last_accessed.clone()));
    }

    rows.push(("".to_string(), "".to_string()));
    rows.push(("Value".to_string(), "x reveal · Y copy".to_string()));

    rows.push(("".to_string(), "".to_string()));
    rows
}

pub(super) fn secret_rotation_lines(secret: &SecretEntry) -> Vec<(String, String)> {
    let mut rows = vec![];

    rows.push((
        "Rotation".to_string(),
        if secret.rotation_enabled { "Enabled".to_string() } else { "Disabled".to_string() },
    ));

    if secret.rotation_enabled {
        if let Some(days) = secret.rotation_after_days {
            rows.push(("Interval".to_string(), format!("every {} days", days)));
        }
        if let Some(sched) = &secret.rotation_schedule {
            rows.push(("Schedule".to_string(), sched.clone()));
        }
        if let Some(lambda) = &secret.rotation_lambda_arn {
            let name = lambda.rsplit(':').next().unwrap_or(lambda);
            rows.push(("Rotation Lambda".to_string(), name.to_string()));
        }
    }

    rows.push(("".to_string(), "".to_string()));
    if !secret.last_rotated.is_empty() {
        rows.push(("Last Rotated".to_string(), secret.last_rotated.clone()));
    } else {
        rows.push(("Last Rotated".to_string(), "never".to_string()));
    }
    if !secret.next_rotation.is_empty() {
        rows.push(("Next Rotation".to_string(), secret.next_rotation.clone()));
    }

    rows.push(("".to_string(), "".to_string()));
    rows
}

pub(super) fn secret_tags_lines(secret: &SecretEntry) -> Vec<(String, String)> {
    let mut rows = vec![("".to_string(), "".to_string())];

    if secret.tags.is_empty() {
        rows.push(("  No tags".to_string(), "".to_string()));
    } else {
        let mut sorted: Vec<(&String, &String)> = secret.tags.iter().collect();
        sorted.sort_by_key(|(k, _)| k.as_str());
        for (key, value) in sorted {
            rows.push((format!("  {}", key), value.clone()));
        }
    }

    rows.push(("".to_string(), "".to_string()));
    rows
}
