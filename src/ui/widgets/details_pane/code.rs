use super::*;

// ── CodeCommit Repository split pane ───────────────────────────────────────────

/// Short (8-char) commit id, the git convention.
pub(super) fn cc_short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

/// The repo pane's lazy states + sibling rows, bundled so the renderer
/// signature stays sane as sections grow.
pub struct CcRepoPaneState<'a> {
    pub extras: Option<&'a Lazy<crate::aws::services::code::CcRepoExtras>>,
    pub branches: Option<&'a Lazy<crate::aws::services::code::CcBranches>>,
    pub commits: Option<&'a Lazy<crate::aws::services::code::CcCommitWalk>>,
    pub commits_branch: &'a str,
    pub readme: Option<&'a Lazy<Option<String>>>,
    /// Open PRs — sibling rows already in the list (zero fetch).
    pub prs: Vec<&'a crate::aws::services::code::CodeCommitPullRequest>,
    /// Recently-closed PRs — lazy per-repo fetch on section enter.
    pub closed_prs: Option<&'a Lazy<Vec<crate::aws::services::code::CodeCommitPullRequest>>>,
}

pub fn code_commit_repo_section_lines(
    repo: &crate::aws::services::code::CodeCommitRepo,
    section: CodeCommitRepoDetailSection,
    st: &CcRepoPaneState<'_>,
) -> Vec<(String, String)> {
    let extras_state = st.extras;
    let branches_state = st.branches;
    let commits_state = st.commits;
    let commits_branch = st.commits_branch;
    let readme_state = st.readme;
    let prs = &st.prs;
    match section {
        CodeCommitRepoDetailSection::Details => {
            let mut rows = vec![
                ("Name".to_string(), repo.name.clone()),
                ("ARN".to_string(), repo.arn.clone()),
                ("Description".to_string(), repo.description.clone()),
                ("Default Branch".to_string(), repo.default_branch.clone()),
                ("".to_string(), "".to_string()),
                ("Clone URLs".to_string(), "".to_string()),
                ("  HTTPS".to_string(), repo.clone_url_http.clone()),
                ("  GRC".to_string(), repo.clone_url_grc.clone()),
                ("  SSH".to_string(), repo.clone_url_ssh.clone()),
                ("".to_string(), "".to_string()),
                ("Last Modified".to_string(), repo.last_modified.clone()),
                ("Account ID".to_string(), repo.account_id.clone()),
                ("".to_string(), "".to_string()),
                ("Tags".to_string(), "".to_string()),
            ];
            match extras_state {
                None | Some(Lazy::Loading) => {
                    rows.push(("".to_string(), "Loading…".to_string()));
                }
                Some(Lazy::Error(e)) => rows.extend(error_rows(e)),
                Some(Lazy::Loaded(x)) => {
                    if let Some(e) = &x.tags_error {
                        rows.extend(error_rows(e));
                    } else if x.tags.is_empty() {
                        rows.push(("  · none".to_string(), "".to_string()));
                    } else {
                        for (k, v) in &x.tags {
                            rows.push((format!("  {}", k), v.clone()));
                        }
                    }
                }
            }
            rows
        }
        CodeCommitRepoDetailSection::Branches => {
            let mut rows = Vec::new();
            match branches_state {
                None | Some(Lazy::Loading) => {
                    rows.push(("".to_string(), "Loading…".to_string()));
                }
                Some(Lazy::Error(e)) => rows.extend(error_rows(e)),
                Some(Lazy::Loaded(b)) => {
                    rows.push((
                        format!("Branches ({})", b.branches.len() + b.unresolved),
                        "".to_string(),
                    ));
                    if b.branches.is_empty() {
                        rows.push(("".to_string(), "No branches (empty repository)".to_string()));
                    }
                    for br in &b.branches {
                        let mut tip = String::new();
                        if br.is_default {
                            tip.push_str("default · ");
                        }
                        tip.push_str(&cc_short_id(&br.tip_commit_id));
                        if !br.subject.is_empty() {
                            tip.push_str(&format!(" {}", clip_inline(&br.subject, 48)));
                        }
                        if !br.date.is_empty() {
                            tip.push_str(&format!(" · {}", br.date));
                        }
                        rows.push((format!("  {}", br.name), tip));
                    }
                    if b.unresolved > 0 {
                        rows.push((
                            format!(
                                "  · {} more branches not shown (tip lookups capped at {})",
                                b.unresolved,
                                crate::aws::services::code::MAX_CC_BRANCHES
                            ),
                            "".to_string(),
                        ));
                    }
                    if !b.branches.is_empty() {
                        rows.push(("".to_string(), "".to_string()));
                        rows.push((
                            "  · ⏎ on a branch opens its commits".to_string(),
                            "".to_string(),
                        ));
                    }
                }
            }
            rows
        }
        CodeCommitRepoDetailSection::Commits => {
            let mut rows = vec![
                ("Branch".to_string(), commits_branch.to_string()),
                ("".to_string(), "".to_string()),
            ];
            match commits_state {
                None | Some(Lazy::Loading) => {
                    rows.push(("".to_string(), "Loading…".to_string()));
                }
                Some(Lazy::Error(e)) => rows.extend(error_rows(e)),
                Some(Lazy::Loaded(walk)) => {
                    rows.push((
                        format!(
                            "Commits ({}{})",
                            walk.commits.len(),
                            if walk.truncated { "+" } else { "" }
                        ),
                        "".to_string(),
                    ));
                    for c in &walk.commits {
                        let mut v = clip_inline(&c.subject, 56);
                        if !c.author.is_empty() {
                            v.push_str(&format!(" · {}", c.author));
                        }
                        if !c.date.is_empty() {
                            v.push_str(&format!(" · {}", c.date));
                        }
                        rows.push((format!("  {}", cc_short_id(&c.id)), v));
                    }
                    rows.push(("".to_string(), "".to_string()));
                    if walk.truncated {
                        rows.push((
                            format!(
                                "  · older history not walked (capped at {})",
                                crate::aws::services::code::MAX_CC_COMMITS
                            ),
                            "".to_string(),
                        ));
                    }
                    rows.push((
                        "  · e opens the full log with commit bodies".to_string(),
                        "".to_string(),
                    ));
                }
            }
            rows
        }
        CodeCommitRepoDetailSection::PullRequests => {
            let mut rows = vec![(format!("Open ({})", prs.len()), "".to_string())];
            if prs.is_empty() {
                rows.push(("".to_string(), "No open pull requests".to_string()));
            }
            for p in prs {
                // Digits-only `#<id>` key — the shape `cc_repo_pr_row_jump_target`
                // keys on (only these rows exist on the Pull Requests sub-tab).
                rows.push((
                    format!("  #{}", p.pr_id),
                    format!(
                        "{} · {} · {}",
                        clip_inline(&p.title, 48),
                        p.author,
                        p.created
                    ),
                ));
            }
            if !prs.is_empty() {
                rows.push((
                    "  · ⏎ on a row opens the pull request".to_string(),
                    "".to_string(),
                ));
            }
            rows.push(("".to_string(), "".to_string()));
            rows.push(("Recently Closed".to_string(), "".to_string()));
            match st.closed_prs {
                None | Some(Lazy::Loading) => {
                    rows.push(("".to_string(), "Loading…".to_string()));
                }
                Some(Lazy::Error(e)) => rows.extend(error_rows(e)),
                Some(Lazy::Loaded(closed)) => {
                    if closed.is_empty() {
                        rows.push(("  · none".to_string(), "".to_string()));
                    }
                    for p in closed {
                        // The `(MERGED)` suffix keeps the digits-only jump
                        // classifier from firing — closed PRs have no row on
                        // the sub-tab to jump to.
                        rows.push((
                            format!("  #{} ({})", p.pr_id, p.status_label()),
                            format!(
                                "{} · {} · {}",
                                clip_inline(&p.title, 44),
                                if p.merged_by.is_empty() {
                                    p.author.clone()
                                } else {
                                    format!("merged by {}", p.merged_by)
                                },
                                p.last_activity
                            ),
                        ));
                    }
                }
            }
            rows
        }
        CodeCommitRepoDetailSection::Triggers => {
            let mut rows = Vec::new();
            match extras_state {
                None | Some(Lazy::Loading) => {
                    rows.push(("".to_string(), "Loading…".to_string()));
                }
                Some(Lazy::Error(e)) => rows.extend(error_rows(e)),
                Some(Lazy::Loaded(x)) => {
                    if let Some(e) = &x.triggers_error {
                        rows.extend(error_rows(e));
                    } else if x.triggers.is_empty() {
                        rows.push(("".to_string(), "No triggers configured".to_string()));
                    } else {
                        for t in &x.triggers {
                            rows.push((t.name.clone(), "".to_string()));
                            rows.push(("  Destination".to_string(), t.destination_arn.clone()));
                            rows.push((
                                "  Events".to_string(),
                                if t.events.is_empty() {
                                    "-".to_string()
                                } else {
                                    t.events.join(", ")
                                },
                            ));
                            rows.push((
                                "  Branches".to_string(),
                                if t.branches.is_empty() {
                                    "all branches".to_string()
                                } else {
                                    t.branches.join(", ")
                                },
                            ));
                            if !t.custom_data.is_empty() {
                                rows.push(("  Custom Data".to_string(), t.custom_data.clone()));
                            }
                            rows.push(("".to_string(), "".to_string()));
                        }
                    }
                }
            }
            rows
        }
        CodeCommitRepoDetailSection::Readme => match readme_state {
            None | Some(Lazy::Loading) => {
                vec![("".to_string(), "Loading README…".to_string())]
            }
            Some(Lazy::Loaded(None)) => {
                vec![("".to_string(), "No README found".to_string())]
            }
            Some(Lazy::Error(e)) => error_rows(e),
            Some(Lazy::Loaded(Some(content))) => {
                content
                    .lines()
                    .map(|line| (format!("  {}", line), "".to_string()))
                    .collect()
            }
        },
    }
}

