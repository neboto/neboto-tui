use super::*;

// ── Trusted Advisor check split pane ───────────────────────────────────────────

pub(super) fn render_ta_check_split(app: &App, check: &TaCheck, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 2, "");

    let mut block = theme::pane_block("Trusted Advisor Check", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let header_lines = build_ta_check_header_lines(check);
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
    // Org-aggregated rows carry the reduced Summary/Accounts descriptor —
    // same per-instance switch as TaCheck::detail_sections().
    let descriptor: &'static crate::sections::SectionDescriptor = if check.org_scope {
        &crate::aws::services::trusted_advisor::TA_ORG_CHECK_SECTIONS
    } else {
        &crate::aws::services::trusted_advisor::TA_CHECK_SECTIONS
    };
    render_section_tab_bar(app, chunks[2], frame, &descriptor_tabs(app, descriptor));
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub(super) fn build_ta_check_header_lines(check: &TaCheck) -> Vec<Line<'static>> {
    let mut lines = vec![Line::raw("")];
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            check.name.clone(),
            Style::default()
                .fg(crate::ui::theme::text_primary())
                .add_modifier(Modifier::BOLD),
        ),
    ]));

    let (status_label, status_color) = ta_status_display(&check.status);
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(check.category_label(), Style::default().fg(theme::text_dim())),
        Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
        Span::styled(
            status_label,
            Style::default()
                .fg(status_color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
        Span::styled(
            format!("{} flagged", check.resources_flagged),
            Style::default().fg(if check.resources_flagged > 0 {
                theme::warning()
            } else {
                theme::text_dim()
            }),
        ),
    ]));
    if check.org_scope {
        let affected = check
            .org_accounts
            .iter()
            .filter(|a| a.status == "error" || a.status == "warning")
            .count();
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(
                format!(
                    "Organization · {} account(s) · {} affected",
                    check.org_accounts.len(),
                    affected
                ),
                Style::default().fg(if affected > 0 {
                    theme::warning()
                } else {
                    theme::text_dim()
                }),
            ),
        ]));
    }
    lines.push(Line::raw(""));
    lines
}

/// (label, colour) for a Trusted Advisor status string.
pub(super) fn ta_status_display(status: &str) -> (String, Color) {
    match status {
        "ok" => ("OK".to_string(), theme::success()),
        "warning" => ("Warning".to_string(), theme::warning()),
        "error" => ("Action recommended".to_string(), theme::error()),
        "" => ("—".to_string(), theme::text_dim()),
        other => (other.to_string(), theme::text_dim()),
    }
}

pub fn ta_check_section_lines(
    check: &TaCheck,
    section: TaCheckDetailSection,
    result: Option<&Lazy<Vec<crate::aws::services::trusted_advisor::TaFlaggedResource>>>,
) -> Vec<(String, String)> {
    match section {
        TaCheckDetailSection::Summary => ta_summary_lines(check),
        TaCheckDetailSection::Resources => ta_resources_lines(check, result),
    }
}

pub(super) fn ta_summary_lines(check: &TaCheck) -> Vec<(String, String)> {
    let (status_label, _) = ta_status_display(&check.status);
    let mut rows = vec![
        ("Check".to_string(), check.name.clone()),
        ("Category".to_string(), check.category_label()),
        ("Status".to_string(), status_label),
        ("".to_string(), "".to_string()),
        (
            "Resources Flagged".to_string(),
            check.resources_flagged.to_string(),
        ),
        (
            "Resources Processed".to_string(),
            check.resources_processed.to_string(),
        ),
        (
            "Resources Suppressed".to_string(),
            check.resources_suppressed.to_string(),
        ),
        ("".to_string(), "".to_string()),
        ("Check ID".to_string(), check.id.clone()),
        ("".to_string(), "".to_string()),
    ];

    // Org scope: the counts above are sums across the organization.
    if check.org_scope {
        let affected = check
            .org_accounts
            .iter()
            .filter(|a| a.status == "error" || a.status == "warning")
            .count();
        rows.insert(3, ("Scope".to_string(), "Organization".to_string()));
        rows.insert(4, ("Accounts".to_string(), check.org_accounts.len().to_string()));
        rows.insert(5, ("Accounts Affected".to_string(), affected.to_string()));
    }

    if !check.description.is_empty() {
        rows.push(("Description".to_string(), "".to_string())); // group header
        // The description is already plain text with paragraph/list newlines
        // (html_to_text); wrap each segment but keep its blank-line breaks.
        for segment in check.description.split('\n') {
            if segment.trim().is_empty() {
                rows.push(("".to_string(), "".to_string())); // blank spacer
            } else {
                for line in wrap_plain(segment, 72) {
                    rows.push((format!(" {}", line), "".to_string()));
                }
            }
        }
    }
    rows
}

