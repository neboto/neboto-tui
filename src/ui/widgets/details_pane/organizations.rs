use super::*;

// ── Organizations Account split pane ──────────────────────────────────────

pub(super) fn render_org_account_split(app: &App, account: &OrgAccount, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;

    let on_scp_row = focused
        && OrgAccountDetailSection::from_index(app.detail_section_index())
            == OrgAccountDetailSection::Policies
        && {
            let rows = app.get_detail_lines_filtered();
            app.details_selected_index
                .and_then(|i| rows.get(i))
                .map(|(k, v)| org_account_scps_row_target(k, v).is_some())
                .unwrap_or(false)
        };

    let footer = detail_footer(
        app,
        focused,
        3,
        if on_scp_row {
            "⏎ expand · e edit · s assume role"
        } else {
            "s assume role"
        },
    );

    let mut block = theme::pane_block("Organizations Account", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let header_lines = build_org_account_header_lines(account);
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
    render_org_account_section_tabs(app, chunks[2], frame);
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub(super) fn build_org_account_header_lines(account: &OrgAccount) -> Vec<Line<'static>> {
    let mut lines = vec![Line::raw("")];

    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            account.account_name.clone(),
            Style::default()
                .fg(crate::ui::theme::text_primary())
                .add_modifier(Modifier::BOLD),
        ),
    ]));

    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(account.account_id.clone(), Style::default().fg(theme::text_dim())),
    ]));

    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(account.email.clone(), Style::default().fg(theme::text_dim())),
    ]));

    lines.push(Line::raw(""));
    lines
}

pub(super) fn render_org_account_section_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let tabs = descriptor_tabs(app, &crate::aws::services::organizations::ORG_ACCOUNT_SECTIONS);
    render_section_tab_bar(app, area, frame, &tabs);
}

pub fn org_account_section_lines(
    account: &OrgAccount,
    section: OrgAccountDetailSection,
    details_state: Option<&crate::lazy::Lazy<crate::aws::services::organizations::OrgAccountDetails>>,
    assume_hint: Option<&str>,
) -> Vec<(String, String)> {
    match section {
        OrgAccountDetailSection::Details => org_account_details_lines(account, assume_hint),
        OrgAccountDetailSection::OuPath => org_account_ou_path_lines(details_state),
        OrgAccountDetailSection::Policies => org_account_policies_lines(details_state),
    }
}

/// `assume_hint` is the dim `· press s to assume …` row (`org_assume_hint`),
/// present only when `s` would actually switch into this account. It sits in
/// Details because that's the section the unfocused preview shows.
pub(super) fn org_account_details_lines(account: &OrgAccount, assume_hint: Option<&str>) -> Vec<(String, String)> {
    let mut rows = vec![("".to_string(), "".to_string())];
    rows.push(("Account Name".to_string(), account.account_name.clone()));
    rows.push(("Account ID".to_string(), account.account_id.clone()));
    rows.push(("Email".to_string(), account.email.clone()));
    rows.push(("Status".to_string(), account.status.clone()));
    if let Some(method) = &account.joined_method {
        rows.push(("Joined Method".to_string(), method.clone()));
    }
    if let Some(ts) = &account.joined_timestamp {
        rows.push(("Joined".to_string(), ts.clone()));
    }
    rows.push(("ARN".to_string(), account.arn.clone()));
    rows.push(("".to_string(), "".to_string()));
    // Last, after the spacer: an export strips the hint row, and leading it
    // with its own spacer would leave two blanks there.
    if let Some(hint) = assume_hint {
        rows.push(("".to_string(), hint.to_string()));
    }
    rows
}

pub(super) fn org_account_ou_path_lines(
    details_state: Option<&crate::lazy::Lazy<crate::aws::services::organizations::OrgAccountDetails>>,
) -> Vec<(String, String)> {
    match details_state {
        None | Some(crate::lazy::Lazy::Loading) => {
            vec![("".to_string(), "Loading OU path…".to_string())]
        }
        Some(crate::lazy::Lazy::Error(e)) => error_rows(e),
        Some(crate::lazy::Lazy::Loaded(d)) => {
            let mut rows = vec![("".to_string(), "".to_string())];
            if d.ou_path.is_empty() {
                rows.push(("  No OU path found".to_string(), "".to_string()));
            } else {
                let joined: Vec<&str> = d.ou_path.iter().map(|(n, _)| n.as_str()).collect();
                rows.push((format!("  {}", joined.join(" / ")), "".to_string()));
                rows.push(("".to_string(), "".to_string()));
                // Each ancestor as its own row so ⏎ walks up the tree.
                for (name, id) in &d.ou_path {
                    rows.push((format!("  {}", name), id.clone()));
                }
            }
            if let Some(e) = &d.ou_path_error {
                rows.extend(error_rows(e));
            }
            rows.push(("".to_string(), "".to_string()));
            rows
        }
    }
}

