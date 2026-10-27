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
            finding.severity_display(),
            Style::default().fg(sev_color).add_modifier(Modifier::BOLD),
        ),
        Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
        Span::styled(finding.finding_type.clone(), Style::default().fg(theme::text_dim())),
    ];
    if let Some(epss) = finding.epss {
        sev_line.push(Span::styled(
            format!("  ·  EPSS {:.1}%", epss * 100.0),
            Style::default().fg(theme::text_dim()),
        ));
    }
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
    let header = one_row_header(app, header, area.width.saturating_sub(2) as usize);
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
        InspFindingDetailSection::Overview => {
            let mut rows = vec![
                ("Finding ARN".to_string(), finding.arn.clone()),
                ("Title".to_string(), finding.title.clone()),
                ("Type".to_string(), finding.finding_type.clone()),
            ];
            if !finding.cve.is_empty() {
                rows.push(("CVE".to_string(), finding.cve.clone()));
            }
            if !finding.related_vulnerabilities.is_empty() {
                rows.push(("Related".to_string(), finding.related_vulnerabilities.join(", ")));
            }
            rows.push((String::new(), String::new()));
            rows.push(("Risk".to_string(), String::new()));
            rows.push(("Severity".to_string(), finding.severity_display()));
            if finding.inspector_score.is_none() {
                // Without a CVSS there's no Inspector score; say where the
                // severity came from instead of printing a fake 0.0.
                rows.push((
                    String::new(),
                    "· not scored by Inspector yet (no CVSS) — see Vendor Severity".to_string(),
                ));
            }
            if !finding.vendor_severity.is_empty() {
                rows.push(("Vendor Severity".to_string(), finding.vendor_severity.clone()));
            }
            if let Some(e) = finding.epss_display() {
                rows.push(("EPSS".to_string(), e));
            }
            rows.push(("Fix Available".to_string(), finding.fix_available.clone()));
            rows.push(("Exploit Available".to_string(), finding.exploit_available.clone()));
            if let Some(x) = &finding.last_known_exploit {
                rows.push(("Last Known Exploit".to_string(), x.clone()));
            }
            rows.push(("Status".to_string(), finding.status.clone()));

            rows.push((String::new(), String::new()));
            rows.push(("Affected".to_string(), String::new()));
            rows.push(("Resource".to_string(), finding.resource_display()));
            if !finding.aws_account_id.is_empty() {
                rows.push(("Account".to_string(), finding.aws_account_id.clone()));
            }
            if !finding.resource_region.is_empty() {
                rows.push(("Region".to_string(), finding.resource_region.clone()));
            }
            const PKG_PREVIEW: usize = 8;
            for p in finding.packages.iter().take(PKG_PREVIEW) {
                rows.push(("Package".to_string(), p.clone()));
            }
            if finding.packages.len() > PKG_PREVIEW {
                rows.push((
                    String::new(),
                    format!("· {} more — 2 Vulnerability", finding.packages.len() - PKG_PREVIEW),
                ));
            }

            rows.push((String::new(), String::new()));
            rows.push(("Timeline".to_string(), String::new()));
            if let Some(f) = &finding.first_observed {
                rows.push(("First Observed".to_string(), f.clone()));
            }
            if let Some(l) = &finding.last_observed {
                rows.push(("Last Observed".to_string(), l.clone()));
            }
            if let Some(u) = &finding.updated_at {
                rows.push(("Updated".to_string(), u.clone()));
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
            if !finding.aws_account_id.is_empty() {
                rows.push(("Account".to_string(), finding.aws_account_id.clone()));
            }
            if !finding.resource_region.is_empty() {
                rows.push(("Region".to_string(), finding.resource_region.clone()));
            }
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
                rows.extend(finding.resource_rows.iter().cloned());
            }
            rows.push((String::new(), String::new()));
            rows.push(("Tags".to_string(), String::new()));
            rows.extend(tag_rows(&finding.tags));
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
