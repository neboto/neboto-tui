use super::*;

// ── Cognito user pool split pane ─────────────────────────────────────────────────

pub(super) fn render_cognito_user_pool_split(
    app: &App,
    pool: &CognitoUserPool,
    area: Rect,
    frame: &mut Frame,
) {
    let focused = app.details_focused;
    let footer = detail_footer(
        app,
        focused,
        crate::aws::services::cognito::COGNITO_USER_POOL_SECTIONS.len(),
        "",
    );
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
    groups: Option<&crate::lazy::Lazy<crate::aws::services::cognito::CognitoGroups>>,
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
        CognitoUserPoolDetailSection::Groups => cognito_group_lines(groups),
    }
}

/// One group header per group (precedence order), its settings, then its
/// members. The role ARN row is jumpable through the IAM ARN classifier.
fn cognito_group_lines(
    state: Option<&crate::lazy::Lazy<crate::aws::services::cognito::CognitoGroups>>,
) -> Vec<(String, String)> {
    use crate::aws::services::cognito::{MAX_COGNITO_GROUPS, MAX_COGNITO_GROUP_MEMBERS};
    let data = match state {
        None | Some(crate::lazy::Lazy::Loading) => {
            return vec![(String::new(), "Loading…".to_string())];
        }
        Some(crate::lazy::Lazy::Error(e)) => return error_rows(e),
        Some(crate::lazy::Lazy::Loaded(d)) => d,
    };
    if data.groups.is_empty() {
        return vec![(String::new(), "No groups".to_string())];
    }
    let mut rows = Vec::new();
    if data.truncated {
        rows.push((String::new(), format!("· first {MAX_COGNITO_GROUPS} groups shown")));
        rows.push((String::new(), String::new()));
    }
    for g in &data.groups {
        rows.push((g.name.clone(), String::new()));
        if let Some(d) = &g.description {
            rows.push(("  Description".to_string(), d.clone()));
        }
        rows.push((
            "  Precedence".to_string(),
            g.precedence.map(|p| p.to_string()).unwrap_or_else(|| "—".to_string()),
        ));
        if let Some(role) = &g.role_arn {
            rows.push(("  IAM Role".to_string(), role.clone()));
        }
        match &g.members {
            Err(e) => rows.push(("  Members".to_string(), format!("⚠ {e}"))),
            Ok(members) => {
                let count = if g.members_truncated {
                    format!("{}+ (first {MAX_COGNITO_GROUP_MEMBERS} shown)", members.len())
                } else {
                    members.len().to_string()
                };
                rows.push(("  Members".to_string(), count));
                for m in members {
                    let mut state = m.status.clone().unwrap_or_default();
                    if !m.enabled {
                        state = if state.is_empty() {
                            "disabled".to_string()
                        } else {
                            format!("{state} · disabled")
                        };
                    }
                    rows.push((format!("    {}", m.label), state));
                }
            }
        }
        rows.push((String::new(), String::new()));
    }
    rows
}

#[cfg(test)]
mod group_tests {
    use super::*;
    use crate::aws::services::cognito::{CognitoGroup, CognitoGroupMember, CognitoGroups};
    use crate::lazy::Lazy;

    fn member(label: &str, enabled: bool, status: Option<&str>) -> CognitoGroupMember {
        CognitoGroupMember {
            username: label.into(),
            label: label.into(),
            enabled,
            status: status.map(str::to_string),
        }
    }

    fn group(name: &str, members: Result<Vec<CognitoGroupMember>, String>, truncated: bool) -> CognitoGroup {
        CognitoGroup {
            name: name.into(),
            description: None,
            precedence: Some(1),
            role_arn: None,
            members,
            members_truncated: truncated,
        }
    }

    fn lines(groups: Vec<CognitoGroup>, truncated: bool) -> Vec<(String, String)> {
        cognito_group_lines(Some(&Lazy::Loaded(CognitoGroups { groups, truncated })))
    }

    #[test]
    fn no_groups_is_a_note() {
        let rows = lines(vec![], false);
        assert_eq!(rows, vec![(String::new(), "No groups".to_string())]);
    }

    #[test]
    fn truncated_groups_say_so() {
        let rows = lines(vec![group("admins", Ok(vec![]), false)], true);
        assert!(rows.iter().any(|(_, v)| v.starts_with("· first 50 groups")), "{rows:?}");
    }

    #[test]
    fn a_failed_member_call_warns_on_that_group_only() {
        let rows = lines(
            vec![
                group("admins", Err("AccessDenied: no".into()), false),
                group("readers", Ok(vec![member("ana@example.com", true, Some("CONFIRMED"))]), false),
            ],
            false,
        );
        let members: Vec<&String> = rows.iter().filter(|(k, _)| k == "  Members").map(|(_, v)| v).collect();
        assert_eq!(members, vec!["⚠ AccessDenied: no", "1"], "{rows:?}");
        assert!(rows.contains(&("    ana@example.com".into(), "CONFIRMED".into())), "{rows:?}");
    }

    #[test]
    fn truncated_members_show_the_page_size() {
        let rows = lines(vec![group("all", Ok(vec![member("a", true, None)]), true)], false);
        assert!(rows.contains(&("  Members".into(), "1+ (first 60 shown)".into())), "{rows:?}");
    }

    #[test]
    fn disabled_members_are_marked_with_or_without_a_status() {
        let rows = lines(
            vec![group(
                "g",
                Ok(vec![member("a", false, Some("CONFIRMED")), member("b", false, None), member("c", true, None)]),
                false,
            )],
            false,
        );
        assert!(rows.contains(&("    a".into(), "CONFIRMED · disabled".into())), "{rows:?}");
        assert!(rows.contains(&("    b".into(), "disabled".into())), "{rows:?}");
        assert!(rows.contains(&("    c".into(), String::new())), "{rows:?}");
    }
}
