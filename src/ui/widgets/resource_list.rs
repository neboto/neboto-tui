use crate::app::App;
use crate::ui::theme;
use ratatui::{
    layout::{Alignment, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{
        block::{Position, Title},
        List, ListItem, Paragraph,
    },
    Frame,
};

/// A leading `[Type]` badge for Organizations policy rows (SCP / Tag / Backup /
/// …) — the policies otherwise differ only by name, so the type is the
/// at-a-glance discriminator. `None` for every other resource type.
/// The dim second column. Normally the id (when it adds to the name); a
/// Route 53 record shows what it *answers with* instead — its id is a
/// compound key nobody wants to read, and a DNS list without values is
/// half a list.
fn id_cell(resource: &dyn crate::aws::resource::Resource) -> String {
    if let Some(rec) = resource
        .as_any()
        .downcast_ref::<crate::aws::services::route53::R53Record>()
    {
        return rec.value_summary();
    }
    if resource.id() != resource.name() {
        resource.id().to_string()
    } else {
        String::new()
    }
}

fn org_policy_badge(resource: &dyn crate::aws::resource::Resource) -> Option<String> {
    resource
        .as_any()
        .downcast_ref::<crate::aws::services::organizations::OrgScp>()
        .map(|scp| format!("[{}]", scp.policy_type))
}

/// Floor for the name column, not a ceiling: it's what the name gets on a
/// cramped list. On a wide one the name grows into whatever the id and state
/// columns don't need — a fixed cap truncated long names (deep OU paths,
/// nested stack names) with the rest of the row sitting empty.
const NAME_MIN: usize = 40;
/// Below this the id column isn't worth rendering; it's dropped and the name
/// takes the space (the id is always in the detail pane anyway).
const MIN_ID_COL: usize = 8;
/// The id column never takes more than a `1/ID_COL_SHARE` slice of a wide
/// list. Ids that are ARNs would otherwise crowd out the name, which is the
/// column people actually read.
const ID_COL_SHARE: usize = 3;

/// Split a wide list row into `(name, id, state)` column widths.
///
/// `*_nat` are the natural (widest-content) widths over the whole filtered
/// set, `state_nat` already capped by the caller. `lead` covers any leading
/// badges. Ordering matters: the id is bounded to its share **first** so the
/// name can claim everything else, then the id is re-derived from what the
/// name actually left — otherwise a long-id service and a long-name service
/// each starve the other.
fn column_widths(
    inner_width: usize,
    lead: usize,
    name_nat: usize,
    id_nat: usize,
    state_nat: usize,
) -> (usize, usize, usize) {
    let id_w = id_nat.min((inner_width / ID_COL_SHARE).max(MIN_ID_COL));
    let gutters = 2 + lead + 2 + 2 + state_nat;
    let name_avail = inner_width.saturating_sub(gutters + id_w);
    // `NAME_MIN` is a floor the name may claim back from the id column, but it
    // can't exceed the pane: a wide badge plus a long state word can leave
    // less than 40 columns even on a "wide" list, and padding past that pushes
    // the right-aligned state off the row.
    let name_ceiling = inner_width.saturating_sub(gutters);
    let name_w = name_nat.min(name_avail.max(NAME_MIN).min(name_ceiling));

    let id_avail = inner_width.saturating_sub(2 + lead + name_w + 2 + 2 + state_nat);
    let id_w = if id_avail < MIN_ID_COL {
        0
    } else {
        id_w.min(id_avail)
    };
    (name_w, id_w, state_nat)
}

pub fn render_resource_list(app: &App, area: Rect, frame: &mut Frame) {
    // Record geometry so mouse clicks/scroll can hit-test this pane.
    app.record_list_geometry(area);

    let focused = !app.details_focused;

    let title = if app.all_search_mode {
        "All services (cached)".to_string()
    } else {
        match app.current_service {
            Some(service) => service.description().to_string(),
            None => "Resources".to_string(),
        }
    };

    // Count / progress indicator shown in the bottom-right corner of the border
    let show_loading = app.show_loading_indicator();
    let corner_text = if show_loading {
        let count = match &app.loading_progress {
            Some(progress) => match progress.total_count {
                Some(total) => format!("{}/{}", progress.loaded_count, total),
                None => format!("{}", progress.loaded_count),
            },
            None => String::new(),
        };
        // Include the per-phase message ("Loading policies…") so multi-batch
        // loads don't look stalled when watching the list instead of the
        // status bar. Truncated so it can't swallow the border.
        let phase = app
            .loading_progress
            .as_ref()
            .and_then(|p| p.status_message.as_deref())
            .map(|m| {
                let m = m.trim_end_matches(['.', '…']);
                // The corner already says "loading" — drop a redundant prefix.
                let m = m.strip_prefix("Loading ").unwrap_or(m);
                let short: String = m.chars().take(32).collect();
                if short.len() < m.len() {
                    format!(" · {short}…")
                } else {
                    format!(" · {short}")
                }
            })
            .unwrap_or_default();
        format!(
            " {} loading {}{} ",
            theme::spinner(app.tick_count),
            count,
            phase
        )
    } else {
        let hidden_note = if app.hide_noise && app.hidden_noise_count > 0 {
            format!(" ({} hidden)", app.hidden_noise_count)
        } else {
            String::new()
        };
        // S3 shows other-region buckets dimmed; ⏎ on one switches region.
        let region_note = if app.current_service == Some(crate::aws::service::ServiceType::S3)
            && app.s3_other_region_count > 0
        {
            format!(" · {} dimmed in other regions (⏎)", app.s3_other_region_count)
        } else {
            String::new()
        };
        // Filter chip: show the active search term so it's clear the count is
        // filtered (and by what) without looking at the search bar.
        let filter_note = if !app.search_query.is_empty() {
            format!("/{} · ", app.search_query)
        } else {
            String::new()
        };
        // Sort / state-filter chips (`z` / `F`) — only when active, so the
        // corner explains any non-default ordering or a thinned-out list.
        let sort_note = app
            .list_sort
            .label()
            .map(|l| format!("z sort: {} · ", l))
            .unwrap_or_default();
        let state_note = app
            .list_state_filter
            .as_deref()
            .map(|s| format!("F state: {} · ", s))
            .unwrap_or_default();
        format!(
            " {}{}{}{} of {}{}{} ",
            sort_note,
            state_note,
            filter_note,
            app.filtered_resources.len(),
            app.current_view_total(),
            hidden_note,
            region_note
        )
    };

    let corner_style = if show_loading {
        Style::default().fg(theme::warning())
    } else {
        Style::default().fg(theme::text_dim())
    };

    let block = theme::pane_block(&title, focused).title(
        Title::from(Span::styled(corner_text, corner_style))
            .position(Position::Bottom)
            .alignment(Alignment::Right),
    );

    // Empty states: centered message with a contextual hint
    if app.filtered_resources.is_empty() {
        let (message, hint) = if show_loading {
            (
                format!("{} Loading resources…", theme::spinner(app.tick_count)),
                String::new(),
            )
        } else if app.all_search_mode {
            // @all searches warm cache entries only — an empty result set
            // needs to say whether nothing matched or nothing is cached yet.
            if app.resources.is_empty() {
                (
                    "Nothing cached to search".to_string(),
                    "@all searches services already visited this session".to_string(),
                )
            } else {
                (
                    format!("No matches for “{}”", app.search_query),
                    "searching cached services only · Esc clear".to_string(),
                )
            }
        } else if app.resources.is_empty() {
            // FMS from a non-admin account fails every call — say why instead
            // of a generic "no resources". (The admin-account id, when the
            // probe could see it, tells the user where to go.)
            let fms_non_admin = app.current_service
                == Some(crate::aws::service::ServiceType::Fms)
                && app.fms_admin.as_ref().is_some_and(|a| {
                    a.error.is_some()
                        || (a.admin_account.is_some() && a.admin_account != app.account_id)
                });
            if fms_non_admin {
                let hint = app
                    .fms_admin
                    .as_ref()
                    .and_then(|a| a.admin_account.as_deref())
                    .map(|acct| format!("admin account: {} · P switch profile", acct))
                    .unwrap_or_else(|| "P switch to the FMS admin profile".to_string());
                (
                    "This account isn't the Firewall Manager administrator".to_string(),
                    hint,
                )
            }
            // Control Tower answers only in the landing zone's home region,
            // from the management account — an empty list is almost always one
            // of those two, not an actually-empty landing zone.
            else if app.current_service == Some(crate::aws::service::ServiceType::ControlTower) {
                (
                    "No Control Tower resources found".to_string(),
                    "Control Tower answers only in its home region, from the management account · R switch region"
                        .to_string(),
                )
            }
            // A server-side CloudTrail filter that matched nothing deserves a
            // pointer back to the filter, not a generic "no resources".
            else if app.current_service == Some(crate::aws::service::ServiceType::CloudTrail) {
                if let Some(chip) = app.ct_query.chip() {
                    (
                        format!("No events match “{}”", chip),
                        "f edit filter".to_string(),
                    )
                } else {
                    (
                        "No resources found".to_string(),
                        "r refresh · R switch region".to_string(),
                    )
                }
            } else {
                (
                    "No resources found".to_string(),
                    "r refresh · R switch region".to_string(),
                )
            }
        } else if app.search_query.is_empty() {
            // `a` is a global toggle that survives service and sub-tab
            // switches, so a tab whose rows are *all* noise reads as empty
            // long after the keypress. This has to be checked FIRST: every
            // per-tab arm below would otherwise misattribute it to a
            // permission gap the user doesn't have.
            if app.hide_noise && app.hidden_noise_count > 0 {
                (
                    format!("{} row(s) hidden by the noise filter", app.hidden_noise_count),
                    "a show all".to_string(),
                )
            }
            // Route 53 Records: rows stream in per zone, so "empty" is
            // usually "still loading" or "every zone was skipped by the
            // budget" — never a permission gap the zones tab didn't show.
            else if app.current_service == Some(crate::aws::service::ServiceType::Route53)
                && app.r53_view == crate::app::R53View::Records
                && app.list_state_filter.is_none()
            {
                let (big, budget) = app.r53_records_skipped;
                if app.lazy.r53_zone_records.loading_count() > 0 {
                    (
                        "Loading records…".to_string(),
                        "one zone at a time (Route 53 allows 5 requests/s)".to_string(),
                    )
                } else if big + budget > 0 {
                    (
                        "No records loaded".to_string(),
                        format!(
                            "{} zone(s) skipped ({} over the per-zone record budget, {} past the zone budget) · open a zone to load its records",
                            big + budget, big, budget
                        ),
                    )
                } else if !app.resources.is_empty() {
                    ("No records in any loaded zone".to_string(), "r refresh".to_string())
                } else {
                    ("No hosted zones".to_string(), "r refresh".to_string())
                }
            }
            // The Pull Requests tab eagerly loads OPEN PRs only (closed ones
            // are a per-repo lazy fetch on the repo pane) — an empty tab
            // usually means nothing is waiting on review, not a load gap.
            else if app.current_service == Some(crate::aws::service::ServiceType::Code)
                && app.code_view == crate::app::CodeView::PullRequests
                && app.list_state_filter.is_none()
            {
                (
                    "No open pull requests".to_string(),
                    "closed PRs live on each repo's Pull Requests section".to_string(),
                )
            }
            // An empty Cross-Account tab means OAM just isn't configured —
            // most accounts have neither a sink nor a link, and the eager
            // lists succeed with nothing. Not a permission gap.
            else if app.current_service == Some(crate::aws::service::ServiceType::CloudWatch)
                && app.cw_view == crate::app::CwView::CrossAccount
                && app.list_state_filter.is_none()
            {
                (
                    "No OAM sinks or links".to_string(),
                    "cross-account observability isn't configured in this account/region"
                        .to_string(),
                )
            }
            // Same reasoning for the other settings-flavoured CloudWatch tabs:
            // empty means the feature is unused, not that a load failed.
            else if app.current_service == Some(crate::aws::service::ServiceType::CloudWatch)
                && app.cw_view == crate::app::CwView::Streams
                && app.list_state_filter.is_none()
            {
                (
                    "No metric streams".to_string(),
                    "nothing continuously exports CloudWatch metrics in this region".to_string(),
                )
            }
            else if app.current_service == Some(crate::aws::service::ServiceType::CloudWatch)
                && app.cw_view == crate::app::CwView::Insights
                && app.list_state_filter.is_none()
            {
                (
                    "No anomaly detectors or Contributor Insights rules".to_string(),
                    "none configured in this region".to_string(),
                )
            }
            else if app.current_service == Some(crate::aws::service::ServiceType::CloudWatch)
                && app.cw_view == crate::app::CwView::AccountPolicies
                && app.list_state_filter.is_none()
            {
                (
                    "No account-level Logs policies".to_string(),
                    "data protection · subscription filter · field index · transformer · metric extraction".to_string(),
                )
            }
            // An empty CloudTrail Insights tab is usually the feature being
            // off, not a filter problem — say so.
            else if app.current_service == Some(crate::aws::service::ServiceType::CloudTrail)
                && app.cloudtrail_view == crate::app::CloudTrailView::Insights
                && app.list_state_filter.is_none()
            {
                (
                    "No Insights events in this window".to_string(),
                    "requires CloudTrail Insights enabled on a trail".to_string(),
                )
            }
            // Only the GuardDuty administrator / delegated admin can list
            // members, so an empty Accounts tab is almost always "wrong
            // account" rather than an org with no members. The load stays
            // silent about it (the calls are *expected* to fail everywhere
            // else) so the explanation has to live here.
            else if app.current_service == Some(crate::aws::service::ServiceType::GuardDuty)
                && app.guardduty_view == crate::app::GuardDutyView::Accounts
                && app.list_state_filter.is_none()
            {
                (
                    "No member accounts visible".to_string(),
                    "only the GuardDuty administrator can list members · P switch profile"
                        .to_string(),
                )
            }
            // The Controls tab needs `ListSecurityControlDefinitions` +
            // `BatchGetSecurityControls`, which the older Security Hub
            // read policies don't grant — the phase warns and moves on, so
            // name the likely cause here.
            else if app.current_service == Some(crate::aws::service::ServiceType::SecurityHub)
                && app.securityhub_view == crate::app::SecurityHubView::Controls
                && app.list_state_filter.is_none()
            {
                (
                    "No security controls loaded".to_string(),
                    "needs securityhub:ListSecurityControlDefinitions · r retry".to_string(),
                )
            }
            // Only the Security Hub administrator can list automation rules,
            // so in a member account this tab is empty for a reason the load
            // warning alone doesn't make obvious.
            else if app.current_service == Some(crate::aws::service::ServiceType::SecurityHub)
                && app.securityhub_view == crate::app::SecurityHubView::Automations
                && app.list_state_filter.is_none()
            {
                (
                    "No automation rules or custom actions".to_string(),
                    "only the Security Hub administrator can list rules · P switch profile"
                        .to_string(),
                )
            }
            // Every region that offers AgentCore returns the AWS-managed
            // defaults (`aws.browser.v1` / `aws.codeinterpreter.v1`) from
            // `ListBrowsers` / `ListCodeInterpreters`, so an empty Tools tab
            // means the service isn't in this region — a much more likely
            // explanation than "no tools configured", and one the per-phase
            // load warnings don't spell out.
            else if app.current_service == Some(crate::aws::service::ServiceType::AgentCore)
                && app.agentcore_view == crate::app::AgentCoreView::Tools
                && app.list_state_filter.is_none()
            {
                (
                    "No built-in tools — AgentCore may not be in this region".to_string(),
                    "the managed browser/code-interpreter always list where it is · R switch region"
                        .to_string(),
                )
            }
            // ListTagOptions fails with TagOptionNotMigratedException in any
            // account that never enabled the TagOptions library, and the load
            // stays silent on that error by design (it would warn on every
            // load in most accounts) — so this hint is the whole explanation.
            else if app.current_service
                == Some(crate::aws::service::ServiceType::ServiceCatalog)
                && app.sc_view == crate::app::ScView::TagOptions
                && app.list_state_filter.is_none()
            {
                (
                    "No TagOptions".to_string(),
                    "the TagOptions library may not be enabled in this account/region".to_string(),
                )
            }
            // Policy / Evaluation / Registry are AgentCore preview surfaces
            // whose list phases fail silently by design (most accounts have
            // never enabled them, so a load warning everywhere would be
            // noise). That means an empty tab here carries no other signal —
            // this hint is the whole explanation.
            else if app.current_service == Some(crate::aws::service::ServiceType::AgentCore)
                && matches!(
                    app.agentcore_view,
                    crate::app::AgentCoreView::Policy
                        | crate::app::AgentCoreView::Evaluation
                        | crate::app::AgentCoreView::Registry
                        | crate::app::AgentCoreView::Harness
                        | crate::app::AgentCoreView::Payments
                )
                && app.list_state_filter.is_none()
            {
                (
                    "Nothing here — this AgentCore feature is in preview".to_string(),
                    "not enabled in this account/region, or not yet available · R switch region"
                        .to_string(),
                )
            }
            // `ListMembers` is silent on failure — a standalone account is
            // expected to fail it, so the explanation lives here rather than
            // as a warning on every load.
            else if app.current_service == Some(crate::aws::service::ServiceType::SecurityHub)
                && app.securityhub_view == crate::app::SecurityHubView::Accounts
                && app.list_state_filter.is_none()
            {
                (
                    "No member accounts visible".to_string(),
                    "only the Security Hub administrator can list members · P switch profile"
                        .to_string(),
                )
            } else {
                // A state filter (or noise filter) thinned the view to nothing.
                let subject = app
                    .list_state_filter
                    .as_deref()
                    .map(|s| format!("No resources in state “{}”", s))
                    .unwrap_or_else(|| "No resources in view".to_string());
                (subject, "F cycle state filter · a show all".to_string())
            }
        } else {
            let hint = if app.search_query.contains("tag:") {
                "Esc clear search".to_string()
            } else {
                "Esc clear search · tag:key=value filters by tag".to_string()
            };
            (format!("No matches for “{}”", app.search_query), hint)
        };

        let inner_height = area.height.saturating_sub(2);
        let top_pad = (inner_height.saturating_sub(2) / 2) as usize;
        let mut lines = vec![Line::raw(""); top_pad];
        lines.push(Line::styled(message, Style::default().fg(crate::ui::theme::text_muted())));
        if !hint.is_empty() {
            lines.push(Line::styled(hint, Style::default().fg(theme::text_dim())));
        }

        let paragraph = Paragraph::new(lines)
            .block(block)
            .alignment(Alignment::Center);

        frame.render_widget(paragraph, area);
        return;
    }

    // Width available for text inside borders, minus the highlight symbol
    let inner_width = area.width.saturating_sub(4) as usize;
    const STATE_CAP: usize = 24;

    // Wide-terminal columns (U16): with enough width, pad names to a shared
    // column, align ids, and right-align the state word in its state color.
    // Below the threshold the compact "name  dim-id" layout stands. Column
    // widths come from the whole filtered set (not just visible rows) so the
    // layout doesn't shift while scrolling.
    const WIDE_MIN_INNER: usize = 70;
    // Cost overloads `state()` as a spend-trend dot; the words
    // "available"/"unavailable" would mislead there, so it stays compact.
    let wide = inner_width >= WIDE_MIN_INNER
        && app.current_service != Some(crate::aws::service::ServiceType::Cost);
    // @all rows lead with a 6-col service badge; budget the wide columns for it.
    let badge_w: usize = if app.all_search_mode { 6 } else { 0 };
    // Organizations policy rows lead with a `[Type]` badge (fixed-width so the
    // name column stays aligned); zero for every other view.
    let pol_badge_w: usize = app
        .filtered_resources
        .iter()
        .filter_map(|&i| org_policy_badge(app.resources[i].as_ref()))
        .map(|b| b.chars().count() + 1)
        .max()
        .unwrap_or(0);
    let (name_col, id_col, state_col) = if wide {
        let mut name_w = 0usize;
        let mut id_w = 0usize;
        let mut state_w = 0usize;
        for &idx in &app.filtered_resources {
            let r = &app.resources[idx];
            name_w = name_w.max(r.name().chars().count());
            id_w = id_w.max(id_cell(r.as_ref()).chars().count());
            state_w = state_w.max(r.state_label().chars().count());
        }
        column_widths(
            inner_width,
            badge_w + pol_badge_w,
            name_w,
            id_w,
            state_w.min(STATE_CAP),
        )
    } else {
        (0, 0, 0)
    };

    // Pad-or-truncate to exactly `w` display chars (truncation gets an `…`).
    let fit = |s: &str, w: usize| -> String {
        let n = s.chars().count();
        if n <= w {
            format!("{}{}", s, " ".repeat(w - n))
        } else {
            let cut: String = s.chars().take(w.saturating_sub(1)).collect();
            format!("{}…", cut)
        }
    };

    // Rows inside the list visual selection (`V` / `Ctrl-A` / drag) render on
    // the selection bar style, like the detail body's visual mode; every span
    // is restyled so per-state colors can't clash with the bar.
    fn visual_item(line: Line<'_>) -> ListItem<'_> {
        let sel = theme::selection_style(true);
        let spans: Vec<Span> = line
            .spans
            .into_iter()
            .map(|s| Span::styled(s.content, sel))
            .collect();
        ListItem::new(Line::from(spans)).style(sel)
    }

    let items: Vec<ListItem> = app
        .filtered_resources
        .iter()
        .enumerate()
        .map(|(pos, &resource_idx)| {
            let in_visual = app.list_row_in_selection(pos);
            let resource = &app.resources[resource_idx];
            let (indicator, state_color) = theme::state_indicator(&resource.state());

            let name = resource.name();

            // S3 cross-region: every bucket shows its region; buckets outside the
            // current region are dimmed and marked with → (Enter switches region).
            let s3_region = if app.current_service == Some(crate::aws::service::ServiceType::S3) {
                resource
                    .as_any()
                    .downcast_ref::<crate::aws::services::s3::S3Bucket>()
                    .map(|b| b.region.clone())
            } else {
                None
            };
            let out_of_region = s3_region
                .as_deref()
                .map(|r| r != app.current_region.as_str())
                .unwrap_or(false);

            let name_style = if out_of_region {
                Style::default().fg(theme::text_dim())
            } else {
                Style::default()
            };
            let indicator_color = if out_of_region {
                theme::text_dim()
            } else {
                state_color
            };

            // Inspector "By Resource" rows: append the friendly type + a
            // severity tally (console-style at-a-glance counts).
            let insp_group = resource
                .as_any()
                .downcast_ref::<crate::aws::services::inspector::InspResourceGroup>();

            let mut spans = vec![Span::styled(
                format!("{} ", indicator),
                Style::default().fg(indicator_color),
            )];

            // @all cross-service rows lead with their owning service so
            // results from sixty services stay tellable-apart.
            if app.all_search_mode {
                let label = app
                    .all_search_sources
                    .get(resource_idx)
                    .map(|s| s.short_name())
                    .unwrap_or("");
                spans.push(Span::styled(
                    format!("{:<6}", label),
                    Style::default().fg(theme::aws_orange()),
                ));
            }

            // Organizations policy type badge (`[SCP]` / `[Tag]` / …), padded to
            // the column width so the name column lines up across rows.
            if pol_badge_w > 0 {
                let badge = org_policy_badge(resource.as_ref()).unwrap_or_default();
                spans.push(Span::styled(
                    format!("{:<width$}", badge, width = pol_badge_w),
                    Style::default()
                        .fg(theme::aws_orange())
                        .add_modifier(Modifier::BOLD),
                ));
            }

            if wide && insp_group.is_none() {
                // Columns: name │ id (dim) │ state right-aligned. The id cell
                // carries the S3 region note for out-of-region buckets.
                spans.push(Span::styled(format!("{}  ", fit(name, name_col)), name_style));
                if id_col > 0 {
                    let cell = if out_of_region {
                        s3_region
                            .as_deref()
                            .map(|r| format!("{} →", r))
                            .unwrap_or_default()
                    } else {
                        id_cell(resource.as_ref())
                    };
                    spans.push(Span::styled(
                        format!("{}  ", fit(&cell, id_col)),
                        Style::default().fg(theme::text_dim()),
                    ));
                }
                let mut state_text = resource.state_label();
                if state_text.chars().count() > STATE_CAP {
                    state_text = fit(&state_text, STATE_CAP);
                }
                let used =
                    2 + badge_w + pol_badge_w + name_col + 2 + if id_col > 0 { id_col + 2 } else { 0 };
                let right = inner_width.saturating_sub(used).max(state_col);
                spans.push(Span::styled(
                    format!("{:>right$}", state_text, right = right),
                    Style::default().fg(indicator_color),
                ));
                let line = Line::from(spans);
                return if in_visual {
                    visual_item(line)
                } else {
                    ListItem::new(line)
                };
            }

            spans.push(Span::styled(name.to_string(), name_style));

            if out_of_region {
                // Only out-of-region buckets show their region (+ a jump hint);
                // in-region buckets stay clean (they're all the current region).
                if let Some(region) = s3_region {
                    spans.push(Span::styled(
                        format!("  {} →", region),
                        Style::default().fg(theme::text_dim()),
                    ));
                }
            } else if let Some(grp) = insp_group {
                spans.push(Span::styled(
                    format!("  {} · {} findings", grp.display_type, grp.total()),
                    Style::default().fg(theme::text_dim()),
                ));
                let summary = grp.severity_summary();
                if !summary.is_empty() {
                    spans.push(Span::styled(
                        format!("  {}", summary),
                        Style::default().fg(state_color),
                    ));
                }
            } else {
                // Show the id cell dimmed after the name when it adds
                // information and there is room for at least part of it
                let cell = id_cell(resource.as_ref());
                let used = 2 + name.chars().count();
                if !cell.is_empty() && inner_width > used + 4 {
                    spans.push(Span::styled(
                        format!("  {}", cell),
                        Style::default().fg(theme::text_dim()),
                    ));
                }
            }

            let line = Line::from(spans);
            if in_visual {
                visual_item(line)
            } else {
                ListItem::new(line)
            }
        })
        .collect();

    let list = List::new(items)
        .block(block)
        .highlight_style(theme::selection_style(focused))
        .highlight_symbol(if focused { "▌ " } else { "  " })
        .highlight_spacing(ratatui::widgets::HighlightSpacing::Always);

    // Stateful rendering enables automatic scrolling (ListState in RefCell)
    frame.render_stateful_widget(list, area, &mut app.resource_list_state.borrow_mut());
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A full-screen list must not truncate a long name at the old fixed 40
    /// columns while the rest of the row sits empty — the regression that deep
    /// OU paths ("Root/Workloads/NonProd/…") surfaced.
    #[test]
    fn wide_list_gives_long_names_the_leftover_width() {
        let (name, id, _state) = column_widths(200, 0, 120, 16, 10);
        assert_eq!(name, 120, "name should get its natural width");
        assert_eq!(id, 16, "a short id keeps its natural width");
    }

    /// ...but the id column can't grow without bound either: an ARN-shaped id
    /// must stay inside its share so the name keeps the majority.
    #[test]
    fn long_ids_are_bounded_to_their_share() {
        let (name, id, _state) = column_widths(200, 0, 120, 100, 10);
        assert_eq!(id, 200 / ID_COL_SHARE);
        assert!(name > id, "name must outweigh a bounded id column: {name} vs {id}");
    }

    /// Narrow lists keep the old behavior: the name gets NAME_MIN and the id
    /// takes what's left.
    #[test]
    fn narrow_list_holds_the_name_floor() {
        let (name, _id, _state) = column_widths(70, 0, 120, 20, 10);
        assert_eq!(name, NAME_MIN);
    }

    /// When a wide badge and a long state word leave too little for both, the
    /// id column is dropped rather than rendered as an unreadable sliver.
    #[test]
    fn id_column_drops_when_squeezed() {
        let (_name, id, _state) = column_widths(70, 10, 200, 20, 24);
        assert_eq!(id, 0);
    }

    /// The name floor must yield to the pane: 40 columns don't fit alongside a
    /// wide badge and a long state word, and padding past the pane pushes the
    /// right-aligned state off the row.
    #[test]
    fn name_floor_yields_to_a_cramped_pane() {
        let (name, id, state) = column_widths(70, 10, 200, 20, 24);
        assert!(name < NAME_MIN, "floor should have been clamped: {name}");
        assert!(2 + 10 + name + 2 + if id > 0 { id + 2 } else { 0 } + state <= 70);
    }

    /// Content narrower than the pane is padded to content width, never
    /// stretched — short names must not push the state column off-screen.
    #[test]
    fn short_content_does_not_stretch() {
        let (name, id, _state) = column_widths(200, 0, 12, 8, 10);
        assert_eq!(name, 12);
        assert_eq!(id, 8);
    }

    /// Every column plus its gutters has to fit inside the pane, or the
    /// right-aligned state word wraps.
    #[test]
    fn columns_fit_within_the_pane() {
        for inner in [70usize, 90, 120, 200, 400] {
            for lead in [0usize, 6, 10] {
                for state in [0usize, 10, 24] {
                    for (name_nat, id_nat) in [(120, 100), (12, 8), (200, 200), (40, 60)] {
                        let (name, id, state_w) =
                            column_widths(inner, lead, name_nat, id_nat, state);
                        let used =
                            2 + lead + name + 2 + if id > 0 { id + 2 } else { 0 } + state_w;
                        assert!(
                            used <= inner,
                            "inner={inner} lead={lead} state={state} \
                             name_nat={name_nat} id_nat={id_nat} → used {used}"
                        );
                    }
                }
            }
        }
    }
}
