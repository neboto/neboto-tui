use super::*;

// ── Security Hub overview / insight / finding split panes ────────────────────

/// The Overview pane: a score header (bars, the one place a chart earns its
/// space) over the standard section machinery. It used to be a bespoke
/// full-body renderer that ignored the descriptor system entirely, which
/// meant no export, no flat view and no bookmark restore for this pane.
pub(super) fn render_sh_overview_split(app: &App, overview: &ShOverview, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(
        app,
        focused,
        crate::aws::services::security_hub::SH_OVERVIEW_SECTIONS.len(),
        "",
    );
    let mut block = theme::pane_block("Security Hub Overview", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let overall = overview.overall_score();
    let score_color = score_color_for(overall);
    let bar_width = 20u8;
    let filled = ((overall as f64 / 100.0) * bar_width as f64).round() as u8;
    let bar: String = format!(
        "{}{}",
        "█".repeat(filled as usize),
        "░".repeat((bar_width - filled) as usize)
    );

    let mut second = vec![
        Span::raw("  "),
        Span::styled(
            "Security Score: ",
            Style::default()
                .fg(crate::ui::theme::text_primary())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{}%", overall),
            Style::default().fg(score_color).add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(bar, Style::default().fg(score_color)),
    ];
    if overview.controls_total > 0 {
        second.push(Span::styled("   ", Style::default()));
        second.push(Span::styled(
            format!(
                "{} of {} controls failing",
                overview.controls_failed, overview.controls_total
            ),
            Style::default().fg(if overview.controls_failed > 0 {
                theme::error()
            } else {
                theme::text_dim()
            }),
        ));
    }

    let header = vec![Line::raw(""), Line::from(second), Line::raw("")];
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
        &descriptor_tabs(
            app,
            &crate::aws::services::security_hub::SH_OVERVIEW_SECTIONS,
        ),
    );
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub(super) fn score_color_for(pct: u8) -> Color {
    if pct >= 80 {
        theme::success()
    } else if pct >= 50 {
        theme::warning()
    } else {
        theme::error()
    }
}

/// A fixed-width text bar for a percentage, rendered as a plain content line.
pub(super) fn score_bar(pct: u8, width: usize) -> String {
    let filled = ((pct as f64 / 100.0) * width as f64).round() as usize;
    format!(
        "{}{}",
        "█".repeat(filled.min(width)),
        "░".repeat(width.saturating_sub(filled))
    )
}

pub fn sh_overview_section_lines(
    o: &ShOverview,
    section: crate::aws::services::security_hub::ShOverviewDetailSection,
    siblings: &[Box<dyn crate::aws::resource::Resource>],
) -> Vec<(String, String)> {
    use crate::aws::services::security_hub::{
        ShConfigPolicy, ShControl, ShMember, ShOverviewDetailSection as S, ShProduct, ShSettings,
        FAILED,
    };
    match section {
        S::Score => {
            let mut rows: Vec<(String, String)> = Vec::new();
            if o.standards.is_empty() {
                rows.push((
                    " No standards enabled — nothing is being scored".to_string(),
                    String::new(),
                ));
                return rows;
            }
            rows.push(("Per Standard".to_string(), String::new()));
            rows.push((String::new(), String::new()));
            for s in &o.standards {
                let pct = s.score_pct();
                // Plain content line: a fixed-width table needs its own
                // spacing, not the key-value column.
                rows.push((
                    format!(
                        "  {:<44} {:>3}%  {}  {}/{}",
                        truncate_middle(&s.name, 44),
                        pct,
                        score_bar(pct, 16),
                        s.controls_passed,
                        s.controls_enabled
                    ),
                    String::new(),
                ));
            }
            rows.push((String::new(), String::new()));
            rows.push(("Overall".to_string(), format!("{}%", o.overall_score())));
            if !o.scan_complete {
                rows.push((
                    "  · a floor, not a measurement — the failed-control scan hit its page cap"
                        .to_string(),
                    String::new(),
                ));
            }
            rows
        }
        S::Findings => {
            let mut rows: Vec<(String, String)> = Vec::new();
            rows.push(("By Severity".to_string(), String::new()));
            rows.push((String::new(), String::new()));
            let sev: [(&str, usize); 5] = [
                ("CRITICAL", o.critical_count),
                ("HIGH", o.high_count),
                ("MEDIUM", o.medium_count),
                ("LOW", o.low_count),
                ("INFORMATIONAL", o.informational_count),
            ];
            for (label, count) in sev {
                if count == 0 && !matches!(label, "CRITICAL" | "HIGH") {
                    continue;
                }
                rows.push((
                    format!("  {:<15}{:>5}  {}", label, count, "▮".repeat(count.min(30))),
                    String::new(),
                ));
            }
            rows.push((String::new(), String::new()));
            rows.push(("Loaded".to_string(), o.total_findings.to_string()));
            if o.suppressed_count > 0 {
                rows.push((
                    "Suppressed".to_string(),
                    format!("{} (hidden by `a`)", o.suppressed_count),
                ));
            }

            // This breakdown counts what was fetched, not what exists. Both
            // bounds have to be visible or it reads as an account-wide total.
            rows.push((String::new(), String::new()));
            rows.push(("Scope".to_string(), String::new()));
            rows.push((
                format!("  · severity scope {} — `t` widens it", o.severity_scope),
                String::new(),
            ));
            if o.findings_capped {
                rows.push((
                    "  ⚠ the fetch cap was hit — counts are a floor, not a total".to_string(),
                    String::new(),
                ));
            }
            rows
        }
        S::Controls => {
            let failing: Vec<&ShControl> = {
                let mut v: Vec<&ShControl> = siblings
                    .iter()
                    .filter_map(|r| r.as_any().downcast_ref::<ShControl>())
                    .filter(|c| c.compliance == FAILED)
                    .collect();
                v.sort_by_key(|c| std::cmp::Reverse(c.failed_resources));
                v
            };
            if failing.is_empty() {
                return vec![(
                    " Nothing is failing a control — or the Controls tab hasn't loaded yet"
                        .to_string(),
                    String::new(),
                )];
            }
            // Sibling filter (the Vpc-Subnets pattern): zero fetch, and the
            // control scan is a wider sample than the findings list.
            let shown = failing.len().min(15);
            let mut rows = vec![(
                format!("Worst Controls ({} failing)", failing.len()),
                String::new(),
            )];
            rows.push((String::new(), String::new()));
            for c in failing.iter().take(shown) {
                rows.push((
                    format!(
                        "  {:<10} {:<9} {:>5}  {}",
                        c.id,
                        c.severity,
                        c.failed_resources,
                        truncate_middle(&c.title, 40)
                    ),
                    String::new(),
                ));
            }
            if failing.len() > shown {
                rows.push((
                    format!("  · {} more — 3 opens the Controls tab", failing.len() - shown),
                    String::new(),
                ));
            }
            rows
        }
        S::Coverage => {
            let mut rows: Vec<(String, String)> = Vec::new();

            // Where findings come from, and whether the whole account is
            // reporting — all read off siblings, no extra calls.
            let settings = siblings
                .iter()
                .find_map(|r| r.as_any().downcast_ref::<ShSettings>());
            if let Some(s) = settings {
                rows.push(("Regions".to_string(), String::new()));
                if s.aggregation_configured {
                    rows.push(("  Aggregating Into".to_string(), s.aggregation_region.clone()));
                    rows.push((
                        "  Linked".to_string(),
                        format!("{} region(s)", s.linked_regions.len()),
                    ));
                } else {
                    rows.push((
                        "  ⚠ no cross-region aggregation — this is one region only".to_string(),
                        String::new(),
                    ));
                }
                rows.push((String::new(), String::new()));
            }

            let products: Vec<&ShProduct> = siblings
                .iter()
                .filter_map(|r| r.as_any().downcast_ref::<ShProduct>())
                .collect();
            if !products.is_empty() {
                let on = products.iter().filter(|p| p.enabled).count();
                rows.push(("Integrations".to_string(), String::new()));
                rows.push((
                    "  Subscribed".to_string(),
                    format!("{} of {} available", on, products.len()),
                ));
                for p in products.iter().filter(|p| p.enabled).take(10) {
                    rows.push((format!("  {}", p.name), String::new()));
                }
                rows.push((String::new(), String::new()));
            }

            let members: Vec<&ShMember> = siblings
                .iter()
                .filter_map(|r| r.as_any().downcast_ref::<ShMember>())
                .collect();
            if !members.is_empty() {
                let healthy = members.iter().filter(|m| m.is_noise()).count();
                rows.push(("Accounts".to_string(), String::new()));
                rows.push((
                    "  Reporting".to_string(),
                    format!("{} of {} members", healthy, members.len()),
                ));
                if healthy < members.len() {
                    rows.push((
                        format!(
                            "  ⚠ {} member(s) not reporting — 9 opens the Accounts tab",
                            members.len() - healthy
                        ),
                        String::new(),
                    ));
                }
                rows.push((String::new(), String::new()));
            }

            let policies: Vec<&ShConfigPolicy> = siblings
                .iter()
                .filter_map(|r| r.as_any().downcast_ref::<ShConfigPolicy>())
                .collect();
            if !policies.is_empty() {
                let applied = policies.iter().filter(|p| !p.targets.is_empty()).count();
                let broken: usize = policies.iter().map(|p| p.failed_targets()).sum();
                rows.push(("Central Configuration".to_string(), String::new()));
                rows.push((
                    "  Policies".to_string(),
                    format!("{} applied of {}", applied, policies.len()),
                ));
                if broken > 0 {
                    rows.push((
                        format!("  ⚠ {} target association(s) failed", broken),
                        String::new(),
                    ));
                }
            }

            if rows.is_empty() {
                rows.push((
                    " Coverage data hasn't streamed yet".to_string(),
                    String::new(),
                ));
            }
            rows
        }
    }
}

pub fn sh_insight_section_lines(
    i: &crate::aws::services::security_hub::ShInsight,
    section: crate::aws::services::security_hub::ShInsightDetailSection,
    results: Option<&Lazy<crate::aws::services::security_hub::ShInsightResults>>,
) -> Vec<(String, String)> {
    use crate::aws::services::security_hub::ShInsightDetailSection as S;
    match section {
        // Results is section 0 on purpose: running the insight is what an
        // insight *is*, so it loads on drill-in rather than sitting behind a
        // near-empty Overview. The metadata that used to be its own section
        // leads here instead.
        S::Results => {
            let mut rows: Vec<(String, String)> = vec![
                ("Group By".to_string(), i.group_by.clone()),
                (
                    "Origin".to_string(),
                    if i.managed {
                        "AWS-managed".to_string()
                    } else {
                        "custom".to_string()
                    },
                ),
                (
                    "Filters".to_string(),
                    if i.filter_rows.is_empty() {
                        "none — groups every finding".to_string()
                    } else {
                        format!("{} condition(s) — 2 to view", i.filter_rows.len())
                    },
                ),
                ("ARN".to_string(), i.arn.clone()),
                (String::new(), String::new()),
            ];

            match results {
                None | Some(Lazy::Loading) => {
                    rows.push(("Loading…".to_string(), String::new()))
                }
                Some(Lazy::Error(e)) => rows.extend(error_rows(e)),
                Some(Lazy::Loaded(r)) if r.values.is_empty() => rows.push((
                    "No findings match this insight".to_string(),
                    String::new(),
                )),
                Some(Lazy::Loaded(r)) => {
                    let total: i64 = r.values.iter().map(|(_, c)| *c as i64).sum();
                    let group = if r.group_by.is_empty() {
                        i.group_by.clone()
                    } else {
                        r.group_by.clone()
                    };
                    rows.push((
                        format!("{} groups · {} findings", r.values.len(), total),
                        String::new(),
                    ));
                    rows.push((String::new(), String::new()));

                    let max = r.values.first().map(|(_, c)| *c).unwrap_or(1).max(1);
                    for (value, count) in &r.values {
                        let pct = if total > 0 {
                            (*count as f64 / total as f64) * 100.0
                        } else {
                            0.0
                        };
                        // Plain content line: the count/bar/share table needs
                        // its own alignment, not the key column.
                        rows.push((
                            format!(
                                "  {:>6}  {}  {:>5.1}%",
                                count,
                                score_bar(((*count as f64 / max as f64) * 100.0) as u8, 18),
                                pct
                            ),
                            String::new(),
                        ));
                        // The group value on its own key-value row so the
                        // generic classifier can make an ARN or resource id
                        // an Enter-jump to the resource itself.
                        rows.push((
                            format!("  {}", group_label(&group)),
                            value.clone(),
                        ));
                        rows.push((String::new(), String::new()));
                    }
                    if r.truncated {
                        rows.push((
                            "  · showing the largest groups only".to_string(),
                            String::new(),
                        ));
                    }
                }
            }
            rows
        }
        S::Filters => {
            if i.filter_rows.is_empty() {
                return vec![(
                    " No filters — this insight groups every finding".to_string(),
                    String::new(),
                )];
            }
            let mut rows = vec![("Matches when".to_string(), String::new())];
            rows.push((String::new(), String::new()));
            rows.extend(i.filter_rows.iter().cloned());
            rows
        }
    }
}

/// A readable row label for an insight's group-by attribute — the raw values
/// are ASFF field paths (`ResourceId`, `SeverityLabel`, `ProductArn`).
pub(super) fn group_label(attr: &str) -> String {
    match attr {
        "ResourceId" => "Resource".to_string(),
        "ResourceType" => "Resource Type".to_string(),
        "AwsAccountId" => "Account".to_string(),
        "SeverityLabel" => "Severity".to_string(),
        "ProductArn" | "ProductName" => "Product".to_string(),
        "ComplianceStatus" => "Compliance".to_string(),
        "WorkflowStatus" => "Workflow".to_string(),
        "RecordState" => "Record State".to_string(),
        "ResourceAwsIamAccessKeyUserName" => "IAM User".to_string(),
        "Type" | "Types" => "Finding Type".to_string(),
        "Title" => "Finding".to_string(),
        "ResourceRegion" => "Region".to_string(),
        other => other.to_string(),
    }
}

/// Shorten to `width`, keeping both ends — a truncated standard or control
/// title is far more recognisable with its tail intact.
pub(super) fn truncate_middle(s: &str, width: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= width {
        return format!("{:<width$}", s, width = width);
    }
    if width <= 3 {
        return chars.iter().take(width).collect();
    }
    let keep = width - 1;
    let head = keep.div_ceil(2);
    let tail = keep - head;
    let mut out: String = chars[..head].iter().collect();
    out.push('…');
    out.extend(&chars[chars.len() - tail..]);
    out
}

pub(super) fn render_sh_finding_split(app: &App, finding: &ShFinding, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(
        app,
        focused,
        crate::aws::services::security_hub::SH_FINDING_SECTIONS.len(),
        "",
    );
    let mut block = theme::pane_block("Security Hub Finding", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let sev_color = severity_color(&finding.severity_label);
    let header = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                finding.name().to_string(),
                Style::default().fg(crate::ui::theme::text_primary()).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("● ", Style::default().fg(sev_color)),
            Span::styled(
                finding.severity_label.clone(),
                Style::default().fg(sev_color).add_modifier(Modifier::BOLD),
            ),
            Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
            Span::styled(finding.product.clone(), Style::default().fg(theme::accent())),
            Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
            Span::styled(finding.workflow_status.clone(), Style::default().fg(theme::text_dim())),
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
    render_section_tab_bar(app, 
        chunks[2],
        frame,
        &descriptor_tabs(app, &crate::aws::services::security_hub::SH_FINDING_SECTIONS),
    );
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub fn sh_finding_section_lines(
    finding: &ShFinding,
    section: ShFindingDetailSection,
    history: Option<&Lazy<Vec<crate::aws::services::security_hub::ShHistoryRecord>>>,
) -> Vec<(String, String)> {
    match section {
        ShFindingDetailSection::Details => sh_finding_details_lines(finding),
        ShFindingDetailSection::Resources => sh_finding_resources_lines(finding),
        ShFindingDetailSection::Compliance => sh_finding_compliance_lines(finding),
        ShFindingDetailSection::Vulnerabilities => {
            if finding.vuln_rows.is_empty() {
                return vec![(
                    " No vulnerability data — this isn't a CVE finding".to_string(),
                    String::new(),
                )];
            }
            finding.vuln_rows.clone()
        }
        ShFindingDetailSection::Context => {
            if finding.context_rows.is_empty() {
                return vec![(
                    " No network, process or threat context on this finding".to_string(),
                    String::new(),
                )];
            }
            finding.context_rows.clone()
        }
        ShFindingDetailSection::Remediation => sh_finding_remediation_lines(finding),
        ShFindingDetailSection::History => sh_finding_history_lines(finding, history),
    }
}

pub(super) fn sh_finding_details_lines(finding: &ShFinding) -> Vec<(String, String)> {
    let mut rows = vec![
        ("Title".to_string(), finding.title.clone()),
        ("Product".to_string(), finding.product.clone()),
        ("Severity".to_string(), finding.severity_label.clone()),
    ];
    if let Some(n) = finding.severity_normalized {
        rows.push(("Severity (0–100)".to_string(), n.to_string()));
    }
    if !finding.severity_original.is_empty() && finding.severity_original != finding.severity_label
    {
        rows.push((
            "Severity (product)".to_string(),
            finding.severity_original.clone(),
        ));
    }
    rows.push(("Workflow".to_string(), finding.workflow_status.clone()));
    rows.push(("Record State".to_string(), finding.record_state.clone()));
    if finding.account_name.is_empty() {
        rows.push(("Account".to_string(), finding.account_id.clone()));
    } else {
        rows.push((
            "Account".to_string(),
            format!("{} ({})", finding.account_id, finding.account_name),
        ));
    }
    rows.push(("Region".to_string(), finding.region.clone()));
    if let Some(c) = finding.criticality {
        rows.push(("Criticality".to_string(), c.to_string()));
    }
    if let Some(c) = finding.confidence {
        rows.push(("Confidence".to_string(), format!("{}%", c)));
    }
    if finding.sample {
        // ⚠-prefixed content line: a sample finding is generated, not real.
        rows.push((
            "  ⚠ sample finding — generated for demonstration".to_string(),
            String::new(),
        ));
    }

    // ── Timeline ──
    let times: Vec<(&str, &Option<String>)> = vec![
        ("First Observed", &finding.first_observed),
        ("Last Observed", &finding.last_observed),
        ("Created", &finding.created),
        ("Updated", &finding.updated),
        ("Processed", &finding.processed_at),
    ];
    let any_time = times.iter().any(|(_, v)| v.is_some());
    if any_time {
        rows.push((String::new(), String::new()));
        rows.push(("Timeline".to_string(), String::new()));
        for (label, value) in times {
            if let Some(v) = value {
                rows.push((format!("  {}", label), v.clone()));
            }
        }
    }

    // ── Type taxonomy ──
    // The ASFF type is a structured identifier (Namespace/Category/…), not a
    // name — decomposing it is how it becomes readable.
    if !finding.types.is_empty() {
        rows.push((String::new(), String::new()));
        rows.push(("Types".to_string(), String::new()));
        for t in &finding.types {
            let parts = crate::aws::services::security_hub::decompose_finding_type(t);
            if parts.is_empty() {
                rows.push((format!("  {}", t), String::new()));
            } else {
                rows.extend(parts);
                rows.push((String::new(), String::new()));
            }
        }
        while rows.last().map(|(k, v)| k.is_empty() && v.is_empty()) == Some(true) {
            rows.pop();
        }
    }

    if !finding.description.is_empty() {
        rows.push((String::new(), String::new()));
        rows.push(("Description".to_string(), String::new())); // group header
        for line in wrap_plain(&finding.description, 64) {
            rows.push((format!("  {}", line), String::new()));
        }
    }

    // ── Analyst note ──
    if !finding.note_text.is_empty() {
        rows.push((String::new(), String::new()));
        rows.push(("Note".to_string(), String::new()));
        for line in wrap_plain(&finding.note_text, 64) {
            rows.push((format!("  {}", line), String::new()));
        }
        if !finding.note_updated_by.is_empty() {
            rows.push(("  By".to_string(), finding.note_updated_by.clone()));
        }
        if !finding.note_updated_at.is_empty() {
            rows.push(("  At".to_string(), finding.note_updated_at.clone()));
        }
    }

    // ── Source ──
    rows.push((String::new(), String::new()));
    rows.push(("Source".to_string(), String::new()));
    if !finding.company.is_empty() {
        rows.push(("  Company".to_string(), finding.company.clone()));
    }
    if !finding.generator_id.is_empty() {
        rows.push(("  Generator".to_string(), finding.generator_id.clone()));
    }
    if !finding.product_arn.is_empty() {
        rows.push(("  Product ARN".to_string(), finding.product_arn.clone()));
    }
    if !finding.source_url.is_empty() {
        rows.push(("  Source URL".to_string(), finding.source_url.clone()));
    }
    rows.push(("  Finding ID".to_string(), finding.id.clone()));

    if !finding.related_findings.is_empty() {
        rows.push((String::new(), String::new()));
        rows.push(("Related Findings".to_string(), String::new()));
        for id in finding.related_findings.iter().take(20) {
            rows.push(("  Finding".to_string(), id.clone()));
        }
    }

    if !finding.user_defined_fields.is_empty() {
        rows.push((String::new(), String::new()));
        rows.push(("User-Defined Fields".to_string(), String::new()));
        for (k, v) in &finding.user_defined_fields {
            rows.push((format!("  {}", k), v.clone()));
        }
    }

    if !finding.product_fields.is_empty() {
        rows.push((String::new(), String::new()));
        rows.push(("Product Fields".to_string(), String::new()));
        for (k, v) in finding.product_fields.iter().take(40) {
            rows.push((format!("  {}", k), v.clone()));
        }
        if finding.product_fields.len() > 40 {
            rows.push((
                format!("  · {} more — `e` opens the full JSON", finding.product_fields.len() - 40),
                String::new(),
            ));
        }
    }

    rows
}

pub(super) fn sh_finding_resources_lines(finding: &ShFinding) -> Vec<(String, String)> {
    if finding.resource_rows.is_empty() {
        return vec![(
            " No affected resources on this finding".to_string(),
            String::new(),
        )];
    }
    let mut rows = vec![(
        format!("Resources ({})", finding.resources.len()),
        String::new(),
    )];
    rows.extend(finding.resource_rows.iter().cloned());
    rows
}

pub(super) fn sh_finding_compliance_lines(finding: &ShFinding) -> Vec<(String, String)> {
    let mut rows = Vec::new();
    if finding.compliance_status.is_empty() && finding.security_control_id.is_empty() {
        rows.push((
            " Not a control-based finding (no compliance data)".to_string(),
            String::new(),
        ));
        return rows;
    }
    if !finding.compliance_status.is_empty() {
        rows.push(("Status".to_string(), finding.compliance_status.clone()));
    }
    if !finding.security_control_id.is_empty() {
        rows.push(("Control ID".to_string(), finding.security_control_id.clone()));
    }

    // Why did it fail? The reason codes are the actionable half of a FAILED
    // control finding and were previously dropped on the floor.
    if !finding.status_reasons.is_empty() {
        rows.push((String::new(), String::new()));
        rows.push(("Status Reasons".to_string(), String::new()));
        for (code, description) in &finding.status_reasons {
            if !code.is_empty() {
                rows.push((format!("  {}", code), String::new()));
            }
            for line in wrap_plain(description, 62) {
                rows.push((format!("    {}", line), String::new()));
            }
        }
    }

    if !finding.associated_standards.is_empty() {
        rows.push((String::new(), String::new()));
        rows.push(("Associated Standards".to_string(), String::new()));
        for s in &finding.associated_standards {
            rows.push(("  Standard".to_string(), s.clone()));
        }
    }

    if !finding.control_parameters.is_empty() {
        rows.push((String::new(), String::new()));
        rows.push(("Control Parameters".to_string(), String::new()));
        for (name, value) in &finding.control_parameters {
            rows.push((format!("  {}", name), value.clone()));
        }
    }

    if !finding.related_requirements.is_empty() {
        rows.push((String::new(), String::new()));
        rows.push(("Related Requirements".to_string(), String::new())); // group header
        for r in &finding.related_requirements {
            rows.push((format!("  {}", r), String::new()));
        }
    }
    rows
}

pub(super) fn sh_finding_remediation_lines(finding: &ShFinding) -> Vec<(String, String)> {
    let mut rows = Vec::new();
    if finding.remediation_text.is_empty() && finding.remediation_url.is_empty() {
        rows.push((
            " No remediation guidance on this finding".to_string(),
            String::new(),
        ));
        return rows;
    }
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

/// The ASFF audit trail (lazy `GetFindingHistory`) — what changed this
/// finding's workflow status or note, and who did it.
pub(super) fn sh_finding_history_lines(
    finding: &ShFinding,
    history: Option<&Lazy<Vec<crate::aws::services::security_hub::ShHistoryRecord>>>,
) -> Vec<(String, String)> {
    if finding.product_arn.is_empty() {
        return vec![(
            " No product ARN on this finding — history can't be looked up".to_string(),
            String::new(),
        )];
    }
    let mut rows: Vec<(String, String)> = Vec::new();
    match history {
        None | Some(Lazy::Loading) => rows.push(("Loading…".to_string(), String::new())),
        Some(Lazy::Error(e)) => rows.extend(error_rows(e)),
        Some(Lazy::Loaded(records)) if records.is_empty() => rows.push((
            "No recorded changes — the finding hasn't been triaged".to_string(),
            String::new(),
        )),
        Some(Lazy::Loaded(records)) => {
            rows.push((format!("Changes ({})", records.len()), String::new()));
            for r in records {
                rows.push((String::new(), String::new()));
                let when = if r.time.is_empty() { "(unknown)" } else { &r.time };
                rows.push((when.to_string(), String::new())); // group header
                if r.created {
                    rows.push(("  Event".to_string(), "finding created".to_string()));
                }
                if !r.source.is_empty() {
                    rows.push(("  Source".to_string(), r.source.clone()));
                }
                if !r.identity.is_empty() {
                    rows.push(("  Identity".to_string(), r.identity.clone()));
                }
                for (field, old, new) in &r.updates {
                    rows.push((
                        format!("  {}", field),
                        if old.is_empty() {
                            new.clone()
                        } else {
                            format!("{} → {}", old, new)
                        },
                    ));
                }
            }
        }
    }
    rows
}

// ── Security control split pane ─────────────────────────────────────────────

pub(super) fn render_sh_control_split(app: &App, control: &ShControl, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(
        app,
        focused,
        crate::aws::services::security_hub::SH_CONTROL_SECTIONS.len(),
        "",
    );
    let mut block = theme::pane_block("Security Control", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let verdict_color = match control.compliance.as_str() {
        crate::aws::services::security_hub::FAILED => theme::error(),
        crate::aws::services::security_hub::PASSED => theme::success(),
        _ => theme::text_dim(),
    };
    let sev_color = severity_color(&control.severity);

    let mut second = vec![
        Span::raw("  "),
        Span::styled("● ", Style::default().fg(verdict_color)),
        Span::styled(
            control.compliance.clone(),
            Style::default().fg(verdict_color).add_modifier(Modifier::BOLD),
        ),
        Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
        Span::styled(control.severity.clone(), Style::default().fg(sev_color)),
    ];
    if control.failed_resources > 0 {
        second.push(Span::styled("  ·  ", Style::default().fg(theme::text_dim())));
        second.push(Span::styled(
            format!("{} failing", control.failed_resources),
            Style::default().fg(theme::error()),
        ));
    }

    let header = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                control.id.clone(),
                Style::default()
                    .fg(crate::ui::theme::text_primary())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("  ", Style::default()),
            Span::styled(control.title.clone(), Style::default().fg(theme::text_dim())),
        ]),
        Line::from(second),
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
        &descriptor_tabs(app, &crate::aws::services::security_hub::SH_CONTROL_SECTIONS),
    );
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub fn sh_control_section_lines(
    control: &ShControl,
    section: ShControlDetailSection,
    siblings: &[Box<dyn crate::aws::resource::Resource>],
) -> Vec<(String, String)> {
    use crate::aws::services::security_hub::{DISABLED, FAILED, NO_DATA};
    match section {
        ShControlDetailSection::Overview => {
            let mut rows = vec![
                ("Control".to_string(), control.id.clone()),
                ("Title".to_string(), control.title.clone()),
                ("Severity".to_string(), control.severity.clone()),
                ("Compliance".to_string(), control.compliance.clone()),
            ];
            if control.compliance == FAILED {
                rows.push((
                    "Failing Resources".to_string(),
                    control.failed_resources.to_string(),
                ));
            }
            if control.compliance == NO_DATA {
                // Annotation row (· prefix): dimmed, and honest about which of
                // the two reasons applies.
                let why = if control.standards.is_empty() {
                    "· not in any enabled standard — never evaluated"
                } else {
                    "· the failed-control scan hit its page cap — no verdict"
                };
                rows.push((format!("  {}", why), String::new()));
            }
            if !control.control_status.is_empty() {
                rows.push(("Status".to_string(), control.control_status.clone()));
            }
            if control.control_status == DISABLED && !control.last_update_reason.is_empty() {
                rows.push((
                    "Disabled Reason".to_string(),
                    control.last_update_reason.clone(),
                ));
            }
            if !control.update_status.is_empty() {
                rows.push(("Update Status".to_string(), control.update_status.clone()));
            }
            if !control.region_availability.is_empty() {
                rows.push((
                    "Region Availability".to_string(),
                    control.region_availability.clone(),
                ));
            }
            if !control.arn.is_empty() {
                rows.push(("ARN".to_string(), control.arn.clone()));
            }
            if !control.description.is_empty() {
                rows.push((String::new(), String::new()));
                rows.push(("Description".to_string(), String::new()));
                for line in wrap_plain(&control.description, 64) {
                    rows.push((format!("  {}", line), String::new()));
                }
            }
            if !control.remediation_url.is_empty() {
                rows.push((String::new(), String::new()));
                rows.push(("Remediation".to_string(), control.remediation_url.clone()));
            }
            rows
        }
        ShControlDetailSection::Standards => {
            if control.standards.is_empty() {
                return vec![(
                    " Not part of any enabled standard — this control is never evaluated"
                        .to_string(),
                    String::new(),
                )];
            }
            let mut rows = vec![(
                format!("Enabled Standards ({})", control.standards.len()),
                String::new(),
            )];
            rows.push((String::new(), String::new()));
            for s in &control.standards {
                rows.push(("Standard".to_string(), s.clone()));
            }
            rows
        }
        ShControlDetailSection::Parameters => {
            let mut rows: Vec<(String, String)> = Vec::new();
            if control.parameters.is_empty() {
                rows.push((
                    " Running with AWS default parameters".to_string(),
                    String::new(),
                ));
            } else {
                rows.push(("Configured".to_string(), String::new()));
                for (name, value) in &control.parameters {
                    rows.push((format!("  {}", name), value.clone()));
                }
            }
            if !control.customizable.is_empty() {
                rows.push((String::new(), String::new()));
                rows.push(("Customizable".to_string(), String::new()));
                for p in &control.customizable {
                    rows.push((format!("  {}", p), String::new()));
                }
            }
            rows
        }
        ShControlDetailSection::Failing => {
            // Sibling filter (the Vpc-Subnets pattern): zero fetch, but the
            // loaded findings are severity-scoped and capped, so say so when
            // the counts disagree rather than implying this is the full set.
            let matches: Vec<&ShFinding> = siblings
                .iter()
                .filter_map(|r| r.as_any().downcast_ref::<ShFinding>())
                .filter(|f| {
                    f.security_control_id == control.id
                        && f.compliance_status == FAILED
                        && f.workflow_status != "SUPPRESSED"
                })
                .collect();

            if control.failed_resources == 0 {
                return vec![(
                    " Nothing is currently failing this control".to_string(),
                    String::new(),
                )];
            }

            let mut rows = vec![(
                format!("Failing Resources ({})", control.failed_resources),
                String::new(),
            )];
            if matches.is_empty() {
                rows.push((String::new(), String::new()));
                rows.push((
                    "  · none loaded at this severity scope — widen with `t` on the list"
                        .to_string(),
                    String::new(),
                ));
                return rows;
            }
            if matches.len() < control.failed_resources {
                rows.push((
                    format!(
                        "  · {} of {} loaded at this severity scope",
                        matches.len(),
                        control.failed_resources
                    ),
                    String::new(),
                ));
            }
            for f in matches {
                rows.push((String::new(), String::new()));
                for (rtype, id) in &f.resources {
                    // Key-value so the generic classifier makes ids jumpable.
                    rows.push((
                        if rtype.is_empty() {
                            "Resource".to_string()
                        } else {
                            rtype.clone()
                        },
                        id.clone(),
                    ));
                }
                if !f.account_id.is_empty() {
                    rows.push(("  Account".to_string(), f.account_id.clone()));
                }
                if let Some((_, reason)) = f.status_reasons.first() {
                    for line in wrap_plain(reason, 62) {
                        rows.push((format!("  {}", line), String::new()));
                    }
                }
            }
            rows
        }
    }
}

// ── Automation rule split pane ──────────────────────────────────────────────

pub(super) fn render_sh_automation_rule_split(
    app: &App,
    rule: &crate::aws::services::security_hub::ShAutomationRule,
    area: Rect,
    frame: &mut Frame,
) {
    let focused = app.details_focused;
    let footer = detail_footer(
        app,
        focused,
        crate::aws::services::security_hub::SH_AUTOMATION_SECTIONS.len(),
        "",
    );
    let mut block = theme::pane_block("Automation Rule", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let (dot_color, status_text) = if rule.status == crate::aws::services::security_hub::DISABLED {
        (theme::text_dim(), rule.status.clone())
    } else if rule.suppresses() {
        (theme::warning(), format!("{} · suppresses", rule.status))
    } else {
        (theme::success(), rule.status.clone())
    };

    let mut second = vec![
        Span::raw("  "),
        Span::styled("● ", Style::default().fg(dot_color)),
        Span::styled(
            status_text,
            Style::default().fg(dot_color).add_modifier(Modifier::BOLD),
        ),
        Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
        Span::styled(
            format!("order {}", rule.order),
            Style::default().fg(theme::text_dim()),
        ),
    ];
    if rule.is_terminal {
        second.push(Span::styled("  ·  ", Style::default().fg(theme::text_dim())));
        second.push(Span::styled(
            "terminal",
            Style::default().fg(theme::accent()),
        ));
    }

    let header = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                rule.name.clone(),
                Style::default()
                    .fg(crate::ui::theme::text_primary())
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(second),
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
        &descriptor_tabs(
            app,
            &crate::aws::services::security_hub::SH_AUTOMATION_SECTIONS,
        ),
    );
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub fn sh_automation_rule_section_lines(
    rule: &crate::aws::services::security_hub::ShAutomationRule,
    section: crate::aws::services::security_hub::ShAutomationRuleDetailSection,
) -> Vec<(String, String)> {
    use crate::aws::services::security_hub::ShAutomationRuleDetailSection as S;
    match section {
        S::Overview => {
            let mut rows = vec![
                ("Rule".to_string(), rule.name.clone()),
                ("Status".to_string(), rule.status.clone()),
                ("Order".to_string(), rule.order.to_string()),
                (
                    "Terminal".to_string(),
                    if rule.is_terminal {
                        "✓ yes — stops later rules".to_string()
                    } else {
                        "no".to_string()
                    },
                ),
            ];
            if rule.suppresses() {
                // ⚠-prefixed content line: this rule is why a finding you
                // expect may not be in the Findings list.
                rows.push((
                    "  ⚠ suppresses matching findings — they load but hide under `a`".to_string(),
                    String::new(),
                ));
            }
            if !rule.description.is_empty() {
                rows.push((String::new(), String::new()));
                rows.push(("Description".to_string(), String::new()));
                for line in wrap_plain(&rule.description, 64) {
                    rows.push((format!("  {}", line), String::new()));
                }
            }
            rows.push((String::new(), String::new()));
            rows.push(("Lifecycle".to_string(), String::new()));
            if !rule.created_by.is_empty() {
                rows.push(("  Created By".to_string(), rule.created_by.clone()));
            }
            if !rule.created_at.is_empty() {
                rows.push(("  Created".to_string(), rule.created_at.clone()));
            }
            if !rule.updated_at.is_empty() {
                rows.push(("  Updated".to_string(), rule.updated_at.clone()));
            }
            rows.push((String::new(), String::new()));
            rows.push(("ARN".to_string(), rule.arn.clone()));
            rows
        }
        S::Criteria => {
            if rule.criteria_rows.is_empty() {
                return vec![(
                    " No criteria — this rule matches every finding".to_string(),
                    String::new(),
                )];
            }
            let mut rows = vec![("Matches when".to_string(), String::new())];
            rows.push((String::new(), String::new()));
            rows.extend(rule.criteria_rows.iter().cloned());
            rows
        }
        S::Actions => {
            if rule.action_rows.is_empty() {
                return vec![(
                    " No field updates on this rule".to_string(),
                    String::new(),
                )];
            }
            let mut rows = vec![("Then set".to_string(), String::new())];
            rows.push((String::new(), String::new()));
            rows.extend(rule.action_rows.iter().cloned());
            rows
        }
    }
}

// ── Security Hub settings / configuration policy split panes ────────────────

pub fn sh_settings_section_lines(
    s: &crate::aws::services::security_hub::ShSettings,
    section: crate::aws::services::security_hub::ShSettingsDetailSection,
) -> Vec<(String, String)> {
    use crate::aws::services::security_hub::ShSettingsDetailSection as S;
    match section {
        S::Overview => {
            let mut rows: Vec<(String, String)> = Vec::new();
            if s.hub_arn.is_empty() {
                rows.push((
                    " Security Hub isn't enabled in this region".to_string(),
                    String::new(),
                ));
                rows.extend(s.section_errors("hub:"));
                return rows;
            }
            rows.push(("Hub ARN".to_string(), s.hub_arn.clone()));
            if !s.subscribed_at.is_empty() {
                rows.push(("Enabled".to_string(), s.subscribed_at.clone()));
            }
            if let Some(auto) = s.auto_enable_controls {
                rows.push((
                    "Auto-Enable Controls".to_string(),
                    if auto {
                        "✓ new controls turn on automatically".to_string()
                    } else {
                        "✗ new controls stay off until enabled".to_string()
                    },
                ));
            }
            if !s.control_finding_generator.is_empty() {
                // SECURITY_CONTROL = consolidated control findings: one finding
                // per control regardless of how many standards include it.
                let explain = match s.control_finding_generator.as_str() {
                    "SECURITY_CONTROL" => "consolidated — one finding per control",
                    "STANDARD_CONTROL" => "per standard — duplicate findings across standards",
                    _ => "",
                };
                rows.push((
                    "Control Findings".to_string(),
                    if explain.is_empty() {
                        s.control_finding_generator.clone()
                    } else {
                        format!("{} ({})", s.control_finding_generator, explain)
                    },
                ));
            }
            rows.extend(s.section_errors("hub:"));
            rows
        }
        S::Regions => {
            let mut rows: Vec<(String, String)> = Vec::new();
            if !s.aggregation_configured {
                rows.push((
                    " No cross-region aggregation — this view shows only the current region"
                        .to_string(),
                    String::new(),
                ));
                rows.extend(s.section_errors("regions:"));
                return rows;
            }
            rows.push((
                "Aggregation Region".to_string(),
                s.aggregation_region.clone(),
            ));
            if !s.region_linking_mode.is_empty() {
                rows.push(("Linking Mode".to_string(), s.region_linking_mode.clone()));
            }
            if !s.linked_regions.is_empty() {
                rows.push((String::new(), String::new()));
                rows.push((
                    format!("Linked Regions ({})", s.linked_regions.len()),
                    String::new(),
                ));
                for r in &s.linked_regions {
                    rows.push((format!("  {}", r), String::new()));
                }
            }
            rows.extend(s.section_errors("regions:"));
            rows
        }
        S::Organization => {
            let mut rows: Vec<(String, String)> = Vec::new();
            if !s.administrator_account.is_empty() {
                rows.push(("Administrator".to_string(), s.administrator_account.clone()));
                if !s.administrator_status.is_empty() {
                    rows.push((
                        "Membership".to_string(),
                        s.administrator_status.clone(),
                    ));
                }
                rows.push((String::new(), String::new()));
            }
            if !s.delegated_admins.is_empty() {
                rows.push(("Delegated Admins".to_string(), String::new()));
                for a in &s.delegated_admins {
                    rows.push((format!("  {}", a), String::new()));
                }
                rows.push((String::new(), String::new()));
            }
            if !s.org_readable {
                rows.push((
                    " Organization settings are readable only by the Security Hub administrator"
                        .to_string(),
                    String::new(),
                ));
                rows.extend(s.section_errors("organization:"));
                return rows;
            }
            if !s.org_config_type.is_empty() {
                // CENTRAL means configuration policies decide what runs where;
                // LOCAL means each account configures itself.
                let explain = match s.org_config_type.as_str() {
                    "CENTRAL" => "policies drive member configuration",
                    "LOCAL" => "each account configures itself",
                    _ => "",
                };
                rows.push((
                    "Configuration".to_string(),
                    if explain.is_empty() {
                        s.org_config_type.clone()
                    } else {
                        format!("{} ({})", s.org_config_type, explain)
                    },
                ));
            }
            if !s.org_config_status.is_empty() {
                rows.push(("Status".to_string(), s.org_config_status.clone()));
            }
            if !s.org_config_status_message.is_empty() {
                for line in wrap_plain(&s.org_config_status_message, 62) {
                    rows.push((format!("  {}", line), String::new()));
                }
            }
            if let Some(auto) = s.auto_enable_members {
                rows.push((
                    "Auto-Enable Members".to_string(),
                    if auto { "✓ yes" } else { "✗ no" }.to_string(),
                ));
            }
            if !s.auto_enable_standards.is_empty() {
                rows.push((
                    "Auto-Enable Standards".to_string(),
                    s.auto_enable_standards.clone(),
                ));
            }
            if s.member_limit_reached == Some(true) {
                rows.push((
                    "  ⚠ member account limit reached — new accounts won't be onboarded"
                        .to_string(),
                    String::new(),
                ));
            }
            rows.extend(s.section_errors("organization:"));
            rows
        }
    }
}

pub fn sh_config_policy_section_lines(
    p: &crate::aws::services::security_hub::ShConfigPolicy,
    section: crate::aws::services::security_hub::ShConfigPolicyDetailSection,
) -> Vec<(String, String)> {
    use crate::aws::services::security_hub::ShConfigPolicyDetailSection as S;
    match section {
        S::Overview => {
            let mut rows = vec![
                ("Policy".to_string(), p.name.clone()),
                (
                    "Security Hub".to_string(),
                    if p.service_enabled {
                        "✓ enabled for targets".to_string()
                    } else {
                        "✗ disabled for targets".to_string()
                    },
                ),
                ("Targets".to_string(), p.targets.len().to_string()),
            ];
            let failed = p.failed_targets();
            if failed > 0 {
                rows.push((
                    format!("  ⚠ {} target(s) failed to apply this policy", failed),
                    String::new(),
                ));
            }
            if p.targets.is_empty() {
                rows.push((
                    "  · not associated with anything — configures nothing".to_string(),
                    String::new(),
                ));
            }
            if !p.description.is_empty() {
                rows.push((String::new(), String::new()));
                rows.push(("Description".to_string(), String::new()));
                for line in wrap_plain(&p.description, 64) {
                    rows.push((format!("  {}", line), String::new()));
                }
            }
            rows.push((String::new(), String::new()));
            if !p.created_at.is_empty() {
                rows.push(("Created".to_string(), p.created_at.clone()));
            }
            if !p.updated_at.is_empty() {
                rows.push(("Updated".to_string(), p.updated_at.clone()));
            }
            rows.push(("Policy ID".to_string(), p.id.clone()));
            if !p.arn.is_empty() {
                rows.push(("ARN".to_string(), p.arn.clone()));
            }
            if !p.detail_error.is_empty() {
                rows.extend(error_rows(&p.detail_error));
            }
            rows
        }
        S::Controls => {
            if !p.service_enabled {
                return vec![(
                    " Security Hub is disabled for this policy's targets — no standards or controls run"
                        .to_string(),
                    String::new(),
                )];
            }
            let mut rows: Vec<(String, String)> = Vec::new();
            if p.enabled_standards.is_empty() {
                rows.push(("No standards enabled by this policy".to_string(), String::new()));
            } else {
                rows.push((
                    format!("Standards ({})", p.enabled_standards.len()),
                    String::new(),
                ));
                for s in &p.enabled_standards {
                    rows.push(("  Standard".to_string(), s.clone()));
                }
            }

            // Exactly one list is populated, and which one inverts the meaning:
            // naming enabled controls turns *everything else* off, including
            // controls AWS hasn't shipped yet.
            if !p.enabled_controls.is_empty() {
                rows.push((String::new(), String::new()));
                rows.push((
                    format!("Enabled Controls ({})", p.enabled_controls.len()),
                    String::new(),
                ));
                rows.push((
                    "  · every other control is off, including future ones".to_string(),
                    String::new(),
                ));
                for c in &p.enabled_controls {
                    rows.push((format!("  {}", c), String::new()));
                }
            }
            if !p.disabled_controls.is_empty() {
                rows.push((String::new(), String::new()));
                rows.push((
                    format!("Disabled Controls ({})", p.disabled_controls.len()),
                    String::new(),
                ));
                rows.push((
                    "  · every other control is on, including future ones".to_string(),
                    String::new(),
                ));
                for c in &p.disabled_controls {
                    rows.push((format!("  {}", c), String::new()));
                }
            }
            if !p.custom_parameters.is_empty() {
                rows.push((String::new(), String::new()));
                rows.push(("Custom Parameters".to_string(), String::new()));
                for (k, v) in &p.custom_parameters {
                    rows.push((format!("  {}", k), v.clone()));
                }
            }
            rows
        }
        S::Targets => {
            if p.targets.is_empty() {
                return vec![(
                    " Not associated with any account, OU or root".to_string(),
                    String::new(),
                )];
            }
            let mut rows = vec![(format!("Targets ({})", p.targets.len()), String::new())];
            for t in &p.targets {
                rows.push((String::new(), String::new()));
                // Key-value so `ou-`/`r-`/account-id values classify as jumps
                // into Organizations.
                let label = if t.target_type.is_empty() {
                    "Target".to_string()
                } else {
                    t.target_type.clone()
                };
                rows.push((label, t.target_id.clone()));
                if !t.association_type.is_empty() {
                    rows.push(("  Association".to_string(), t.association_type.clone()));
                }
                if !t.status.is_empty() {
                    rows.push(("  Status".to_string(), t.status.clone()));
                }
                if !t.status_message.is_empty() {
                    for line in wrap_plain(&t.status_message, 60) {
                        rows.push((format!("    {}", line), String::new()));
                    }
                }
                if !t.updated_at.is_empty() {
                    rows.push(("  Updated".to_string(), t.updated_at.clone()));
                }
            }
            rows
        }
    }
}