pub(super) fn render_code_commit_repo_split(
    app: &App,
    repo: &crate::aws::services::code::CodeCommitRepo,
    area: ratatui::layout::Rect,
    frame: &mut Frame,
) {
    let focused = app.details_focused;
    let footer = detail_footer(
        app,
        focused,
        crate::aws::services::code::CODE_COMMIT_SECTIONS.len(),
        "",
    );
    let mut block = theme::pane_block("Repository", focused);
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
                repo.name.clone(),
                Style::default()
                    .fg(crate::ui::theme::text_primary())
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                if repo.description.is_empty() {
                    repo.default_branch.clone()
                } else {
                    repo.description.clone()
                },
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

    // Section tabs
    let sections = descriptor_tabs(app, &crate::aws::services::code::CODE_COMMIT_SECTIONS);
    let mut spans: Vec<Span> = vec![Span::raw("  ")];
    for (i, (key, label, section)) in sections.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("   │   ", Style::default().fg(theme::text_dim())));
        }
        let active = *section;
        if active {
            spans.push(Span::styled(
                key.to_string(),
                Style::default()
                    .fg(theme::aws_orange())
                    .add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::styled(
                format!(" {} ", label),
                Style::default()
                    .fg(Color::Black)
                    .bg(theme::aws_orange())
                    .add_modifier(Modifier::BOLD),
            ));
        } else {
            spans.push(Span::styled(
                key.to_string(),
                Style::default().fg(theme::text_dim()),
            ));
            spans.push(Span::styled(
                format!(" {} ", label),
                Style::default().fg(crate::ui::theme::text_muted()),
            ));
        }
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), chunks[2]);
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

// ── CodeCommit Pull Request split pane ─────────────────────────────────────────

pub fn cc_pr_section_lines(
    pr: &crate::aws::services::code::CodeCommitPullRequest,
    section: crate::aws::services::code::CcPrDetailSection,
    approvals_state: Option<&Lazy<crate::aws::services::code::CcPrApprovals>>,
    events_state: Option<&Lazy<Vec<crate::aws::services::code::CcPrEvent>>>,
    comments_state: Option<&Lazy<crate::aws::services::code::CcPrComments>>,
    diff_state: Option<&Lazy<crate::aws::services::code::CcPrDiff>>,
) -> Vec<(String, String)> {
    use crate::aws::services::code::CcPrDetailSection as S;
    match section {
        S::Overview => {
            let mut rows = vec![
                ("Repository".to_string(), pr.repo.clone()),
                ("Pull Request".to_string(), format!("#{}", pr.pr_id)),
                ("Title".to_string(), pr.title.clone()),
                ("Status".to_string(), pr.status_label().to_string()),
                ("Author".to_string(), pr.author.clone()),
                ("Created".to_string(), pr.created.clone()),
                ("Last Activity".to_string(), pr.last_activity.clone()),
                ("".to_string(), "".to_string()),
                ("Branches".to_string(), "".to_string()),
                (
                    "  Source → Destination".to_string(),
                    format!("{} → {}", pr.source_ref, pr.dest_ref),
                ),
                (
                    "  Source Tip".to_string(),
                    pr.source_commit.chars().take(8).collect(),
                ),
                (
                    "  Merge Base".to_string(),
                    pr.merge_base.chars().take(8).collect(),
                ),
            ];
            if pr.is_merged || !pr.merged_by.is_empty() {
                rows.push(("".to_string(), "".to_string()));
                rows.push(("Merge".to_string(), "".to_string()));
                rows.push(("  Merged".to_string(), if pr.is_merged { "✓ yes" } else { "no" }.to_string()));
                if !pr.merged_by.is_empty() {
                    rows.push(("  Merged By".to_string(), pr.merged_by.clone()));
                }
                if !pr.merge_option.is_empty() {
                    rows.push(("  Strategy".to_string(), pr.merge_option.clone()));
                }
                if !pr.merge_commit_id.is_empty() {
                    rows.push((
                        "  Merge Commit".to_string(),
                        pr.merge_commit_id.chars().take(8).collect(),
                    ));
                }
            }
            if !pr.description.is_empty() {
                rows.push(("".to_string(), "".to_string()));
                rows.push(("Description".to_string(), "".to_string()));
                for line in pr.description.lines().take(20) {
                    rows.push((format!("  {}", line), "".to_string()));
                }
            }
            rows.push(("".to_string(), "".to_string()));
            rows.push(("Approvals".to_string(), "".to_string()));
            match approvals_state {
                None | Some(Lazy::Loading) => {
                    rows.push(("".to_string(), "Loading…".to_string()));
                }
                Some(Lazy::Error(e)) => rows.extend(error_rows(e)),
                Some(Lazy::Loaded(a)) => {
                    if let Some(e) = &a.eval_error {
                        rows.extend(error_rows(e));
                    } else if pr.approval_rule_names.is_empty() {
                        rows.push(("  · no approval rules".to_string(), "".to_string()));
                    } else {
                        rows.push((
                            "  Approved".to_string(),
                            if a.approved {
                                "✓ yes".to_string()
                            } else if a.overridden {
                                "⚠ overridden".to_string()
                            } else {
                                "✗ not yet".to_string()
                            },
                        ));
                        for r in &a.satisfied {
                            rows.push((format!("  {}", r), "✓ satisfied".to_string()));
                        }
                        for r in &a.not_satisfied {
                            rows.push((format!("  {}", r), "✗ not satisfied".to_string()));
                        }
                    }
                    if let Some(e) = &a.states_error {
                        rows.extend(error_rows(e));
                    } else {
                        for (user, state) in &a.approvals {
                            rows.push((format!("  {}", user), state.clone()));
                        }
                    }
                }
            }
            rows
        }
        S::Activity => {
            let mut rows = Vec::new();
            match events_state {
                None | Some(Lazy::Loading) => {
                    rows.push(("".to_string(), "Loading…".to_string()));
                }
                Some(Lazy::Error(e)) => rows.extend(error_rows(e)),
                Some(Lazy::Loaded(events)) => {
                    rows.push((format!("Activity ({})", events.len()), "".to_string()));
                    if events.is_empty() {
                        rows.push(("".to_string(), "No events".to_string()));
                    }
                    for e in events {
                        let mut v = e.kind.clone();
                        if !e.detail.is_empty() {
                            v.push_str(&format!(" · {}", e.detail));
                        }
                        if !e.actor.is_empty() {
                            v.push_str(&format!(" · {}", e.actor));
                        }
                        rows.push((format!("  {}", e.date), v));
                    }
                }
            }
            rows
        }
        S::Comments => {
            let mut rows = Vec::new();
            match comments_state {
                None | Some(Lazy::Loading) => {
                    rows.push(("".to_string(), "Loading…".to_string()));
                }
                Some(Lazy::Error(e)) => rows.extend(error_rows(e)),
                Some(Lazy::Loaded(c)) => {
                    rows.push((
                        format!(
                            "Comments ({} in {} thread(s))",
                            c.total_comments,
                            c.threads.len()
                        ),
                        "".to_string(),
                    ));
                    if c.threads.is_empty() {
                        rows.push(("".to_string(), "No comments".to_string()));
                    }
                    for t in &c.threads {
                        rows.push(("".to_string(), "".to_string()));
                        if t.path.is_empty() {
                            rows.push(("General".to_string(), "".to_string()));
                        } else if t.line.is_empty() {
                            rows.push((t.path.clone(), "".to_string()));
                        } else {
                            rows.push((format!("{}:{}", t.path, t.line), "".to_string()));
                        }
                        for cm in &t.comments {
                            if cm.deleted {
                                rows.push(("  · (comment deleted)".to_string(), "".to_string()));
                                continue;
                            }
                            rows.push((
                                format!("  {} · {}", cm.author, cm.date),
                                "".to_string(),
                            ));
                            for line in cm.content.lines() {
                                rows.push((format!("    {}", line), "".to_string()));
                            }
                        }
                    }
                    if c.truncated {
                        rows.push(("".to_string(), "".to_string()));
                        rows.push((
                            format!(
                                "  · thread list truncated at {}",
                                crate::aws::services::code::MAX_PR_COMMENT_THREADS
                            ),
                            "".to_string(),
                        ));
                    }
                }
            }
            rows
        }
        S::Changes => {
            let mut rows = Vec::new();
            match diff_state {
                None | Some(Lazy::Loading) => {
                    rows.push(("".to_string(), "Loading…".to_string()));
                }
                Some(Lazy::Error(e)) => rows.extend(error_rows(e)),
                Some(Lazy::Loaded(d)) => {
                    rows.push((
                        format!(
                            "Changed Files ({}{})",
                            d.entries.len(),
                            if d.truncated { "+" } else { "" }
                        ),
                        format!(
                            "{} → {}",
                            d.before.chars().take(8).collect::<String>(),
                            d.after.chars().take(8).collect::<String>()
                        ),
                    ));
                    if d.entries.is_empty() {
                        rows.push(("".to_string(), "No differences".to_string()));
                    }
                    for e in &d.entries {
                        let mark = match e.change.as_str() {
                            "A" => "+",
                            "D" => "−",
                            _ => "~",
                        };
                        let label = if !e.old_path.is_empty() && e.old_path != e.path {
                            format!("{} → {}", e.old_path, e.path)
                        } else {
                            e.path.clone()
                        };
                        rows.push((format!("  {} {}", mark, label), "".to_string()));
                    }
                    rows.push(("".to_string(), "".to_string()));
                    if d.truncated {
                        rows.push((
                            format!(
                                "  · file list truncated at {}",
                                crate::aws::services::code::MAX_PR_DIFF_FILES
                            ),
                            "".to_string(),
                        ));
                    }
                    rows.push((
                        "  · e builds the unified patch and opens it in $EDITOR".to_string(),
                        "".to_string(),
                    ));
                }
            }
            rows
        }
    }
}