/// Classify an SCPs-section row by its key/value shape: SCP rows are
/// `("  {name}", "{policy_id}")`. Group headers and blank spacers don't
/// start with two spaces, so they're naturally excluded. Returns
/// `(name, policy_id)`.
pub fn org_account_scps_row_target(key: &str, value: &str) -> Option<(String, String)> {
    if !key.starts_with("  ") || value.is_empty() {
        return None;
    }
    let name = key.trim().to_string();
    if name.is_empty() {
        return None;
    }
    Some((name, value.to_string()))
}

pub(super) fn org_account_policies_lines(
    details_state: Option<&crate::lazy::Lazy<crate::aws::services::organizations::OrgAccountDetails>>,
) -> Vec<(String, String)> {
    match details_state {
        None | Some(crate::lazy::Lazy::Loading) => {
            vec![("".to_string(), "Loading policies…".to_string())]
        }
        Some(crate::lazy::Lazy::Error(e)) => error_rows(e),
        Some(crate::lazy::Lazy::Loaded(d)) => org_policy_rows(
            &d.policies,
            d.policies_error.as_ref(),
            "No policies apply to this account",
        ),
    }
}

// ── Organizations OU split pane ───────────────────────────────────────────

pub(super) type OrgUnitLazy<'a> = Option<&'a crate::lazy::Lazy<crate::aws::services::organizations::OrgUnitDetails>>;

pub fn org_ou_section_lines(
    ou: &OrgUnit,
    section: OrgUnitDetailSection,
    details_state: OrgUnitLazy<'_>,
) -> Vec<(String, String)> {
    match section {
        OrgUnitDetailSection::Overview => org_ou_overview_lines(ou, details_state),
        OrgUnitDetailSection::Children => org_ou_children_lines(details_state),
        OrgUnitDetailSection::Policies => org_ou_policies_lines(details_state),
        OrgUnitDetailSection::Tags => org_ou_tags_lines(details_state),
    }
}

pub(super) fn org_ou_overview_lines(ou: &OrgUnit, details_state: OrgUnitLazy<'_>) -> Vec<(String, String)> {
    let mut rows = vec![("".to_string(), "".to_string())];
    rows.push(("OU Name".to_string(), ou.ou_name.clone()));
    rows.push(("OU ID".to_string(), ou.ou_id.clone()));
    rows.push(("Path".to_string(), ou.full_path.clone()));
    if !ou.parent_id.is_empty() {
        let parent_name = ou
            .parent_path
            .rsplit('/')
            .next()
            .filter(|s| !s.is_empty())
            .unwrap_or("Root");
        rows.push((
            "Parent".to_string(),
            format!("{} ({})", ou.parent_id, parent_name),
        ));
    }
    rows.push(("ARN".to_string(), ou.arn.clone()));

    rows.push(("".to_string(), "".to_string()));
    rows.push(("Contents".to_string(), "".to_string()));
    match details_state {
        None | Some(crate::lazy::Lazy::Loading) => {
            rows.push(("  Loading…".to_string(), "".to_string()));
        }
        Some(crate::lazy::Lazy::Error(e)) => rows.extend(error_rows(e)),
        Some(crate::lazy::Lazy::Loaded(d)) => {
            rows.push(("Child OUs".to_string(), d.child_ous.len().to_string()));
            rows.push(("Accounts".to_string(), d.accounts.len().to_string()));
            let direct = d.policies.iter().filter(|p| p.source.is_none()).count();
            rows.push((
                "Policies".to_string(),
                format!("{} ({} direct)", d.policies.len(), direct),
            ));
            rows.push(("Tags".to_string(), d.tags.len().to_string()));
        }
    }
    rows.push(("".to_string(), "".to_string()));
    rows
}

