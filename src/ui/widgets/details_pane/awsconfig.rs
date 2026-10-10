use super::*;

// ── Config Rule split pane ─────────────────────────────────────────────────────

pub(super) fn render_config_rule_split(app: &App, rule: &ConfigRule, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 3, "");

    let mut block = theme::pane_block("Config Rule", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let header_lines = build_config_rule_header_lines(rule);
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
    render_config_rule_section_tabs(app, chunks[2], frame);
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub(super) fn build_config_rule_header_lines(rule: &ConfigRule) -> Vec<Line<'static>> {
    let mut lines = vec![Line::raw("")];
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            rule.name.clone(),
            Style::default().fg(crate::ui::theme::text_primary()).add_modifier(Modifier::BOLD),
        ),
    ]));

    let comp_color = match rule.compliance.as_str() {
        "COMPLIANT" => theme::success(),
        "NON_COMPLIANT" => theme::error(),
        _ => theme::text_dim(),
    };
    let source_label = if rule.source == "AWS" {
        format!("AWS Managed ({})", rule.identifier)
    } else {
        rule.source.clone()
    };
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(rule.compliance.clone(), Style::default().fg(comp_color)),
        Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
        Span::styled(source_label, Style::default().fg(theme::text_dim())),
    ]));
    lines.push(Line::raw(""));
    lines
}

pub(super) fn render_config_rule_section_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let tabs = descriptor_tabs(app, &crate::aws::services::awsconfig::CONFIG_RULE_SECTIONS);
    render_section_tab_bar(app, area, frame, &tabs);
}

pub fn config_rule_section_lines(
    rule: &ConfigRule,
    section: ConfigRuleDetailSection,
    eval_state: Option<&crate::lazy::Lazy<Vec<crate::aws::services::awsconfig::ConfigEvalResult>>>,
) -> Vec<(String, String)> {
    match section {
        ConfigRuleDetailSection::Details => config_rule_details_lines(rule),
        ConfigRuleDetailSection::NonCompliant => config_rule_noncompliant_lines(eval_state),
        ConfigRuleDetailSection::Parameters => config_rule_parameters_lines(rule),
    }
}

pub(super) fn config_rule_details_lines(rule: &ConfigRule) -> Vec<(String, String)> {
    let mut rows = vec![
        ("Rule Name".to_string(), rule.name.clone()),
        ("ARN".to_string(), rule.arn.clone()),
        ("Compliance".to_string(), rule.compliance.clone()),
        ("Non-Compliant Resources".to_string(), rule.non_compliant_count.to_string()),
        ("".to_string(), "".to_string()),
        ("Source".to_string(), rule.source.clone()),
    ];
    if !rule.identifier.is_empty() {
        rows.push(("Identifier".to_string(), rule.identifier.clone()));
    }
    rows.push(("Trigger".to_string(), rule.trigger.clone()));
    if !rule.description.is_empty() {
        rows.push(("".to_string(), "".to_string()));
        rows.push(("Description".to_string(), rule.description.clone()));
    }
    rows.push(("".to_string(), "".to_string()));
    rows
}

pub(super) fn config_rule_noncompliant_lines(state: Option<&crate::lazy::Lazy<Vec<crate::aws::services::awsconfig::ConfigEvalResult>>>) -> Vec<(String, String)> {
    match state {
        None | Some(crate::lazy::Lazy::Loading) => {
            vec![("".to_string(), "Loading non-compliant resources…".to_string())]
        }
        Some(crate::lazy::Lazy::Loaded(results)) => {
            if results.is_empty() {
                return vec![("".to_string(), "All resources compliant".to_string())];
            }
            let mut rows = vec![];
            for r in results {
                rows.push((r.resource_type.to_string(), r.resource_id.clone()));
                if !r.annotation.is_empty() {
                    rows.push((format!("  → {}", r.annotation), "".to_string()));
                }
                if let Some(ts) = &r.last_evaluated {
                    rows.push(("  Evaluated".to_string(), ts.clone()));
                }
                rows.push(("".to_string(), "".to_string()));
            }
            rows
        }
        Some(crate::lazy::Lazy::Error(e)) => error_rows(e),
    }
}

pub(super) fn config_rule_parameters_lines(rule: &ConfigRule) -> Vec<(String, String)> {
    if rule.parameters.is_empty() {
        return vec![("".to_string(), "No parameters".to_string())];
    }
    rule.parameters
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}