pub(super) fn ta_resources_lines(check: &TaCheck, state: Option<&Lazy<Vec<crate::aws::services::trusted_advisor::TaFlaggedResource>>>) -> Vec<(String, String)> {
    match state {
        None | Some(Lazy::Loading) => {
            vec![("".to_string(), "Loading flagged resources…".to_string())]
        }
        Some(Lazy::Error(e)) => error_rows(e),
        Some(Lazy::Loaded(resources)) => {
            if resources.is_empty() {
                return vec![("".to_string(), "No flagged resources".to_string())];
            }
            let cols = &check.metadata_cols;
            let mut rows: Vec<(String, String)> =
                vec![(format!("{} flagged resource(s)", resources.len()), String::new())];
            rows.push(("".to_string(), "".to_string()));

            for r in resources {
                // Per-resource subsection header: region + suppressed badge.
                let mut head = if r.region.is_empty() {
                    "resource".to_string()
                } else {
                    r.region.clone()
                };
                if r.is_suppressed {
                    head.push_str("  [suppressed]");
                }
                rows.push((head, "".to_string())); // group header (magenta)

                if !r.status.is_empty() {
                    rows.push(("Status".to_string(), r.status.clone()));
                }

                // Each metadata value labelled by its column header (positional).
                for (i, val) in r.metadata.iter().enumerate() {
                    if val.is_empty() {
                        continue;
                    }
                    let key = cols
                        .get(i)
                        .filter(|c| !c.is_empty())
                        .cloned()
                        .unwrap_or_else(|| format!("col {}", i + 1));
                    rows.push((key, val.clone()));
                }
                rows.push(("".to_string(), "".to_string()));
            }
            rows
        }
    }
}

pub fn ta_org_check_section_lines(
    check: &TaCheck,
    section: TaOrgCheckDetailSection,
) -> Vec<(String, String)> {
    match section {
        TaOrgCheckDetailSection::Summary => ta_summary_lines(check),
        TaOrgCheckDetailSection::Accounts => ta_accounts_lines(check),
    }
}

/// Per-account breakdown of an org-aggregated check. Affected accounts get a
/// full block; clean / not-evaluated accounts collapse to one count line each
/// so a 50-account org stays scannable.
pub(super) fn ta_accounts_lines(check: &TaCheck) -> Vec<(String, String)> {
    if check.org_accounts.is_empty() {
        return vec![(
            "".to_string(),
            "No account returned data for this check".to_string(),
        )];
    }
    let affected: Vec<_> = check
        .org_accounts
        .iter()
        .filter(|a| a.status == "error" || a.status == "warning")
        .collect();
    let ok = check.org_accounts.iter().filter(|a| a.status == "ok").count();
    let na = check.org_accounts.len() - affected.len() - ok;

    let mut rows = vec![
        ("Accounts".to_string(), check.org_accounts.len().to_string()),
        ("Affected".to_string(), affected.len().to_string()),
        ("".to_string(), "".to_string()),
    ];

    for a in &affected {
        rows.push((format!("{} ({})", a.account_name, a.account_id), "".to_string()));
        let (status_label, _) = ta_status_display(&a.status);
        rows.push(("Status".to_string(), status_label));
        rows.push(("Flagged".to_string(), a.flagged.to_string()));
        if a.suppressed > 0 {
            rows.push(("Suppressed".to_string(), a.suppressed.to_string()));
        }
        rows.push(("".to_string(), "".to_string()));
    }

    if affected.is_empty() {
        rows.push(("  · no account is flagged on this check".to_string(), "".to_string()));
    }
    if ok > 0 {
        rows.push((format!("  · {} account(s) OK", ok), "".to_string()));
    }
    if na > 0 {
        rows.push((format!("  · {} account(s) not evaluated", na), "".to_string()));
    }
    rows.push(("".to_string(), "".to_string()));
    rows.push((
        "  · flagged-resource detail is per account — assume into it (@orgs) and open this check"
            .to_string(),
        "".to_string(),
    ));
    rows
}

// ── Trusted Advisor Priority recommendations ─────────────────────────────────

