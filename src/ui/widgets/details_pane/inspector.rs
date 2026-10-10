use super::*;

// ── Inspector finding split pane ─────────────────────────────────────────────────

pub(super) fn render_insp_finding_split(app: &App, finding: &InspFinding, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 4, "");
    let mut block = theme::pane_block("Inspector Finding", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let sev_color = severity_color(&finding.severity_label);
    // The do-now signal: exploitable + fix available.
    let actionable = finding.is_actionable();
    let mut sev_line = vec![
        Span::raw("  "),
        Span::styled("● ", Style::default().fg(sev_color)),
        Span::styled(
            format!("{} ({:.1})", finding.severity_label, finding.inspector_score),
            Style::default().fg(sev_color).add_modifier(Modifier::BOLD),
        ),
        Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
        Span::styled(finding.finding_type.clone(), Style::default().fg(theme::text_dim())),
    ];
    if actionable {
        sev_line.push(Span::styled(
            "  ·  ⚡ exploit + fix",
            Style::default().fg(theme::error()).add_modifier(Modifier::BOLD),
        ));
    }
    let header = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                finding.name().to_string(),
                Style::default().fg(crate::ui::theme::text_primary()).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(sev_line),
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
    render_section_tab_bar(app, 
        chunks[2],
        frame,
        &descriptor_tabs(app, &crate::aws::services::inspector::INSP_FINDING_SECTIONS),
    );
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub fn insp_finding_section_lines(
    finding: &InspFinding,
    section: InspFindingDetailSection,
) -> Vec<(String, String)> {
    match section {
        InspFindingDetailSection::Details => {
            let mut rows = vec![
                ("Title".to_string(), finding.title.clone()),
                ("Type".to_string(), finding.finding_type.clone()),
                (
                    "Severity".to_string(),
                    format!("{} ({:.1})", finding.severity_label, finding.inspector_score),
                ),
                ("Status".to_string(), finding.status.clone()),
                ("Fix Available".to_string(), finding.fix_available.clone()),
                ("Exploit Available".to_string(), finding.exploit_available.clone()),
            ];
            if let Some(f) = &finding.first_observed {
                rows.push(("First Observed".to_string(), f.clone()));
            }
            if let Some(l) = &finding.last_observed {
                rows.push(("Last Observed".to_string(), l.clone()));
            }
            if !finding.description.is_empty() {
                rows.push((String::new(), String::new()));
                rows.push(("Description".to_string(), String::new())); // group header
                for line in wrap_plain(&finding.description, 64) {
                    rows.push((format!("  {}", line), String::new()));
                }
            }
            rows
        }
        InspFindingDetailSection::Vulnerability => {
            if finding.vuln_rows.is_empty() {
                return vec![(" No vulnerability detail on this finding".to_string(), String::new())];
            }
            finding.vuln_rows.clone()
        }
        InspFindingDetailSection::Resource => {
            let mut rows = vec![("Resource Type".to_string(), finding.resource_type.clone())];
            rows.push((String::new(), String::new()));
            if finding.resource_id.is_empty() {
                rows.push((" No affected resource".to_string(), String::new()));
            } else {
                // Container images: lead with the human-readable repo/tags; keep
                // the digest ARN as the (jumpable) Resource ID row below.
                if !finding.repository.is_empty() {
                    rows.push(("Repository".to_string(), finding.repository.clone()));
                }
                if !finding.image_tags.is_empty() {
                    rows.push(("Image Tags".to_string(), finding.image_tags.join(", ")));
                }
                // ARN/id row is a cross-service jump anchor (EC2 instance / Lambda / …).
                rows.push(("Resource ID".to_string(), finding.resource_id.clone()));
            }
            rows
        }
        InspFindingDetailSection::Remediation => {
            let mut rows = Vec::new();
            rows.push(("Fix Available".to_string(), finding.fix_available.clone()));
            if finding.remediation_text.is_empty() && finding.remediation_url.is_empty() {
                rows.push((String::new(), String::new()));
                rows.push((" No remediation guidance on this finding".to_string(), String::new()));
                return rows;
            }
            rows.push((String::new(), String::new()));
            rows.push(("Recommendation".to_string(), String::new())); // group header
            if !finding.remediation_text.is_empty() {
                for line in wrap_plain(&finding.remediation_text, 64) {
                    rows.push((format!("  {}", line), String::new()));
                }
            }
            if !finding.remediation_url.is_empty() {
                rows.push((String::new(), String::new()));
                rows.push(("URL".to_string(), finding.remediation_url.clone()));
            }
            rows
        }
    }
}