pub(super) fn render_cc_pr_split(
    app: &App,
    pr: &crate::aws::services::code::CodeCommitPullRequest,
    area: ratatui::layout::Rect,
    frame: &mut Frame,
) {
    let focused = app.details_focused;
    let footer = detail_footer(
        app,
        focused,
        crate::aws::services::code::CC_PR_SECTIONS.len(),
        "",
    );
    let mut block = theme::pane_block("Pull Request", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let status_color = match pr.status_label() {
        "OPEN" => theme::warning(),
        "MERGED" => theme::success(),
        _ => theme::text_dim(),
    };
    let header_lines = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                format!("{} #{}", pr.repo, pr.pr_id),
                Style::default()
                    .fg(crate::ui::theme::text_primary())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(
                pr.status_label().to_string(),
                Style::default().fg(status_color).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(pr.title.clone(), Style::default().fg(theme::text_dim())),
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
    render_section_tab_bar(
        app,
        chunks[2],
        frame,
        &descriptor_tabs(app, &crate::aws::services::code::CC_PR_SECTIONS),
    );
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

// ── CodeBuild Project split pane ───────────────────────────────────────────────

pub fn code_build_section_lines(
    proj: &crate::aws::services::code::CodeBuildProject,
    section: CodeBuildDetailSection,
    builds_state: Option<&Lazy<Vec<crate::aws::services::code::CodeBuildBuild>>>,
    buildspec_state: Option<&Lazy<crate::aws::services::code::BuildspecResult>>,
) -> Vec<(String, String)> {
    match section {
        CodeBuildDetailSection::Details => code_build_details_lines(proj),
        CodeBuildDetailSection::Buildspec => code_build_buildspec_lines(proj, buildspec_state),
        CodeBuildDetailSection::Builds => code_build_builds_lines(builds_state),
        CodeBuildDetailSection::Environment => code_build_env_lines(proj),
        CodeBuildDetailSection::Tags => tag_rows(&proj.tags),
    }
}

pub(super) fn code_build_details_lines(
    proj: &crate::aws::services::code::CodeBuildProject,
) -> Vec<(String, String)> {
    let yesno = |b: bool| if b { "Yes" } else { "No" }.to_string();
    let mut rows: Vec<(String, String)> = vec![
        ("Name".to_string(), proj.name.clone()),
        ("ARN".to_string(), proj.arn.clone()),
    ];
    if !proj.description.is_empty() {
        rows.push(("Description".to_string(), proj.description.clone()));
    }

    // Source
    rows.push(("".to_string(), "".to_string()));
    rows.push(("Source".to_string(), "".to_string()));
    rows.push(("  Type".to_string(), proj.source_type.clone()));
    if !proj.source_location.is_empty() {
        rows.push(("  Location".to_string(), proj.source_location.clone()));
    }
    if !proj.source_version.is_empty() {
        rows.push(("  Version".to_string(), proj.source_version.clone()));
    }
    let bs = if proj.buildspec.trim().is_empty() {
        "buildspec.yml (source root)".to_string()
    } else if proj.buildspec_is_inline() {
        "inline → press 2".to_string()
    } else {
        proj.buildspec.clone()
    };
    rows.push(("  Buildspec".to_string(), bs));

    // Environment
    rows.push(("".to_string(), "".to_string()));
    rows.push(("Environment".to_string(), "".to_string()));
    rows.push(("  Type".to_string(), proj.environment_type.clone()));
    rows.push(("  Compute".to_string(), proj.compute_type.clone()));
    rows.push(("  Image".to_string(), proj.image.clone()));
    rows.push(("  Privileged".to_string(), yesno(proj.privileged_mode)));
    rows.push(("  Env Vars".to_string(), proj.env_vars.len().to_string()));

    // Artifacts
    if !proj.artifacts_type.is_empty() {
        rows.push(("".to_string(), "".to_string()));
        rows.push(("Artifacts".to_string(), "".to_string()));
        rows.push(("  Type".to_string(), proj.artifacts_type.clone()));
        if !proj.artifacts_location.is_empty() {
            rows.push(("  Location".to_string(), proj.artifacts_location.clone()));
        }
        if !proj.artifacts_name.is_empty() {
            rows.push(("  Name".to_string(), proj.artifacts_name.clone()));
        }
    }

    // Cache
    if !proj.cache_type.is_empty() && proj.cache_type != "NO_CACHE" {
        rows.push(("".to_string(), "".to_string()));
        rows.push(("Cache".to_string(), "".to_string()));
        rows.push(("  Type".to_string(), proj.cache_type.clone()));
        if !proj.cache_location.is_empty() {
            rows.push(("  Location".to_string(), proj.cache_location.clone()));
        }
    }

    // VPC
    if !proj.vpc_id.is_empty() {
        rows.push(("".to_string(), "".to_string()));
        rows.push(("VPC".to_string(), "".to_string()));
        rows.push(("  VPC".to_string(), proj.vpc_id.clone()));
        if !proj.vpc_subnets.is_empty() {
            rows.push(("  Subnets".to_string(), proj.vpc_subnets.join(", ")));
        }
        if !proj.vpc_security_groups.is_empty() {
            rows.push((
                "  Security Groups".to_string(),
                proj.vpc_security_groups.join(", "),
            ));
        }
    }

    // Logs
    rows.push(("".to_string(), "".to_string()));
    rows.push(("Logs".to_string(), "".to_string()));
    if !proj.log_cw_status.is_empty() {
        rows.push(("  CloudWatch".to_string(), proj.log_cw_status.clone()));
        if !proj.log_cw_group.is_empty() {
            rows.push(("  Group".to_string(), proj.log_cw_group.clone()));
        }
        if !proj.log_cw_stream.is_empty() {
            rows.push(("  Stream Prefix".to_string(), proj.log_cw_stream.clone()));
        }
    }
    if !proj.log_s3_status.is_empty() && proj.log_s3_status != "DISABLED" {
        rows.push(("  S3".to_string(), proj.log_s3_status.clone()));
        if !proj.log_s3_location.is_empty() {
            rows.push(("  S3 Location".to_string(), proj.log_s3_location.clone()));
        }
    }
    rows.push(("  Tail latest".to_string(), "press t".to_string()));

    // Configuration
    rows.push(("".to_string(), "".to_string()));
    rows.push(("Configuration".to_string(), "".to_string()));
    rows.push(("  Service Role".to_string(), proj.service_role.clone()));
    rows.push(("  Timeout (min)".to_string(), proj.timeout_minutes.to_string()));
    if proj.queued_timeout_minutes > 0 {
        rows.push((
            "  Queued Timeout".to_string(),
            format!("{} min", proj.queued_timeout_minutes),
        ));
    }
    if let Some(limit) = proj.concurrent_build_limit {
        rows.push(("  Concurrent Limit".to_string(), limit.to_string()));
    }
    rows.push(("  Badge".to_string(), yesno(proj.badge_enabled)));
    if !proj.created.is_empty() {
        rows.push(("  Created".to_string(), proj.created.clone()));
    }
    rows.push(("  Last Modified".to_string(), proj.last_modified.clone()));

    rows
}

pub(super) fn code_build_buildspec_lines(
    proj: &crate::aws::services::code::CodeBuildProject,
    buildspec_state: Option<&Lazy<crate::aws::services::code::BuildspecResult>>,
) -> Vec<(String, String)> {
    // Inline buildspec — render it directly.
    if proj.buildspec_is_inline() {
        return buildspec_body_lines("", &proj.buildspec);
    }

    // File-path (or default) buildspec — pulled from the source repo lazily.
    let path = if proj.buildspec.trim().is_empty() {
        "buildspec.yml".to_string()
    } else {
        proj.buildspec.clone()
    };
    use crate::aws::services::code::BuildspecResult;
    match buildspec_state {
        None | Some(Lazy::Loading) => vec![
            ("Buildspec file".to_string(), path),
            ("".to_string(), "".to_string()),
            ("".to_string(), "Fetching from source repo…".to_string()),
        ],
        Some(Lazy::Loaded(BuildspecResult::Content(text))) => {
            let mut rows = vec![("Buildspec file".to_string(), path), ("".to_string(), "".to_string())];
            rows.extend(buildspec_body_lines(
                "from repo",
                text,
            ));
            rows
        }
        Some(Lazy::Loaded(BuildspecResult::NotFound)) => vec![
            ("Buildspec file".to_string(), path),
            ("".to_string(), "".to_string()),
            ("⚠".to_string(), "Not found in the source repo".to_string()),
        ],
        Some(Lazy::Loaded(BuildspecResult::Unsupported(note))) => vec![
            ("Buildspec file".to_string(), path),
            ("".to_string(), "".to_string()),
            ("".to_string(), note.clone()),
        ],
        Some(Lazy::Error(e)) => {
            let mut rows = vec![("Buildspec file".to_string(), path)];
            rows.extend(error_rows(e));
            rows
        }
    }
}

pub(super) fn buildspec_body_lines(hint: &str, content: &str) -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = vec![
        ("".to_string(), hint.to_string()),
        ("".to_string(), "".to_string()),
    ];
    // Plain content rows (leading-space key) preserve the YAML's own indentation.
    for line in content.lines() {
        rows.push((format!(" {}", line), "".to_string()));
    }
    rows
}

pub(super) fn code_build_builds_lines(builds_state: Option<&Lazy<Vec<crate::aws::services::code::CodeBuildBuild>>>) -> Vec<(String, String)> {
    code_build_builds_lines_indexed(builds_state).0
}

/// Build the Builds-section rows and, alongside, the body-line index at which
/// each build's block starts — so `t` can map the detail cursor to a build.
pub fn code_build_builds_lines_indexed(
    builds_state: Option<&Lazy<Vec<crate::aws::services::code::CodeBuildBuild>>>,
) -> (Vec<(String, String)>, Vec<usize>) {
    match builds_state {
        None | Some(Lazy::Loading) => {
            (vec![("".to_string(), "Loading builds…".to_string())], vec![])
        }
        Some(Lazy::Error(e)) => (
            error_rows(e),
            vec![],
        ),
        Some(Lazy::Loaded(builds)) => {
            if builds.is_empty() {
                return (vec![("".to_string(), "No builds found".to_string())], vec![]);
            }
            let mut rows: Vec<(String, String)> = vec![
                (
                    "".to_string(),
                    "j/k to a build · t tails its CloudWatch logs".to_string(),
                ),
                ("".to_string(), "".to_string()),
            ];
            let mut starts: Vec<usize> = Vec::with_capacity(builds.len());
            for (i, b) in builds.iter().enumerate() {
                if i > 0 {
                    rows.push(("".to_string(), "".to_string()));
                }
                starts.push(rows.len());
                let duration = if b.duration_secs > 0 {
                    format!("{}m {}s", b.duration_secs / 60, b.duration_secs % 60)
                } else {
                    "—".to_string()
                };
                // Group header (empty value, no leading space) so `[[`/`]]`
                // anchor on it and jump build-to-build; status rides in the
                // header label so pass/fail shows at a glance while jumping.
                rows.push((
                    format!("Build #{} — {}", b.build_number, b.status),
                    "".to_string(),
                ));
                rows.push(("  Started".to_string(), b.start_time.clone()));
                if !b.end_time.is_empty() {
                    rows.push(("  Ended".to_string(), b.end_time.clone()));
                }
                rows.push(("  Duration".to_string(), duration));
                if !b.initiator.is_empty() {
                    rows.push(("  Initiator".to_string(), b.initiator.clone()));
                }
                if !b.source_version.is_empty() {
                    rows.push(("  Source Version".to_string(), b.source_version.clone()));
                }
                if !b.resolved_source_version.is_empty()
                    && b.resolved_source_version != b.source_version
                {
                    rows.push(("  Resolved".to_string(), b.resolved_source_version.clone()));
                }
                if !b.log_group.is_empty() {
                    rows.push(("  Log Group".to_string(), b.log_group.clone()));
                    rows.push(("  Log Stream".to_string(), b.log_stream.clone()));
                }
                // Phases
                if !b.phases.is_empty() {
                    rows.push(("  Phases".to_string(), "".to_string()));
                    for p in &b.phases {
                        if p.phase_type.is_empty() {
                            continue;
                        }
                        let dur = if p.duration_secs > 0 {
                            format!("{}s", p.duration_secs)
                        } else {
                            "—".to_string()
                        };
                        rows.push((
                            format!("    {}", p.phase_type),
                            format!("{} ({})", p.status, dur),
                        ));
                    }
                }
            }
            (rows, starts)
        }
    }
}

pub(super) fn code_build_env_lines(proj: &crate::aws::services::code::CodeBuildProject) -> Vec<(String, String)> {
    if proj.env_vars.is_empty() {
        return vec![("".to_string(), "No environment variables".to_string())];
    }
    // PARAMETER_STORE / SECRETS_MANAGER values are references (param name /
    // secret id), not the secret itself — safe to display, tagged by source.
    proj.env_vars
        .iter()
        .map(|v| {
            let label = match v.var_type.as_str() {
                "PARAMETER_STORE" => format!("{} (SSM)", v.name),
                "SECRETS_MANAGER" => format!("{} (Secret)", v.name),
                _ => v.name.clone(),
            };
            (label, v.value.clone())
        })
        .collect()
}

pub(super) fn render_code_build_split(
    app: &App,
    proj: &crate::aws::services::code::CodeBuildProject,
    area: ratatui::layout::Rect,
    frame: &mut Frame,
) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 5, "");
    let mut block = theme::pane_block("Build Project", focused);
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
                proj.name.clone(),
                Style::default()
                    .fg(crate::ui::theme::text_primary())
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                format!("{} · {}", proj.source_type, proj.compute_type),
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

    let sections = descriptor_tabs(app, &crate::aws::services::code::CODE_BUILD_SECTIONS);
    let mut spans: Vec<Span> = vec![Span::raw("  ")];
    for (i, (key, label, section)) in sections.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("   │   ", Style::default().fg(theme::text_dim())));
        }
        let active = *section;
        if active {
            spans.push(Span::styled(
                key.to_string(),
                Style::default()
                    .fg(theme::aws_orange())
                    .add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::styled(
                format!(" {} ", label),
                Style::default()
                    .fg(Color::Black)
                    .bg(theme::aws_orange())
                    .add_modifier(Modifier::BOLD),
            ));
        } else {
            spans.push(Span::styled(
                key.to_string(),
                Style::default().fg(theme::text_dim()),
            ));
            spans.push(Span::styled(
                format!(" {} ", label),
                Style::default().fg(crate::ui::theme::text_muted()),
            ));
        }
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), chunks[2]);
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

