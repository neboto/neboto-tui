use super::*;

// ── Backup vault / plan split panes ──────────────────────────────────────────────

pub(super) fn backup_header(name: &str, subtitle: Vec<Span<'static>>) -> Vec<Line<'static>> {
    let mut second = vec![Span::raw("  ")];
    second.extend(subtitle);
    vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(name.to_string(), Style::default().fg(crate::ui::theme::text_primary()).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(second),
        Line::raw(""),
    ]
}

pub(super) fn render_backup_split(
    app: &App,
    title: &str,
    header: Vec<Line<'static>>,
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

pub(super) fn render_backup_vault_split(app: &App, vault: &BackupVault, area: Rect, frame: &mut Frame) {
    let mut subtitle = vec![Span::styled(
        format!("{} recovery points", vault.recovery_points),
        Style::default().fg(theme::text_dim()),
    )];
    if vault.locked {
        subtitle.push(Span::styled("  ·  ", Style::default().fg(theme::text_dim())));
        subtitle.push(Span::styled("🔒 locked", Style::default().fg(theme::warning())));
    }
    let header = backup_header(&vault.name, subtitle);
    render_backup_split(
        app,
        "Backup Vault",
        header,
        &descriptor_tabs(app, &crate::aws::services::backup::BACKUP_VAULT_SECTIONS),
        area,
        frame,
    );
}

pub(super) fn render_backup_plan_split(app: &App, plan: &BackupPlan, area: Rect, frame: &mut Frame) {
    let subtitle = vec![Span::styled(
        format!("last run {}", plan.last_execution.as_deref().unwrap_or("—")),
        Style::default().fg(theme::text_dim()),
    )];
    let header = backup_header(&plan.name, subtitle);
    render_backup_split(
        app,
        "Backup Plan",
        header,
        &descriptor_tabs(app, &crate::aws::services::backup::BACKUP_PLAN_SECTIONS),
        area,
        frame,
    );
}

pub fn backup_vault_section_lines(
    vault: &BackupVault,
    section: BackupVaultDetailSection,
    points: Option<&crate::lazy::Lazy<Vec<crate::aws::services::backup::RecoveryPoint>>>,
) -> Vec<(String, String)> {
    match section {
        BackupVaultDetailSection::Details => {
            let mut rows = vec![
                ("Vault".to_string(), vault.name.clone()),
                ("Recovery Points".to_string(), vault.recovery_points.to_string()),
                ("Locked (Vault Lock)".to_string(), if vault.locked { "yes".to_string() } else { "no".to_string() }),
            ];
            if !vault.encryption_key_arn.is_empty() {
                rows.push(("Encryption Key".to_string(), vault.encryption_key_arn.clone()));
            }
            if let Some(c) = &vault.created {
                rows.push(("Created".to_string(), c.clone()));
            }
            rows.push((String::new(), String::new()));
            rows.push(("ARN".to_string(), vault.arn.clone()));
            rows
        }
        BackupVaultDetailSection::RecoveryPoints => match points {
            None | Some(crate::lazy::Lazy::Loading) => {
                vec![("".to_string(), "Loading recovery points…".to_string())]
            }
            Some(crate::lazy::Lazy::Error(e)) => error_rows(e),
            Some(crate::lazy::Lazy::Loaded(list)) => {
                if list.is_empty() {
                    return vec![("".to_string(), "No recovery points".to_string())];
                }
                let mut rows = vec![(format!("Recovery Points ({})", list.len()), String::new())];
                rows.push((String::new(), String::new()));
                for rp in list {
                    let size = rp.size.map(crate::aws::services::backup::fmt_bytes).unwrap_or_else(|| "—".to_string());
                    rows.push((
                        format!("{} · {}", rp.resource_type, rp.status),
                        String::new(),
                    ));
                    rows.push(("  Created".to_string(), rp.created.clone().unwrap_or_else(|| "—".to_string())));
                    rows.push(("  Size".to_string(), size));
                    rows.push(("  ARN".to_string(), rp.arn.clone()));
                    rows.push((String::new(), String::new()));
                }
                rows
            }
        },
        BackupVaultDetailSection::Tags => tag_rows(&vault.tags),
    }
}

pub fn backup_plan_section_lines(
    plan: &BackupPlan,
    section: BackupPlanDetailSection,
    details: Option<&crate::lazy::Lazy<crate::aws::services::backup::BackupPlanDetails>>,
) -> Vec<(String, String)> {
    match section {
        BackupPlanDetailSection::Rules => match details {
            None | Some(crate::lazy::Lazy::Loading) => {
                vec![("".to_string(), "Loading rules…".to_string())]
            }
            Some(crate::lazy::Lazy::Error(e)) => error_rows(e),
            Some(crate::lazy::Lazy::Loaded(d)) => {
                if d.rules.is_empty() {
                    return vec![("".to_string(), "No rules".to_string())];
                }
                let mut rows = vec![(format!("Rules ({})", d.rules.len()), String::new())];
                rows.push((String::new(), String::new()));
                for r in &d.rules {
                    rows.push((r.name.clone(), String::new())); // group header
                    rows.push(("  Schedule".to_string(), r.schedule.clone()));
                    rows.push(("  Target Vault".to_string(), r.target_vault.clone()));
                    rows.push(("  Lifecycle".to_string(), r.lifecycle.clone()));
                    rows.push(("  Start Window".to_string(), r.start_window.clone()));
                    rows.push((String::new(), String::new()));
                }
                rows
            }
        },
        BackupPlanDetailSection::Selections => match details {
            None | Some(crate::lazy::Lazy::Loading) => {
                vec![("".to_string(), "Loading selections…".to_string())]
            }
            Some(crate::lazy::Lazy::Error(e)) => error_rows(e),
            Some(crate::lazy::Lazy::Loaded(d)) => {
                if d.selections.is_empty() {
                    return vec![("".to_string(), "No resource selections".to_string())];
                }
                let mut rows = vec![(format!("Selections ({})", d.selections.len()), String::new())];
                rows.push((String::new(), String::new()));
                for s in &d.selections {
                    rows.push((format!("  {}", s), String::new()));
                }
                rows
            }
        },
        BackupPlanDetailSection::Tags => tag_rows(&plan.tags),
    }
}