pub(super) fn render_ta_rec_split(
    app: &App,
    rec: &crate::aws::services::trusted_advisor::TaRecommendation,
    area: Rect,
    frame: &mut Frame,
) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 3, "");

    let mut block = theme::pane_block("TA Recommendation", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let header_lines = build_ta_rec_header_lines(rec);
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
    render_section_tab_bar(
        app,
        chunks[2],
        frame,
        &descriptor_tabs(app, &crate::aws::services::trusted_advisor::TA_REC_SECTIONS),
    );
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub(super) fn build_ta_rec_header_lines(
    rec: &crate::aws::services::trusted_advisor::TaRecommendation,
) -> Vec<Line<'static>> {
    let mut lines = vec![Line::raw("")];
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            rec.name.clone(),
            Style::default()
                .fg(crate::ui::theme::text_primary())
                .add_modifier(Modifier::BOLD),
        ),
    ]));

    let (status_label, status_color) = ta_status_display(&rec.status);
    let lifecycle_color = if rec.active() {
        theme::warning()
    } else {
        theme::text_dim()
    };
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(rec.lifecycle_label(), Style::default().fg(lifecycle_color)),
        Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
        Span::styled(
            status_label,
            Style::default().fg(status_color).add_modifier(Modifier::BOLD),
        ),
        Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
        Span::styled(
            format!("{} error / {} warning", rec.error_count, rec.warning_count),
            Style::default().fg(if rec.error_count + rec.warning_count > 0 {
                theme::warning()
            } else {
                theme::text_dim()
            }),
        ),
        Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
        Span::styled(
            format!("{} account(s)", rec.affected_accounts.len()),
            Style::default().fg(theme::text_dim()),
        ),
    ]));
    lines.push(Line::raw(""));
    lines
}

pub fn ta_rec_section_lines(
    rec: &crate::aws::services::trusted_advisor::TaRecommendation,
    section: crate::aws::services::trusted_advisor::TaRecDetailSection,
    detail: Option<&Lazy<crate::aws::services::trusted_advisor::TaRecDetail>>,
    accounts: Option<&Lazy<Vec<crate::aws::services::trusted_advisor::TaRecAccount>>>,
    resources: Option<&Lazy<(Vec<crate::aws::services::trusted_advisor::TaRecResource>, bool)>>,
) -> Vec<(String, String)> {
    use crate::aws::services::trusted_advisor::TaRecDetailSection as S;
    match section {
        S::Overview => ta_rec_overview_lines(rec, detail),
        S::Accounts => ta_rec_accounts_lines(accounts),
        S::Resources => ta_rec_resources_lines(resources),
    }
}

pub(super) fn ta_rec_overview_lines(
    rec: &crate::aws::services::trusted_advisor::TaRecommendation,
    detail: Option<&Lazy<crate::aws::services::trusted_advisor::TaRecDetail>>,
) -> Vec<(String, String)> {
    let (status_label, _) = ta_status_display(&rec.status);
    let mut rows = vec![
        ("Recommendation".to_string(), rec.name.clone()),
        ("Status".to_string(), status_label),
        ("Lifecycle".to_string(), rec.lifecycle_label()),
        ("Type".to_string(), rec.rec_type.clone()),
        ("Source".to_string(), rec.source.replace('_', " ")),
        (
            "Pillars".to_string(),
            rec.pillars
                .iter()
                .map(|p| crate::aws::services::trusted_advisor::category_label(p))
                .collect::<Vec<_>>()
                .join(", "),
        ),
        ("Services".to_string(), rec.aws_services.join(", ")),
        (
            "Accounts Affected".to_string(),
            if rec.affected_accounts.is_empty() {
                "—".to_string()
            } else {
                rec.affected_accounts.join(", ")
            },
        ),
        ("".to_string(), "".to_string()),
        ("Resources".to_string(), "".to_string()), // group header
        ("Error".to_string(), rec.error_count.to_string()),
        ("Warning".to_string(), rec.warning_count.to_string()),
        ("OK".to_string(), rec.ok_count.to_string()),
        ("".to_string(), "".to_string()),
        ("Created".to_string(), rec.created_at.clone()),
        ("Last Updated".to_string(), rec.last_updated.clone()),
    ];

    match detail {
        None | Some(Lazy::Loading) => {
            rows.push(("".to_string(), "".to_string()));
            rows.push(("".to_string(), "Loading…".to_string()));
        }
        Some(Lazy::Error(e)) => rows.extend(error_rows(e)),
        Some(Lazy::Loaded(d)) => {
            if !d.resolved_at.is_empty() {
                rows.push(("Resolved".to_string(), d.resolved_at.clone()));
            }
            if !d.created_by.is_empty() {
                rows.push(("Created By".to_string(), d.created_by.clone()));
            }
            if !d.updated_on_behalf_of.is_empty() {
                rows.push((
                    "Updated On Behalf Of".to_string(),
                    d.updated_on_behalf_of.clone(),
                ));
            }
            if !d.update_reason_code.is_empty() {
                rows.push(("Update Reason".to_string(), d.update_reason_code.replace('_', " ")));
            }
            if !d.update_reason.is_empty() {
                for line in wrap_plain(&d.update_reason, 72) {
                    rows.push((format!(" {}", line), "".to_string()));
                }
            }
            if !d.description.is_empty() {
                rows.push(("".to_string(), "".to_string()));
                rows.push(("Description".to_string(), "".to_string())); // group header
                for segment in d.description.split('\n') {
                    if segment.trim().is_empty() {
                        rows.push(("".to_string(), "".to_string()));
                    } else {
                        for line in wrap_plain(segment, 72) {
                            rows.push((format!(" {}", line), "".to_string()));
                        }
                    }
                }
            }
        }
    }
    rows
}