// ── CodePipeline split pane ────────────────────────────────────────────────────

pub fn code_pipeline_section_lines(
    p: &crate::aws::services::code::CodePipeline,
    section: CodePipelineDetailSection,
    details_state: Option<&Lazy<crate::aws::services::code::CodePipelineDetails>>,
    executions: &[&crate::aws::services::code::CodePipelineExecution],
) -> Vec<(String, String)> {
    match section {
        CodePipelineDetailSection::Stages => code_pipeline_stages_lines(p, details_state),
        CodePipelineDetailSection::Executions => code_pipeline_executions_lines(executions),
        // `ListPipelines` returns no tags — they ride the details fetch.
        CodePipelineDetailSection::Tags => match details_state {
            None | Some(Lazy::Loading) => vec![("".to_string(), "Loading…".to_string())],
            Some(Lazy::Error(e)) => error_rows(e),
            Some(Lazy::Loaded(d)) => match &d.tags_error {
                Some(err) => error_rows(err),
                None => tag_rows(&d.tags),
            },
        },
    }
}

pub(super) fn code_pipeline_stages_lines(
    p: &crate::aws::services::code::CodePipeline,
    details_state: Option<&Lazy<crate::aws::services::code::CodePipelineDetails>>,
) -> Vec<(String, String)> {
    let mut rows = vec![
        ("Name".to_string(), p.name.clone()),
        ("Type".to_string(), p.pipeline_type.clone()),
        ("Version".to_string(), p.version.to_string()),
        ("Created".to_string(), p.created.clone()),
        ("Updated".to_string(), p.updated.clone()),
    ];

    match details_state {
        None | Some(Lazy::Loading) => {
            rows.push(("".to_string(), "".to_string()));
            rows.push(("".to_string(), "Loading pipeline…".to_string()));
        }
        Some(Lazy::Error(e)) => {
            rows.extend(error_rows(e));
        }
        Some(Lazy::Loaded(details)) => {
            if !details.execution_mode.is_empty() {
                rows.push(("Execution Mode".to_string(), details.execution_mode.clone()));
            }
            if !details.role_arn.is_empty() {
                rows.push(("Role ARN".to_string(), details.role_arn.clone()));
            }
            // Artifact store
            if !details.artifact_store_location.is_empty() {
                rows.push(("".to_string(), "".to_string()));
                rows.push(("Artifact Store".to_string(), "".to_string()));
                rows.push(("  Type".to_string(), details.artifact_store_type.clone()));
                rows.push(("  Location".to_string(), details.artifact_store_location.clone()));
                if !details.artifact_store_kms.is_empty() {
                    rows.push(("  KMS Key".to_string(), details.artifact_store_kms.clone()));
                }
            }
            rows.push(("".to_string(), "".to_string()));
            rows.push(("Stages".to_string(), "".to_string()));
            for stage in &details.stages {
                let status_str = if stage.status.is_empty() {
                    String::new()
                } else {
                    format!(" [{}]", stage.status)
                };
                rows.push(("".to_string(), "".to_string()));
                rows.push((format!("  ▸ {}{}", stage.name, status_str), "".to_string()));
                for action in &stage.actions {
                    let order = action
                        .run_order
                        .map(|o| format!("  ·  run order {}", o))
                        .unwrap_or_default();
                    rows.push((
                        format!("    {}", action.name),
                        format!("{} / {}{}", action.category, action.provider, order),
                    ));
                    // Jumpable target row (the key is read by code_row_jump_target).
                    if let Some((label, value)) = &action.target {
                        rows.push((format!("      {}", label), value.clone()));
                    }
                    if !action.region.is_empty() {
                        rows.push(("      Region".to_string(), action.region.clone()));
                    }
                    if !action.namespace.is_empty() {
                        rows.push(("      Namespace".to_string(), action.namespace.clone()));
                    }
                    if !action.role_arn.is_empty() {
                        rows.push(("      Role".to_string(), action.role_arn.clone()));
                    }
                    if !action.input_artifacts.is_empty() {
                        rows.push((
                            "      Input".to_string(),
                            action.input_artifacts.join(", "),
                        ));
                    }
                    if !action.output_artifacts.is_empty() {
                        rows.push((
                            "      Output".to_string(),
                            action.output_artifacts.join(", "),
                        ));
                    }
                    // The rest of the action configuration verbatim — this is
                    // where a CFN template path or an ECS file name lives.
                    for (k, v) in &action.configuration {
                        rows.push((format!("      {}", k), v.clone()));
                    }
                }
            }
        }
    }
    rows
}

