use super::*;

// ── OAM Sink / Link Split Panes (Cross-Account tab) ──────────────────────────

pub(super) fn render_oam_sink_split(
    app: &App,
    sink: &crate::aws::services::oam::OamSink,
    area: Rect,
    frame: &mut Frame,
) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 3, "");

    let mut block = theme::pane_block("OAM Sink", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let header_lines = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                sink.name.clone(),
                Style::default().fg(theme::text_primary()).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                "Monitoring account — receives cross-account telemetry",
                Style::default().fg(theme::text_dim()),
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
    let tabs = descriptor_tabs(app, &crate::aws::services::oam::OAM_SINK_SECTIONS);
    render_section_tab_bar(app, chunks[2], frame, &tabs);
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub(super) fn render_oam_link_split(
    app: &App,
    link: &crate::aws::services::oam::OamLink,
    area: Rect,
    frame: &mut Frame,
) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 2, "");

    let mut block = theme::pane_block("OAM Link", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let sink_account = crate::aws::services::oam::oam_arn_account(&link.sink_arn)
        .map(|a| format!("Source account — telemetry flows to sink in {}", a))
        .unwrap_or_else(|| "Source account — telemetry flows to the sink".to_string());
    let header_lines = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                link.label.clone(),
                Style::default().fg(theme::text_primary()).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(sink_account, Style::default().fg(theme::text_dim())),
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
    let tabs = descriptor_tabs(app, &crate::aws::services::oam::OAM_LINK_SECTIONS);
    render_section_tab_bar(app, chunks[2], frame, &tabs);
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub fn oam_sink_section_lines(
    sink: &crate::aws::services::oam::OamSink,
    section: crate::aws::services::oam::OamSinkDetailSection,
    policy_state: Option<&crate::lazy::Lazy<crate::aws::services::oam::OamSinkPolicy>>,
    links_state: Option<&crate::lazy::Lazy<Vec<crate::aws::services::oam::OamAttachedLink>>>,
) -> Vec<(String, String)> {
    use crate::aws::services::oam::OamSinkDetailSection as S;
    match section {
        S::Details => {
            let mut rows: Vec<(String, String)> = Vec::new();
            rows.push(("Sink".to_string(), "".to_string()));
            rows.push(("".to_string(), "".to_string()));
            rows.push(("  Name".to_string(), sink.name.clone()));
            if !sink.sink_id.is_empty() {
                rows.push(("  Sink ID".to_string(), sink.sink_id.clone()));
            }
            rows.push(("  ARN".to_string(), sink.arn.clone()));
            rows.push(("".to_string(), "".to_string()));
            rows.push((
                "  · This account is the monitoring account — source accounts link to this sink."
                    .to_string(),
                "".to_string(),
            ));
            rows
        }
        S::Policy => match policy_state {
            None | Some(crate::lazy::Lazy::Loading) => {
                vec![("".to_string(), "Loading policy…".to_string())]
            }
            Some(crate::lazy::Lazy::Loaded(p)) => {
                let mut rows: Vec<(String, String)> = Vec::new();
                rows.push(("Who may link".to_string(), "".to_string()));
                rows.push(("".to_string(), "".to_string()));
                if p.any_principal {
                    rows.push((
                        "  ⚠ Any AWS account (Principal \"*\" with no org condition)".to_string(),
                        "".to_string(),
                    ));
                }
                for org in &p.org_ids {
                    rows.push(("  Organization".to_string(), org.clone()));
                }
                for path in &p.org_paths {
                    rows.push(("  Org path".to_string(), path.clone()));
                }
                for acct in &p.accounts {
                    rows.push(("  Account".to_string(), acct.clone()));
                }
                if !p.any_principal
                    && p.org_ids.is_empty()
                    && p.org_paths.is_empty()
                    && p.accounts.is_empty()
                {
                    rows.push((
                        "  · no allow principals parsed — see the policy document below".to_string(),
                        "".to_string(),
                    ));
                }

                rows.push(("".to_string(), "".to_string()));
                rows.push(("Telemetry accepted".to_string(), "".to_string()));
                rows.push(("".to_string(), "".to_string()));
                if p.telemetry_types.is_empty() {
                    rows.push((
                        "  · no oam:ResourceTypes condition — every telemetry type is accepted"
                            .to_string(),
                        "".to_string(),
                    ));
                } else {
                    for t in &p.telemetry_types {
                        rows.push((
                            format!("  {}", crate::aws::services::oam::telemetry_display(t)),
                            t.clone(),
                        ));
                    }
                }

                rows.push(("".to_string(), "".to_string()));
                rows.push(("Policy document".to_string(), "".to_string()));
                rows.push(("".to_string(), "".to_string()));
                for line in p.policy_json.lines() {
                    rows.push((format!("  {}", line), "".to_string()));
                }
                rows
            }
            Some(crate::lazy::Lazy::Error(e)) => error_rows(e),
        },
        S::AttachedLinks => match links_state {
            None | Some(crate::lazy::Lazy::Loading) => {
                vec![("".to_string(), "Loading attached links…".to_string())]
            }
            Some(crate::lazy::Lazy::Loaded(links)) => {
                let mut rows: Vec<(String, String)> = Vec::new();
                rows.push((format!("Attached links ({})", links.len()), "".to_string()));
                rows.push(("".to_string(), "".to_string()));
                if links.is_empty() {
                    rows.push((
                        "  (no source accounts are linked to this sink)".to_string(),
                        "".to_string(),
                    ));
                } else {
                    for l in links {
                        let account = crate::aws::services::oam::oam_arn_account(&l.link_arn)
                            .unwrap_or("?");
                        rows.push((
                            format!("  {}", l.label),
                            format!(
                                "{} · {}",
                                account,
                                crate::aws::services::oam::telemetry_summary(&l.resource_types)
                            ),
                        ));
                    }
                }
                rows
            }
            Some(crate::lazy::Lazy::Error(e)) => error_rows(e),
        },
    }
}

pub fn oam_link_section_lines(
    link: &crate::aws::services::oam::OamLink,
    section: crate::aws::services::oam::OamLinkDetailSection,
    detail_state: Option<&crate::lazy::Lazy<crate::aws::services::oam::OamLinkDetail>>,
) -> Vec<(String, String)> {
    use crate::aws::services::oam::OamLinkDetailSection as S;
    match section {
        S::Details => {
            let mut rows: Vec<(String, String)> = Vec::new();
            rows.push(("Link".to_string(), "".to_string()));
            rows.push(("".to_string(), "".to_string()));
            rows.push(("  Label".to_string(), link.label.clone()));
            if !link.link_id.is_empty() {
                rows.push(("  Link ID".to_string(), link.link_id.clone()));
            }
            rows.push(("  ARN".to_string(), link.arn.clone()));
            rows.push(("".to_string(), "".to_string()));

            rows.push(("Sink".to_string(), "".to_string()));
            rows.push(("".to_string(), "".to_string()));
            if let Some(acct) = crate::aws::services::oam::oam_arn_account(&link.sink_arn) {
                rows.push(("  Monitoring account".to_string(), acct.to_string()));
            }
            rows.push(("  Sink ARN".to_string(), link.sink_arn.clone()));
            rows.push(("".to_string(), "".to_string()));

            rows.push(("Shared telemetry".to_string(), "".to_string()));
            rows.push(("".to_string(), "".to_string()));
            if link.resource_types.is_empty() {
                rows.push(("  (none)".to_string(), "".to_string()));
            } else {
                for t in &link.resource_types {
                    rows.push((
                        format!("  {}", crate::aws::services::oam::telemetry_display(t)),
                        t.clone(),
                    ));
                }
            }
            rows.push(("".to_string(), "".to_string()));
            rows.push((
                "  · This account is a source account — the telemetry above flows to the sink."
                    .to_string(),
                "".to_string(),
            ));
            rows
        }
        S::Configuration => match detail_state {
            None | Some(crate::lazy::Lazy::Loading) => {
                vec![("".to_string(), "Loading configuration…".to_string())]
            }
            Some(crate::lazy::Lazy::Loaded(d)) => {
                let mut rows: Vec<(String, String)> = Vec::new();
                rows.push(("Configuration".to_string(), "".to_string()));
                rows.push(("".to_string(), "".to_string()));
                if !d.label_template.is_empty() {
                    rows.push(("  Label template".to_string(), d.label_template.clone()));
                }

                rows.push(("".to_string(), "".to_string()));
                rows.push(("Filters".to_string(), "".to_string()));
                rows.push(("".to_string(), "".to_string()));
                rows.push((
                    "  Metric namespaces".to_string(),
                    if d.metric_filter.is_empty() {
                        "All namespaces".to_string()
                    } else {
                        d.metric_filter.clone()
                    },
                ));
                rows.push((
                    "  Log groups".to_string(),
                    if d.log_filter.is_empty() {
                        "All log groups".to_string()
                    } else {
                        d.log_filter.clone()
                    },
                ));

                if !d.tags.is_empty() {
                    rows.push(("".to_string(), "".to_string()));
                    rows.push(("Tags".to_string(), "".to_string()));
                    rows.push(("".to_string(), "".to_string()));
                    for (k, v) in &d.tags {
                        rows.push((format!("  {}", k), v.clone()));
                    }
                }
                rows
            }
            Some(crate::lazy::Lazy::Error(e)) => error_rows(e),
        },
    }
}