pub(super) fn org_ou_children_lines(details_state: OrgUnitLazy<'_>) -> Vec<(String, String)> {
    match details_state {
        None | Some(crate::lazy::Lazy::Loading) => {
            vec![("".to_string(), "Loading children…".to_string())]
        }
        Some(crate::lazy::Lazy::Error(e)) => error_rows(e),
        Some(crate::lazy::Lazy::Loaded(d)) => {
            let mut rows = vec![("".to_string(), "".to_string())];

            rows.push((
                "Organizational Units".to_string(),
                format!("({})", d.child_ous.len()),
            ));
            if d.child_ous.is_empty() {
                rows.push(("  No nested OUs".to_string(), "".to_string()));
            } else {
                for (name, id) in &d.child_ous {
                    rows.push((format!("  {}", name), id.clone()));
                }
            }

            rows.push(("".to_string(), "".to_string()));
            rows.push(("Accounts".to_string(), format!("({})", d.accounts.len())));
            if d.accounts.is_empty() {
                rows.push(("  No accounts directly in this OU".to_string(), "".to_string()));
            } else {
                for a in &d.accounts {
                    // The account id is the jump anchor; the email/status ride
                    // along as annotation rows under it.
                    rows.push((format!("  {}", a.account_name), a.account_id.clone()));
                    let suffix = if a.status == "ACTIVE" {
                        String::new()
                    } else {
                        format!("  ·  {}", a.status)
                    };
                    rows.push((format!("    · {}{}", a.email, suffix), "".to_string()));
                }
            }

            if let Some(e) = &d.children_error {
                rows.extend(error_rows(e));
            }
            rows.push(("".to_string(), "".to_string()));
            rows
        }
    }
}

/// Shared body for the account and OU panes' Policies sections: the effective
/// policy set grouped by type, each entry annotated with where it's attached.
/// Every policy row keeps the `("  {name}", "{policy_id}")` shape that
/// `org_account_scps_row_target` (the `e`/`v` document fetch) and
/// `App::org_row_jump_target` (⏎) both key on.
pub(super) fn org_policy_rows(
    policies: &[crate::aws::services::organizations::OrgAttachedPolicy],
    error: Option<&String>,
    empty_hint: &str,
) -> Vec<(String, String)> {
    let mut rows = vec![("".to_string(), "".to_string())];
    if policies.is_empty() {
        rows.push((format!("  {}", empty_hint), "".to_string()));
    } else {
        // Grouped by type in first-seen order (the enabled-type probe returns
        // SCP/RCP/Tag/… in the root's order); within a type the direct
        // attachments come first, then each ancestor working outward, because
        // that's the order `effective_policies` walks its targets.
        let mut current = "";
        for p in policies {
            if p.type_label != current {
                if !current.is_empty() {
                    rows.push(("".to_string(), "".to_string()));
                }
                rows.push((p.type_label.clone(), "".to_string()));
                current = &p.type_label;
            }
            rows.push((format!("  {}", p.name), p.policy_id.clone()));
            let origin = match &p.source {
                None => "attached directly".to_string(),
                Some((name, id)) => format!("inherited from {} ({})", name, id),
            };
            rows.push((format!("    · {}", origin), "".to_string()));
        }
    }
    if let Some(e) = error {
        rows.extend(error_rows(e));
    }
    rows.push(("".to_string(), "".to_string()));
    rows
}

pub(super) fn org_ou_policies_lines(details_state: OrgUnitLazy<'_>) -> Vec<(String, String)> {
    match details_state {
        None | Some(crate::lazy::Lazy::Loading) => {
            vec![("".to_string(), "Loading policies…".to_string())]
        }
        Some(crate::lazy::Lazy::Error(e)) => error_rows(e),
        Some(crate::lazy::Lazy::Loaded(d)) => org_policy_rows(
            &d.policies,
            d.policies_error.as_ref(),
            "No policies apply to this OU",
        ),
    }
}