/// The pipeline's runs, filtered from sibling rows already in `resources` (the
/// Executions sub-tab loaded them) — no fetch, and every `Execution` row jumps
/// into that run's own pane.
pub(super) fn code_pipeline_executions_lines(
    executions: &[&crate::aws::services::code::CodePipelineExecution],
) -> Vec<(String, String)> {
    if executions.is_empty() {
        return vec![
            ("".to_string(), "No recent executions".to_string()),
            (
                "".to_string(),
                "  · runs are listed on the Executions sub-tab (6)".to_string(),
            ),
        ];
    }

    let running = executions.iter().filter(|e| e.is_running()).count();
    let failed = executions.iter().filter(|e| e.failed()).count();
    let mut rows = vec![(
        format!(
            "Recent Executions ({}{}{})",
            executions.len(),
            if running > 0 {
                format!(", {} running", running)
            } else {
                String::new()
            },
            if failed > 0 {
                format!(", {} failed", failed)
            } else {
                String::new()
            }
        ),
        String::new(),
    )];

    for ex in executions {
        rows.push(("".to_string(), "".to_string()));
        // Group header (empty value) so `[[`/`]]` jump execution-to-execution.
        rows.push((format!("{} — {}", ex.execution_id, ex.status), String::new()));
        rows.push(("  Execution".to_string(), ex.execution_id.clone()));
        rows.push(("  Started".to_string(), ex.start_time.clone()));
        if ex.duration_secs > 0 {
            rows.push(("  Duration".to_string(), ex.duration_label()));
        }
        if !ex.trigger.is_empty() {
            let t = if ex.trigger_detail.is_empty() {
                ex.trigger.clone()
            } else {
                format!("{} · {}", ex.trigger, ex.trigger_detail)
            };
            rows.push(("  Trigger".to_string(), t));
        }
        for rev in &ex.source_revisions {
            rows.push(("  Revision".to_string(), rev.clone()));
        }
    }
    rows
}

// ── CodePipeline execution pane ────────────────────────────────────────────────

