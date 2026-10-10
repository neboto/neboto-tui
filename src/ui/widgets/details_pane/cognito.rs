use super::*;

// ── Cognito user pool split pane ─────────────────────────────────────────────────

pub(super) fn render_cognito_user_pool_split(
    app: &App,
    pool: &CognitoUserPool,
    area: Rect,
    frame: &mut Frame,
) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 4, "");
    let mut block = theme::pane_block("Cognito User Pool", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let header = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                pool.name.clone(),
                Style::default().fg(crate::ui::theme::text_primary()).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(pool.id.clone(), Style::default().fg(theme::text_dim())),
            Span::styled("  ·  MFA ", Style::default().fg(theme::text_dim())),
            Span::styled(
                pool.mfa_config.clone(),
                Style::default().fg(if pool.mfa_config == "OFF" {
                    theme::text_dim()
                } else {
                    theme::success()
                }),
            ),
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
        &descriptor_tabs(app, &crate::aws::services::cognito::COGNITO_USER_POOL_SECTIONS),
    );
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub fn cognito_user_pool_section_lines(
    pool: &CognitoUserPool,
    section: CognitoUserPoolDetailSection,
    clients: Option<&crate::lazy::Lazy<Vec<crate::aws::services::cognito::CognitoAppClient>>>,
) -> Vec<(String, String)> {
    match section {
        CognitoUserPoolDetailSection::Details => {
            let mut rows = vec![
                ("Name".to_string(), pool.name.clone()),
                ("Pool ID".to_string(), pool.id.clone()),
                ("MFA".to_string(), pool.mfa_config.clone()),
                (
                    "Username Attributes".to_string(),
                    if pool.username_attributes.is_empty() {
                        "username".to_string()
                    } else {
                        pool.username_attributes.join(", ")
                    },
                ),
                (
                    "Estimated Users".to_string(),
                    format!("{} (approx)", pool.estimated_users),
                ),
            ];
            if let Some(d) = &pool.domain {
                rows.push(("Domain".to_string(), d.clone()));
            }
            if let Some(c) = &pool.created {
                rows.push(("Created".to_string(), c.clone()));
            }
            rows.push((String::new(), String::new()));
            rows.push(("ARN".to_string(), pool.arn.clone()));
            // Lambda triggers
            if !pool.lambda_triggers.is_empty() {
                rows.push((String::new(), String::new()));
                rows.push(("Lambda Triggers".to_string(), String::new())); // group header
                for (trigger_name, fn_name) in &pool.lambda_triggers {
                    rows.push((format!("  {}", trigger_name), fn_name.clone()));
                }
            }
            rows
        }
        CognitoUserPoolDetailSection::AppClients => match clients {
            None | Some(crate::lazy::Lazy::Loading) => {
                vec![("".to_string(), "Loading app clients…".to_string())]
            }
            Some(crate::lazy::Lazy::Error(e)) => {
                error_rows(e)
            }
            Some(crate::lazy::Lazy::Loaded(list)) => {
                if list.is_empty() {
                    return vec![("".to_string(), "No app clients".to_string())];
                }
                let mut rows = vec![(format!("App Clients ({})", list.len()), String::new())];
                rows.push((String::new(), String::new()));
                for c in list {
                    rows.push((c.name.clone(), String::new())); // group header
                    rows.push(("  Client ID".to_string(), c.id.clone()));
                    rows.push((
                        "  Secret".to_string(),
                        if c.has_secret { "✓ confidential".to_string() } else { "public".to_string() },
                    ));
                    if !c.explicit_auth_flows.is_empty() {
                        rows.push(("  Auth Flows".to_string(), c.explicit_auth_flows.join(", ")));
                    }
                    for (i, url) in c.callback_urls.iter().enumerate() {
                        let label = if i == 0 { "  Callback URLs".to_string() } else { "  ".to_string() };
                        rows.push((label, url.clone()));
                    }
                    rows.push((String::new(), String::new()));
                }
                rows
            }
        },
        CognitoUserPoolDetailSection::Policies => {
            vec![
                ("Password Policy".to_string(), String::new()), // group header
                (format!("  {}", pool.policies_summary), String::new()),
            ]
        }
        CognitoUserPoolDetailSection::Tags => tag_rows(&pool.tags),
    }
}