pub(super) fn org_ou_tags_lines(details_state: OrgUnitLazy<'_>) -> Vec<(String, String)> {
    match details_state {
        None | Some(crate::lazy::Lazy::Loading) => {
            vec![("".to_string(), "Loading tags…".to_string())]
        }
        Some(crate::lazy::Lazy::Error(e)) => error_rows(e),
        Some(crate::lazy::Lazy::Loaded(d)) => {
            let mut rows = vec![("".to_string(), "".to_string())];
            if d.tags.is_empty() {
                rows.push(("  No tags".to_string(), "".to_string()));
            } else {
                for (k, v) in &d.tags {
                    rows.push((k.clone(), v.clone()));
                }
            }
            if let Some(e) = &d.tags_error {
                rows.extend(error_rows(e));
            }
            rows.push(("".to_string(), "".to_string()));
            rows
        }
    }
}

// ── Organizations SCP split pane ──────────────────────────────────────────

pub(super) fn render_org_scp_split(app: &App, scp: &OrgScp, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;

    let footer = detail_footer(app, focused, 2, "");

    let mut block = theme::pane_block("Organization Policy", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let header_lines = build_org_scp_header_lines(scp);
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
    render_org_scp_section_tabs(app, chunks[2], frame);
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub(super) fn build_org_scp_header_lines(scp: &OrgScp) -> Vec<Line<'static>> {
    let mut lines = vec![Line::raw("")];

    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            scp.policy_name.clone(),
            Style::default()
                .fg(crate::ui::theme::text_primary())
                .add_modifier(Modifier::BOLD),
        ),
    ]));

    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(scp.arn.clone(), Style::default().fg(theme::text_dim())),
    ]));

    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            format!("[{}]", scp.policy_type),
            Style::default().fg(theme::aws_orange()).add_modifier(Modifier::BOLD),
        ),
        Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
        Span::styled(
            if scp.aws_managed { "AWS Managed" } else { "Customer Managed" },
            Style::default().fg(theme::text_dim()),
        ),
    ]));

    lines.push(Line::raw(""));
    lines
}

pub(super) fn render_org_scp_section_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let tabs = descriptor_tabs(app, &crate::aws::services::organizations::ORG_SCP_SECTIONS);
    render_section_tab_bar(app, area, frame, &tabs);
}

pub fn org_scp_section_lines(
    _scp: &OrgScp,
    section: OrgScpDetailSection,
    details_state: Option<&crate::lazy::Lazy<crate::aws::services::organizations::OrgScpDetails>>,
) -> Vec<(String, String)> {
    match section {
        OrgScpDetailSection::Document => org_scp_document_lines(details_state),
        OrgScpDetailSection::AttachedTargets => org_scp_attached_targets_lines(details_state),
    }
}

pub(super) fn org_scp_document_lines(details_state: Option<&crate::lazy::Lazy<crate::aws::services::organizations::OrgScpDetails>>) -> Vec<(String, String)> {
    match details_state {
        None | Some(crate::lazy::Lazy::Loading) => {
            vec![("".to_string(), "Loading policy document…".to_string())]
        }
        Some(crate::lazy::Lazy::Error(e)) => error_rows(e),
        Some(crate::lazy::Lazy::Loaded(d)) => {
            let mut rows = vec![("".to_string(), "".to_string())];
            for line in d.document_json.lines() {
                rows.push((format!("  {}", line), "".to_string()));
            }
            rows.push(("".to_string(), "".to_string()));
            rows
        }
    }
}

pub(super) fn org_scp_attached_targets_lines(
    details_state: Option<&crate::lazy::Lazy<crate::aws::services::organizations::OrgScpDetails>>,
) -> Vec<(String, String)> {
    match details_state {
        None | Some(crate::lazy::Lazy::Loading) => {
            vec![("".to_string(), "Loading attached targets…".to_string())]
        }
        Some(crate::lazy::Lazy::Error(e)) => error_rows(e),
        Some(crate::lazy::Lazy::Loaded(d)) => {
            let mut rows = vec![("".to_string(), "".to_string())];
            if d.attached_targets.is_empty() {
                rows.push(("  No attached targets".to_string(), "".to_string()));
            } else {
                rows.push((
                    "Attached Targets".to_string(),
                    format!("({})", d.attached_targets.len()),
                ));
                for (name, id, ty) in &d.attached_targets {
                    rows.push((format!("  {} ({})", name, ty), id.clone()));
                }
            }
            rows.push(("".to_string(), "".to_string()));
            rows
        }
    }
}