pub(super) fn render_code_exec_split(
    app: &App,
    exec: &crate::aws::services::code::CodePipelineExecution,
    area: Rect,
    frame: &mut Frame,
) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 3, "");

    let mut block = theme::pane_block("Execution", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let header_lines = build_code_exec_header_lines(exec);
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
        &descriptor_tabs(app, &crate::aws::services::code::CODE_EXEC_SECTIONS),
    );
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub(super) fn build_code_exec_header_lines(
    exec: &crate::aws::services::code::CodePipelineExecution,
) -> Vec<Line<'static>> {
    let mut lines = vec![Line::raw("")];

    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            exec.pipeline_name.clone(),
            Style::default()
                .fg(theme::text_primary())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  /  {}", exec.execution_id),
            Style::default().fg(theme::text_dim()),
        ),
    ]));

    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            exec.status.clone(),
            Style::default()
                .fg(theme::state_indicator(
                    &crate::aws::services::code::pipeline_execution_state(&exec.status),
                )
                .1)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  ·  {}", exec.duration_label()),
            Style::default().fg(theme::text_dim()),
        ),
    ]));

    lines.push(Line::raw(""));
    lines
}

pub fn code_exec_section_lines(
    exec: &crate::aws::services::code::CodePipelineExecution,
    section: crate::aws::services::code::CodeExecDetailSection,
    actions_state: Option<&Lazy<Vec<crate::aws::services::code::PipelineActionExecution>>>,
) -> Vec<(String, String)> {
    use crate::aws::services::code::CodeExecDetailSection as S;
    match section {
        S::Overview => code_exec_overview_lines(exec, actions_state),
        S::Actions => code_exec_actions_lines_indexed(actions_state).0,
        S::Artifacts => code_exec_artifacts_lines(exec, actions_state),
    }
}

pub(super) fn code_exec_overview_lines(
    exec: &crate::aws::services::code::CodePipelineExecution,
    actions_state: Option<&Lazy<Vec<crate::aws::services::code::PipelineActionExecution>>>,
) -> Vec<(String, String)> {
    let mut rows = vec![
        ("Pipeline".to_string(), exec.pipeline_name.clone()),
        ("Execution".to_string(), exec.execution_id.clone()),
        ("Status".to_string(), exec.status.clone()),
    ];
    if !exec.status_summary.is_empty() {
        rows.push(("Summary".to_string(), exec.status_summary.clone()));
    }
    rows.push(("Started".to_string(), exec.start_time.clone()));
    if exec.duration_secs > 0 {
        rows.push(("Duration".to_string(), exec.duration_label()));
    }
    if !exec.trigger.is_empty() {
        rows.push(("Trigger".to_string(), exec.trigger.clone()));
    }
    if !exec.trigger_detail.is_empty() {
        rows.push(("Trigger Detail".to_string(), exec.trigger_detail.clone()));
    }
    if !exec.execution_mode.is_empty() {
        rows.push(("Mode".to_string(), exec.execution_mode.clone()));
    }
    if !exec.execution_type.is_empty() && exec.execution_type != "STANDARD" {
        rows.push(("Type".to_string(), exec.execution_type.clone()));
    }
    if !exec.rollback_target.is_empty() {
        rows.push(("Rolled Back To".to_string(), exec.rollback_target.clone()));
    }
    if !exec.stop_reason.is_empty() {
        rows.push(("Stop Reason".to_string(), exec.stop_reason.clone()));
    }

    if !exec.source_revisions.is_empty() {
        rows.push(("".to_string(), "".to_string()));
        rows.push(("Source Revisions".to_string(), "".to_string()));
        for rev in &exec.source_revisions {
            rows.push((" ".to_string() + rev, "".to_string()));
        }
    }

    // A one-line verdict per stage once the action detail lands, so Overview
    // answers "where did it stop" without switching sections.
    match actions_state {
        None | Some(Lazy::Loading) => {
            rows.push(("".to_string(), "".to_string()));
            rows.push(("".to_string(), "Loading…".to_string()));
        }
        Some(Lazy::Error(e)) => rows.extend(error_rows(e)),
        Some(Lazy::Loaded(actions)) if !actions.is_empty() => {
            rows.push(("".to_string(), "".to_string()));
            rows.push(("Stages".to_string(), "".to_string()));
            let mut seen: Vec<&str> = Vec::new();
            for a in actions.iter() {
                if seen.contains(&a.stage_name.as_str()) {
                    continue;
                }
                seen.push(&a.stage_name);
                let in_stage: Vec<_> = actions
                    .iter()
                    .filter(|x| x.stage_name == a.stage_name)
                    .collect();
                let failed = in_stage.iter().filter(|x| x.failed()).count();
                let status = if failed > 0 {
                    format!("✗ {} of {} actions failed", failed, in_stage.len())
                } else if in_stage.iter().any(|x| x.status == "InProgress") {
                    "⚠ in progress".to_string()
                } else {
                    format!("✓ {} actions", in_stage.len())
                };
                rows.push((format!("  {}", a.stage_name), status));
            }
        }
        Some(Lazy::Loaded(_)) => {
            rows.push(("".to_string(), "".to_string()));
            rows.push((
                "".to_string(),
                "No action executions recorded for this run".to_string(),
            ));
        }
    }
    rows
}

/// The Actions body plus, for each action, the row index its block starts at —
/// so `t` can map the detail cursor onto the action whose logs to tail (the
/// `code_build_builds_lines_indexed` shape).
pub fn code_exec_actions_lines_indexed(
    actions_state: Option<&Lazy<Vec<crate::aws::services::code::PipelineActionExecution>>>,
) -> (Vec<(String, String)>, Vec<usize>) {
    let mut starts = Vec::new();
    let actions = match actions_state {
        None | Some(Lazy::Loading) => {
            return (vec![("".to_string(), "Loading…".to_string())], starts)
        }
        Some(Lazy::Error(e)) => return (error_rows(e), starts),
        Some(Lazy::Loaded(a)) => a,
    };
    if actions.is_empty() {
        return (
            vec![(
                "".to_string(),
                "No action executions recorded for this run".to_string(),
            )],
            starts,
        );
    }

    let mut rows: Vec<(String, String)> = Vec::new();
    let mut current_stage = "";
    for a in actions {
        if a.stage_name != current_stage {
            current_stage = &a.stage_name;
            rows.push(("".to_string(), "".to_string()));
            rows.push((format!("▸ {}", a.stage_name), String::new()));
        }
        starts.push(rows.len());
        rows.push((format!("  {}", a.action_name), a.status.clone()));
        rows.push((
            "    Provider".to_string(),
            format!("{} / {}", a.category, a.provider),
        ));
        if let Some((label, value)) = &a.target {
            rows.push((format!("    {}", label), value.clone()));
        }
        if !a.external_execution_id.is_empty() {
            // For CodeBuild this is `<project>:<build-uuid>` — the row jumps to
            // the project, whose Builds section holds this very build.
            let label = if a.provider == "CodeBuild" {
                "    Build ID"
            } else {
                "    External ID"
            };
            rows.push((label.to_string(), a.external_execution_id.clone()));
        }
        if !a.external_execution_summary.is_empty() {
            rows.push((
                "    Summary".to_string(),
                a.external_execution_summary.clone(),
            ));
        }
        if !a.error_code.is_empty() {
            rows.push(("    Error".to_string(), a.error_code.clone()));
        }
        if !a.error_message.is_empty() {
            rows.push(("".to_string(), "".to_string()));
            push_wrapped_content(&mut rows, &format!("✗ {}", a.error_message), 12);
        }
        if !a.log_stream_arn.is_empty() {
            rows.push(("    Log Stream".to_string(), a.log_stream_arn.clone()));
            rows.push((
                "".to_string(),
                "  · t to tail this action's logs".to_string(),
            ));
        }
        if !a.external_execution_url.is_empty() {
            rows.push(("    Console".to_string(), a.external_execution_url.clone()));
        }
        if !a.start_time.is_empty() {
            rows.push(("    Started".to_string(), a.start_time.clone()));
        }
        if a.duration_secs > 0 {
            let d = a.duration_secs;
            rows.push((
                "    Duration".to_string(),
                if d >= 3600 {
                    format!("{}h {}m", d / 3600, (d % 3600) / 60)
                } else {
                    format!("{}m {}s", d / 60, d % 60)
                },
            ));
        }
        if !a.region.is_empty() {
            rows.push(("    Region".to_string(), a.region.clone()));
        }
        if !a.namespace.is_empty() {
            rows.push(("    Namespace".to_string(), a.namespace.clone()));
        }
        if !a.updated_by.is_empty() {
            rows.push(("    Updated By".to_string(), a.updated_by.clone()));
        }
        if !a.role_arn.is_empty() {
            rows.push(("    Role".to_string(), a.role_arn.clone()));
        }
        for (k, v) in &a.configuration {
            rows.push((format!("    {}", k), v.clone()));
        }
    }
    (rows, starts)
}

