use crate::app::App;
use crate::aws::service::ServiceType;
use crate::ui::theme;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

/// `" name "` chip width in columns.
fn chip_width(label: &str) -> u16 {
    label.chars().count() as u16 + 2
}

/// `" │ "` separator between chips.
const SEP: u16 = 3;
/// `"‹ "` / `" ›"` overflow markers.
const MARKER_W: u16 = 2;

/// Service strip showing only visited services (loaded at least once this
/// session), plus the right-side badges (endpoint / assumed role / profile /
/// account / hint). The badges are **never dropped**: they degrade
/// field-by-field (hint → account → long assumed form → profile) and the chip
/// strip scrolls a window around the active chip with `‹`/`›` markers, like
/// the sub-tab bars. Switch via S modal, @prefix search, or chip click.
pub fn render_service_tabs(app: &App, area: Rect, frame: &mut Frame) {
    // The service bar is the first tab strip drawn each frame — reset the
    // mouse click-region list here so every bar can append fresh regions.
    app.clear_click_regions();

    // ── Right side: badge parts in priority order ─────────────────────────
    let endpoint_text = match app.aws_clients.current_endpoint() {
        Some(ep) => {
            // Strip the scheme for a compact `⚙ localhost:4566` badge.
            let host = ep
                .strip_prefix("http://")
                .or_else(|| ep.strip_prefix("https://"))
                .unwrap_or(ep);
            format!(" ⚙ {} ", host.trim_end_matches('/'))
        }
        None => String::new(),
    };
    // Assumed member-account badge — loud on purpose: every list/detail row
    // rendered while this shows belongs to the assumed account, not the base.
    let (assumed_full, assumed_short) = match app.aws_clients.current_assumed_role() {
        Some(role) => {
            let short = format!(" ⇄ {} ", role.account_id);
            let full = if role.account_name.is_empty() {
                short.clone()
            } else {
                format!(" ⇄ {} ({}) ", role.account_name, role.account_id)
            };
            (full, short)
        }
        None => (String::new(), String::new()),
    };
    let profile_text = match app.aws_clients.current_profile() {
        Some(p) => format!(" ⦿ {} ", p),
        None => String::new(),
    };
    let account_text = match (&app.account_alias, &app.account_id) {
        (Some(alias), Some(id)) => format!(" {} ({}) ", alias, id),
        (None, Some(id)) => format!(" {} ", id),
        _ => String::new(),
    };
    let hint_text = " S: services  @: prefix ";

    let endpoint_style = Style::default()
        .fg(Color::Black)
        .bg(theme::warning())
        .add_modifier(Modifier::BOLD);
    let assumed_style = Style::default()
        .fg(Color::Black)
        .bg(theme::aws_orange())
        .add_modifier(Modifier::BOLD);
    let profile_style = Style::default()
        .fg(crate::ui::theme::text_primary())
        .add_modifier(Modifier::BOLD);
    let account_style = Style::default().fg(theme::aws_orange());
    let hint_style = Style::default().fg(theme::text_dim());

    // Candidate badge sets, widest first. The last (assumed/endpoint only) is
    // the floor — identity info survives any terminal width.
    let build = |hint: bool, account: bool, assumed_long: bool, profile: bool| {
        let mut parts: Vec<(String, Style)> = Vec::new();
        if !endpoint_text.is_empty() {
            parts.push((endpoint_text.clone(), endpoint_style));
        }
        let assumed = if assumed_long { &assumed_full } else { &assumed_short };
        if !assumed.is_empty() {
            parts.push((assumed.clone(), assumed_style));
        }
        if profile && !profile_text.is_empty() {
            parts.push((profile_text.clone(), profile_style));
        }
        if account && !account_text.is_empty() {
            parts.push((account_text.clone(), account_style));
        }
        if hint {
            parts.push((hint_text.to_string(), hint_style));
        }
        parts
    };
    let candidates = [
        build(true, true, true, true),
        build(false, true, true, true),
        build(false, false, true, true),
        build(false, false, false, true),
        build(false, false, false, false),
    ];
    let parts_width = |parts: &[(String, Style)]| -> u16 {
        parts
            .iter()
            .map(|(t, _)| t.chars().count() as u16)
            .sum()
    };

    // ── Left side: brand prefix + visited-service chips ───────────────────
    let visible: Vec<ServiceType> = ServiceType::all()
        .iter()
        .copied()
        .filter(|s| app.visited_services.contains(s) || app.current_service == Some(*s))
        .collect();
    // Chip labels, built once so every width computation (reservation,
    // windowing, render, click regions) agrees.
    let labels: Vec<String> = visible
        .iter()
        .map(|s| s.short_name().to_string())
        .collect();
    let active_idx = visible
        .iter()
        .position(|s| app.current_service == Some(*s))
        .unwrap_or(0);

    let prefix_w: u16 = 1 + if app.banner_visible { 0 } else { 9 }; // " " + "neboto │ "

    // Reserve enough left space for at least the active chip + both markers;
    // pick the widest badge set that still allows that.
    let min_left = prefix_w
        + MARKER_W * 2
        + labels
            .get(active_idx)
            .map(|l| chip_width(l))
            .unwrap_or(0);
    let right_parts = candidates
        .iter()
        .find(|p| parts_width(p) + min_left <= area.width)
        .unwrap_or(&candidates[candidates.len() - 1]);
    let right_width = parts_width(right_parts);
    let left_avail = area.width.saturating_sub(right_width);

    // Window the chips into the space left of the badges.
    let full_chips: u16 = labels
        .iter()
        .enumerate()
        .map(|(i, l)| chip_width(l) + if i > 0 { SEP } else { 0 })
        .sum();
    let (lo, hi) = if visible.is_empty() {
        (0usize, 0usize)
    } else if prefix_w + full_chips <= left_avail {
        (0, visible.len() - 1)
    } else {
        visible_window(&labels, active_idx, left_avail.saturating_sub(prefix_w))
    };
    let left_marker = lo > 0;
    let right_marker = !visible.is_empty() && hi + 1 < visible.len();

    let mut spans: Vec<Span> = vec![Span::raw(" ")];
    if !app.banner_visible {
        spans.push(Span::styled(
            "neboto",
            Style::default()
                .fg(theme::aws_orange())
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(" │ ", Style::default().fg(theme::text_dim())));
    }
    let mut chip_x = area.x + prefix_w;

    if left_marker {
        spans.push(Span::styled("‹ ", Style::default().fg(theme::text_dim())));
        chip_x += MARKER_W;
    }

    for (slot, i) in (lo..=hi).enumerate() {
        let Some(service) = visible.get(i) else { break };
        let Some(label) = labels.get(i) else { break };
        if slot > 0 {
            spans.push(Span::styled(" │ ", Style::default().fg(theme::text_dim())));
            chip_x += SEP;
        }

        let chip_w = chip_width(label);
        app.push_click_region(
            Rect {
                x: chip_x,
                y: area.y,
                width: chip_w,
                height: 1,
            },
            crate::app::ClickAction::Service(*service),
        );
        chip_x += chip_w;

        let is_active = app.current_service == Some(*service);
        if is_active {
            spans.push(Span::styled(
                format!(" {} ", label),
                Style::default()
                    .fg(Color::Black)
                    .bg(theme::aws_orange())
                    .add_modifier(Modifier::BOLD),
            ));
        } else {
            spans.push(Span::styled(
                format!(" {} ", label),
                Style::default().fg(crate::ui::theme::text_muted()),
            ));
        }
    }

    if right_marker {
        spans.push(Span::styled(" ›", Style::default().fg(theme::text_dim())));
    }

    // ── Compose: left chips, pad, badges (always rendered) ────────────────
    let left_width = Line::from(spans.clone()).width() as u16;
    let pad = area.width.saturating_sub(left_width + right_width);
    if pad > 0 {
        spans.push(Span::raw(" ".repeat(pad as usize)));
    }
    for (text, style) in right_parts {
        spans.push(Span::styled(text.clone(), *style));
    }

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// Pick the inclusive chip range `[lo, hi]` to show when the strip overflows:
/// always include the active chip, then grow outward (right first, then left)
/// while it fits. Marker columns are reserved up front. Same algorithm as
/// `subtab_bar::visible_window`, over the pre-built chip labels.
fn visible_window(labels: &[String], active: usize, width: u16) -> (usize, usize) {
    let n = labels.len();
    let budget = width.saturating_sub(MARKER_W * 2);

    let mut lo = active;
    let mut hi = active;
    let mut used = chip_width(&labels[active]).min(budget);

    loop {
        let mut grew = false;
        if hi + 1 < n {
            let need = SEP + chip_width(&labels[hi + 1]);
            if used + need <= budget {
                hi += 1;
                used += need;
                grew = true;
            }
        }
        if lo > 0 {
            let need = SEP + chip_width(&labels[lo - 1]);
            if used + need <= budget {
                lo -= 1;
                used += need;
                grew = true;
            }
        }
        if !grew {
            break;
        }
    }
    (lo, hi)
}