pub(super) fn ta_rec_accounts_lines(
    state: Option<&Lazy<Vec<crate::aws::services::trusted_advisor::TaRecAccount>>>,
) -> Vec<(String, String)> {
    match state {
        None | Some(Lazy::Loading) => {
            vec![("".to_string(), "Loading affected accounts…".to_string())]
        }
        Some(Lazy::Error(e)) => error_rows(e),
        Some(Lazy::Loaded(accounts)) => {
            if accounts.is_empty() {
                return vec![("".to_string(), "No affected accounts".to_string())];
            }
            let mut rows: Vec<(String, String)> = vec![
                ("Affected Accounts".to_string(), accounts.len().to_string()),
                ("".to_string(), "".to_string()),
            ];
            for a in accounts {
                rows.push((a.account_id.clone(), "".to_string())); // group header
                let lifecycle = match a.lifecycle.as_str() {
                    "" => "—".to_string(),
                    other => other.replace('_', " "),
                };
                rows.push(("Lifecycle".to_string(), lifecycle));
                if !a.updated_on_behalf_of.is_empty() {
                    rows.push(("Updated On Behalf Of".to_string(), a.updated_on_behalf_of.clone()));
                }
                if !a.update_reason.is_empty() {
                    for line in wrap_plain(&a.update_reason, 68) {
                        rows.push((format!(" {}", line), "".to_string()));
                    }
                }
                if !a.last_updated.is_empty() {
                    rows.push(("Last Updated".to_string(), a.last_updated.clone()));
                }
                rows.push(("".to_string(), "".to_string()));
            }
            rows.push((
                "  · assume into an account (@orgs) for its own Trusted Advisor view".to_string(),
                "".to_string(),
            ));
            rows
        }
    }
}

pub(super) fn ta_rec_resources_lines(
    state: Option<
        &Lazy<(Vec<crate::aws::services::trusted_advisor::TaRecResource>, bool)>,
    >,
) -> Vec<(String, String)> {
    match state {
        None | Some(Lazy::Loading) => {
            vec![("".to_string(), "Loading affected resources…".to_string())]
        }
        Some(Lazy::Error(e)) => error_rows(e),
        Some(Lazy::Loaded((resources, truncated))) => {
            if resources.is_empty() {
                return vec![("".to_string(), "No affected resources".to_string())];
            }
            let mut rows: Vec<(String, String)> = vec![
                ("Affected Resources".to_string(), resources.len().to_string()),
                ("".to_string(), "".to_string()),
            ];
            for r in resources {
                // Group header: account · region — the "which account is this
                // in" dimension the console's account selector serves.
                let mut head = if r.account_id.is_empty() {
                    r.region.clone()
                } else {
                    format!("{}  ·  {}", r.account_id, r.region)
                };
                if r.excluded {
                    head.push_str("  [excluded]");
                }
                rows.push((head, "".to_string())); // group header
                rows.push(("Resource".to_string(), r.aws_resource_id.clone()));
                if !r.status.is_empty() {
                    rows.push(("Status".to_string(), r.status.clone()));
                }
                for (k, v) in &r.metadata {
                    rows.push((k.clone(), v.clone()));
                }
                rows.push(("".to_string(), "".to_string()));
            }
            if *truncated {
                rows.push((
                    format!(
                        "  · showing the first {} resources (list truncated)",
                        crate::aws::services::trusted_advisor::TA_REC_RESOURCES_CAP
                    ),
                    "".to_string(),
                ));
            }
            rows
        }
    }
}