pub(super) fn code_exec_artifacts_lines(
    exec: &crate::aws::services::code::CodePipelineExecution,
    actions_state: Option<&Lazy<Vec<crate::aws::services::code::PipelineActionExecution>>>,
) -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = Vec::new();

    if !exec.source_revisions.is_empty() {
        rows.push(("Source Revisions".to_string(), "".to_string()));
        for rev in &exec.source_revisions {
            rows.push((" ".to_string() + rev, "".to_string()));
        }
        rows.push(("".to_string(), "".to_string()));
    }

    let actions = match actions_state {
        None | Some(Lazy::Loading) => {
            rows.push(("".to_string(), "Loading…".to_string()));
            return rows;
        }
        Some(Lazy::Error(e)) => {
            rows.extend(error_rows(e));
            return rows;
        }
        Some(Lazy::Loaded(a)) => a,
    };

    let mut any = false;
    for a in actions {
        if a.input_artifacts.is_empty()
            && a.output_artifacts.is_empty()
            && a.output_variables.is_empty()
        {
            continue;
        }
        any = true;
        rows.push((
            format!("{} / {}", a.stage_name, a.action_name),
            String::new(),
        ));
        if !a.input_artifacts.is_empty() {
            rows.push(("  Input".to_string(), a.input_artifacts.join(", ")));
        }
        if !a.output_artifacts.is_empty() {
            rows.push(("  Output".to_string(), a.output_artifacts.join(", ")));
        }
        // Namespaced output variables — what a downstream action actually read.
        for (k, v) in &a.output_variables {
            rows.push((format!("  #{}", k), v.clone()));
        }
        rows.push(("".to_string(), "".to_string()));
    }
    if !any {
        rows.push(("".to_string(), "No artifacts or output variables".to_string()));
    }
    rows
}

pub(super) fn render_code_pipeline_split(
    app: &App,
    resource: &dyn Resource,
    area: ratatui::layout::Rect,
    frame: &mut Frame,
) {
    let p = match resource
        .as_any()
        .downcast_ref::<crate::aws::services::code::CodePipeline>()
    {
        Some(p) => p,
        None => return,
    };

    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 3, "");
    let mut block = theme::pane_block("Pipeline", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Header
    let header_lines = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                p.name.clone(),
                Style::default()
                    .fg(crate::ui::theme::text_primary())
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                format!("{} · v{}", p.pipeline_type, p.version),
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

    // Section tabs
    let sections = descriptor_tabs(app, &crate::aws::services::code::CODE_PIPELINE_SECTIONS);
    let mut spans: Vec<Span> = vec![Span::raw("  ")];
    for (i, (key, label, section)) in sections.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("   │   ", Style::default().fg(theme::text_dim())));
        }
        let active = *section;
        if active {
            spans.push(Span::styled(
                key.to_string(),
                Style::default()
                    .fg(theme::aws_orange())
                    .add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::styled(
                format!(" {} ", label),
                Style::default()
                    .fg(Color::Black)
                    .bg(theme::aws_orange())
                    .add_modifier(Modifier::BOLD),
            ));
        } else {
            spans.push(Span::styled(
                key.to_string(),
                Style::default().fg(theme::text_dim()),
            ));
            spans.push(Span::styled(
                format!(" {} ", label),
                Style::default().fg(crate::ui::theme::text_muted()),
            ));
        }
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), chunks[2]);
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

// ── CodeDeploy Deployment Group split pane ─────────────────────────────────────

/// Prefix a CodeDeploy status with a ✓/✗/⚠ glyph so `style_detail_row` colours it.
pub(super) fn codedeploy_status_value(status: &str) -> String {
    match status {
        "Succeeded" => format!("✓ {}", status),
        "Failed" | "Stopped" => format!("✗ {}", status),
        "" => "—".to_string(),
        other => format!("⚠ {}", other),
    }
}

pub(super) fn deployment_ref_rows(
    label: &str,
    dref: &Option<crate::aws::services::code::DeploymentRef>,
    rows: &mut Vec<(String, String)>,
) {
    rows.push((label.to_string(), String::new()));
    match dref {
        Some(d) => {
            rows.push(("  Deployment".to_string(), d.id.clone()));
            rows.push(("  Status".to_string(), codedeploy_status_value(&d.status)));
            if !d.create_time.is_empty() {
                rows.push(("  Created".to_string(), d.create_time.clone()));
            }
            if !d.end_time.is_empty() {
                rows.push(("  Ended".to_string(), d.end_time.clone()));
            }
        }
        None => rows.push(("  ".to_string(), String::new())),
    }
}

pub fn code_deploy_group_section_lines(
    g: &crate::aws::services::code::CodeDeployGroup,
    section: CodeDeployGroupDetailSection,
    deployments_state: Option<&Lazy<Vec<crate::aws::services::code::CodeDeployDeployment>>>,
) -> Vec<(String, String)> {
    match section {
        CodeDeployGroupDetailSection::Overview => {
            let mut rows = vec![
                ("Application".to_string(), g.app_name.clone()),
                ("Deployment Group".to_string(), g.group_name.clone()),
                ("Group ID".to_string(), g.group_id.clone()),
                ("Compute Platform".to_string(), g.compute_platform.clone()),
                ("Deployment Config".to_string(), g.deployment_config.clone()),
            ];
            if !g.deployment_type.is_empty() {
                rows.push(("Deployment Type".to_string(), g.deployment_type.clone()));
            }
            if !g.deployment_option.is_empty() {
                rows.push(("Traffic Control".to_string(), g.deployment_option.clone()));
            }
            if !g.service_role.is_empty() {
                rows.push(("Service Role".to_string(), g.service_role.clone()));
            }
            rows.push((String::new(), String::new()));
            rows.push((
                "Auto Rollback".to_string(),
                if g.auto_rollback_enabled {
                    "✓ Enabled".to_string()
                } else {
                    "Disabled".to_string()
                },
            ));
            if g.auto_rollback_enabled && !g.auto_rollback_events.is_empty() {
                rows.push(("  On Events".to_string(), g.auto_rollback_events.join(", ")));
            }
            rows.push((String::new(), String::new()));
            deployment_ref_rows("Last Attempted", &g.last_attempted, &mut rows);
            rows.push((String::new(), String::new()));
            deployment_ref_rows("Last Successful", &g.last_successful, &mut rows);
            rows
        }
        CodeDeployGroupDetailSection::Targets => {
            let mut rows: Vec<(String, String)> = Vec::new();
            if !g.ec2_tag_filters.is_empty() {
                rows.push(("EC2 Tag Filters".to_string(), String::new()));
                for f in &g.ec2_tag_filters {
                    rows.push((format!("  {}", f), String::new()));
                }
                rows.push((String::new(), String::new()));
            }
            if !g.asg_names.is_empty() {
                rows.push(("Auto Scaling Groups".to_string(), String::new()));
                for a in &g.asg_names {
                    rows.push((format!("  {}", a), String::new()));
                }
                rows.push((String::new(), String::new()));
            }
            if !g.ecs_services.is_empty() {
                rows.push(("ECS Services".to_string(), String::new()));
                for s in &g.ecs_services {
                    rows.push((format!("  {}", s), String::new()));
                }
                rows.push((String::new(), String::new()));
            }
            if rows.is_empty() {
                rows.push((
                    String::new(),
                    "No EC2 / ASG / ECS targets (on-premises or custom deployment)".to_string(),
                ));
            }
            rows
        }
        CodeDeployGroupDetailSection::Deployments => match deployments_state {
            None | Some(Lazy::Loading) => {
                vec![("".to_string(), "Loading recent deployments…".to_string())]
            }
            Some(Lazy::Error(e)) => error_rows(e),
            Some(Lazy::Loaded(deployments)) => {
                if deployments.is_empty() {
                    return vec![("".to_string(), "No deployments for this group".to_string())];
                }
                let mut rows: Vec<(String, String)> = Vec::new();
                for (i, d) in deployments.iter().enumerate() {
                    if i > 0 {
                        rows.push((String::new(), String::new()));
                    }
                    // Group header (status rides in the label for at-a-glance scan).
                    rows.push((format!("{} — {}", d.deployment_id, d.status), String::new()));
                    if !d.create_time.is_empty() {
                        rows.push(("  Created".to_string(), d.create_time.clone()));
                    }
                    if !d.complete_time.is_empty() {
                        rows.push(("  Completed".to_string(), d.complete_time.clone()));
                    }
                    // Instance / target summary counts (only non-zero).
                    let mut counts: Vec<String> = Vec::new();
                    if d.succeeded > 0 {
                        counts.push(format!("✓{} ok", d.succeeded));
                    }
                    if d.failed > 0 {
                        counts.push(format!("✗{} failed", d.failed));
                    }
                    if d.in_progress > 0 {
                        counts.push(format!("{} in-progress", d.in_progress));
                    }
                    if d.pending > 0 {
                        counts.push(format!("{} pending", d.pending));
                    }
                    if d.ready > 0 {
                        counts.push(format!("{} ready", d.ready));
                    }
                    if d.skipped > 0 {
                        counts.push(format!("{} skipped", d.skipped));
                    }
                    if !counts.is_empty() {
                        rows.push(("  Targets".to_string(), counts.join(" · ")));
                    }
                    if !d.error_code.is_empty() || !d.error_message.is_empty() {
                        let msg = if d.error_message.is_empty() {
                            d.error_code.clone()
                        } else {
                            format!("{}: {}", d.error_code, d.error_message)
                        };
                        rows.push(("  Error".to_string(), format!("✗ {}", msg)));
                    }
                    if !d.rollback_deployment_id.is_empty() {
                        rows.push((
                            "  Rolled back by".to_string(),
                            d.rollback_deployment_id.clone(),
                        ));
                    }
                    if !d.rollback_message.is_empty() {
                        rows.push(("  Rollback".to_string(), d.rollback_message.clone()));
                    }
                    if !d.description.is_empty() {
                        rows.push(("  Description".to_string(), d.description.clone()));
                    }
                }
                rows
            }
        },
    }
}

pub(super) fn render_code_deploy_group_split(
    app: &App,
    g: &crate::aws::services::code::CodeDeployGroup,
    area: ratatui::layout::Rect,
    frame: &mut Frame,
) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 3, "");
    let mut block = theme::pane_block("Deployment Group", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let (state_glyph, state_color) = g
        .last_attempted
        .as_ref()
        .map(|d| match d.status.as_str() {
            "Succeeded" => ("●", theme::success()),
            "Failed" | "Stopped" => ("●", theme::error()),
            "" => ("○", theme::text_dim()),
            _ => ("●", theme::warning()),
        })
        .unwrap_or(("○", theme::text_dim()));
    let header_lines = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                g.group_name.clone(),
                Style::default()
                    .fg(crate::ui::theme::text_primary())
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(format!("{} ", state_glyph), Style::default().fg(state_color)),
            Span::styled(
                format!(
                    "{} · {}",
                    g.app_name,
                    if g.compute_platform.is_empty() {
                        "Server".to_string()
                    } else {
                        g.compute_platform.clone()
                    }
                ),
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

    let sections = descriptor_tabs(app, &crate::aws::services::code::CODE_DEPLOY_SECTIONS);
    let mut spans: Vec<Span> = vec![Span::raw("  ")];
    for (i, (key, label, section)) in sections.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("   │   ", Style::default().fg(theme::text_dim())));
        }
        let active = *section;
        if active {
            spans.push(Span::styled(
                key.to_string(),
                Style::default()
                    .fg(theme::aws_orange())
                    .add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::styled(
                format!(" {} ", label),
                Style::default()
                    .fg(Color::Black)
                    .bg(theme::aws_orange())
                    .add_modifier(Modifier::BOLD),
            ));
        } else {
            spans.push(Span::styled(
                key.to_string(),
                Style::default().fg(theme::text_dim()),
            ));
            spans.push(Span::styled(
                format!(" {} ", label),
                Style::default().fg(crate::ui::theme::text_muted()),
            ));
        }
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), chunks[2]);
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

// ── CodeArtifact Repository split pane ─────────────────────────────────────────

pub fn code_artifact_repo_section_lines(
    repo: &crate::aws::services::code::CodeArtifactRepo,
    section: CodeArtifactRepoDetailSection,
    packages_state: Option<&Lazy<Vec<crate::aws::services::code::CodeArtifactPackage>>>,
) -> Vec<(String, String)> {
    match section {
        CodeArtifactRepoDetailSection::Overview => {
            let mut rows = vec![
                ("Name".to_string(), repo.name.clone()),
                ("Domain".to_string(), repo.domain_name.clone()),
            ];
            if !repo.domain_owner.is_empty() {
                rows.push(("Domain Owner".to_string(), repo.domain_owner.clone()));
            }
            if !repo.admin_account.is_empty() {
                rows.push(("Admin Account".to_string(), repo.admin_account.clone()));
            }
            rows.push(("ARN".to_string(), repo.arn.clone()));
            if !repo.description.is_empty() {
                rows.push(("Description".to_string(), repo.description.clone()));
            }
            if !repo.created.is_empty() {
                rows.push(("Created".to_string(), repo.created.clone()));
            }
            rows
        }
        CodeArtifactRepoDetailSection::Packages => match packages_state {
            None | Some(Lazy::Loading) => {
                vec![("".to_string(), "Loading packages…".to_string())]
            }
            Some(Lazy::Error(e)) => error_rows(e),
            Some(Lazy::Loaded(packages)) => {
                if packages.is_empty() {
                    return vec![("".to_string(), "No packages in this repository".to_string())];
                }
                let mut rows: Vec<(String, String)> = vec![
                    (
                        "".to_string(),
                        format!("{} package(s) · latest version shown", packages.len()),
                    ),
                    ("".to_string(), "".to_string()),
                ];
                for p in packages {
                    let name = if p.namespace.is_empty() {
                        p.package.clone()
                    } else {
                        format!("{}/{}", p.namespace, p.package)
                    };
                    let version = if p.latest_version.is_empty() {
                        p.format.clone()
                    } else {
                        format!("{} · {}", p.format, p.latest_version)
                    };
                    rows.push((name, version));
                }
                rows
            }
        },
    }
}

pub(super) fn render_code_artifact_repo_split(
    app: &App,
    repo: &crate::aws::services::code::CodeArtifactRepo,
    area: ratatui::layout::Rect,
    frame: &mut Frame,
) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 2, "");
    let mut block = theme::pane_block("CodeArtifact Repository", focused);
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
                repo.name.clone(),
                Style::default()
                    .fg(crate::ui::theme::text_primary())
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                format!("domain: {}", repo.domain_name),
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

    let sections = descriptor_tabs(app, &crate::aws::services::code::CODE_ARTIFACT_SECTIONS);
    let mut spans: Vec<Span> = vec![Span::raw("  ")];
    for (i, (key, label, section)) in sections.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("   │   ", Style::default().fg(theme::text_dim())));
        }
        let active = *section;
        if active {
            spans.push(Span::styled(
                key.to_string(),
                Style::default()
                    .fg(theme::aws_orange())
                    .add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::styled(
                format!(" {} ", label),
                Style::default()
                    .fg(Color::Black)
                    .bg(theme::aws_orange())
                    .add_modifier(Modifier::BOLD),
            ));
        } else {
            spans.push(Span::styled(
                key.to_string(),
                Style::default().fg(theme::text_dim()),
            ));
            spans.push(Span::styled(
                format!(" {} ", label),
                Style::default().fg(crate::ui::theme::text_muted()),
            ));
        }
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), chunks[2]);
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}
