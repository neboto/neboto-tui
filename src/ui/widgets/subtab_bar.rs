//! Responsive single-row sub-tab strip shared by the per-service `*_tabs.rs`
//! widgets.
//!
//! Renders `key + " label "` chips with the active one highlighted. When the
//! chips fit `area.width` it looks identical to a hand-rolled bar. When they
//! don't, it scrolls a window of chips **around the active one** and shows
//! `‹`/`›` markers on the clipped ends — the active tab is always visible, and
//! number-key nav is unaffected since it never depends on visibility.
//!
//! It also records the mouse click-regions for the visible chips (replacing a
//! separate `App::record_subtab_regions` call), so a click always maps to the
//! chip actually drawn under the cursor.

use crate::app::{App, ClickAction};
use crate::ui::theme;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

/// One leading space before the first chip (matches the legacy bars).
const LEADING: u16 = 1;
/// `" │ "` between chips.
const SEP: u16 = 3;
const SEP_TEXT: &str = " │ ";
/// `"‹ "` / `" ›"` overflow markers.
const MARKER_W: u16 = 2;

/// Displayed width of a chip: key glyph (1) + `" label "` (label + 2).
fn chip_width(label: &str) -> u16 {
    label.chars().count() as u16 + 3
}

/// Render a responsive sub-tab strip. `tabs` is `(key, label, is_active)` in
/// display order; exactly one is expected to be active.
pub fn render_subtab_bar(app: &App, area: Rect, frame: &mut Frame, tabs: &[(char, &str, bool)]) {
    let spans = subtab_bar_spans(app, area, tabs);
    if !spans.is_empty() {
        frame.render_widget(Paragraph::new(Line::from(spans)), area);
    }
}

/// Like [`render_subtab_bar`] but returns the styled spans instead of
/// rendering, so a widget can append trailing chips on the same row (the ECS
/// status filter, the GD/SH/Inspector severity toggles, Cost's period
/// selector) and render the combined line itself. Click regions for the tab
/// chips are still recorded here, against `area`; the caller records regions
/// for whatever it appends (measuring `Line::from(spans.clone()).width()`
/// like the legacy bars did).
pub fn subtab_bar_spans(
    app: &App,
    area: Rect,
    tabs: &[(char, &str, bool)],
) -> Vec<Span<'static>> {
    if tabs.is_empty() || area.width == 0 {
        return Vec::new();
    }

    // Total width if everything is shown.
    let full_width: u16 = LEADING
        + tabs
            .iter()
            .enumerate()
            .map(|(i, (_, l, _))| chip_width(l) + if i > 0 { SEP } else { 0 })
            .sum::<u16>();

    let (lo, hi) = if full_width <= area.width {
        (0usize, tabs.len() - 1)
    } else {
        visible_window(tabs, area.width)
    };

    let left_marker = lo > 0;
    let right_marker = hi + 1 < tabs.len();

    let mut spans: Vec<Span> = vec![Span::raw(" ".repeat(LEADING as usize))];
    let mut x = area.x + LEADING;

    if left_marker {
        spans.push(Span::styled("‹ ", Style::default().fg(theme::text_dim())));
        x += MARKER_W;
    }

    for (slot, i) in (lo..=hi).enumerate() {
        if slot > 0 {
            spans.push(Span::styled(SEP_TEXT, Style::default().fg(theme::text_dim())));
            x += SEP;
        }
        let (key, label, is_active) = tabs[i];

        // Click region spans the whole chip (key glyph + padded label).
        let w = chip_width(label);
        app.push_click_region(
            Rect { x, y: area.y, width: w, height: 1 },
            ClickAction::Key(key),
        );
        x += w;

        if is_active {
            spans.push(Span::styled(
                key.to_string(),
                Style::default().fg(theme::aws_orange()).add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::styled(
                format!(" {} ", label),
                Style::default()
                    .fg(Color::Black)
                    .bg(theme::aws_orange())
                    .add_modifier(Modifier::BOLD),
            ));
        } else {
            spans.push(Span::styled(key.to_string(), Style::default().fg(theme::accent())));
            spans.push(Span::styled(format!(" {} ", label), Style::default().fg(crate::ui::theme::text_muted())));
        }
    }

    if right_marker {
        spans.push(Span::styled(" ›", Style::default().fg(theme::text_dim())));
    }

    spans
}

/// Pick the inclusive chip range `[lo, hi]` to show when the full strip
/// overflows: always include the active chip, then grow outward (right first,
/// then left) while it fits. Marker columns are reserved up front so the window
/// never overruns `width`.
fn visible_window(tabs: &[(char, &str, bool)], width: u16) -> (usize, usize) {
    let n = tabs.len();
    let active = tabs.iter().position(|(_, _, a)| *a).unwrap_or(0);

    // Reserve leading + both markers. Over-reserving by one marker at the ends
    // is harmless and keeps the fit check simple.
    let budget = width.saturating_sub(LEADING + MARKER_W * 2);

    let mut lo = active;
    let mut hi = active;
    let mut used = chip_width(tabs[active].1).min(budget);

    loop {
        let mut grew = false;
        if hi + 1 < n {
            let need = SEP + chip_width(tabs[hi + 1].1);
            if used + need <= budget {
                hi += 1;
                used += need;
                grew = true;
            }
        }
        if lo > 0 {
            let need = SEP + chip_width(tabs[lo - 1].1);
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The 9 Bedrock sub-tabs, with `active_idx` marked active.
    fn bedrock_tabs(active_idx: usize) -> Vec<(char, &'static str, bool)> {
        [
            ('1', "Foundation Models"),
            ('2', "Inference Profiles"),
            ('3', "Guardrails"),
            ('4', "Knowledge Bases"),
            ('5', "Agents"),
            ('6', "Prompts"),
            ('7', "Flows"),
            ('8', "Custom Models"),
            ('9', "Imported Models"),
        ]
        .iter()
        .enumerate()
        .map(|(i, (k, l))| (*k, *l, i == active_idx))
        .collect()
    }

    /// Total drawn width for a window, including leading + any overflow markers.
    fn drawn_width(tabs: &[(char, &str, bool)], lo: usize, hi: usize) -> u16 {
        let mut w = LEADING;
        if lo > 0 {
            w += MARKER_W;
        }
        for (slot, i) in (lo..=hi).enumerate() {
            if slot > 0 {
                w += SEP;
            }
            w += chip_width(tabs[i].1);
        }
        if hi + 1 < tabs.len() {
            w += MARKER_W;
        }
        w
    }

    #[test]
    fn window_always_includes_active_and_fits() {
        for active in 0..9 {
            let tabs = bedrock_tabs(active);
            for width in [40u16, 60, 80, 100, 120] {
                let (lo, hi) = visible_window(&tabs, width);
                assert!(lo <= active && active <= hi, "active {active} outside window at width {width}");
                assert!(
                    drawn_width(&tabs, lo, hi) <= width,
                    "window [{lo},{hi}] overruns width {width} (active {active})"
                );
            }
        }
    }

    #[test]
    fn window_edges_show_single_marker() {
        // Active at the far left: nothing hidden to the left.
        let (lo, _) = visible_window(&bedrock_tabs(0), 60);
        assert_eq!(lo, 0);
        // Active at the far right: window reaches the last tab.
        let (_, hi) = visible_window(&bedrock_tabs(8), 60);
        assert_eq!(hi, 8);
    }

    #[test]
    fn narrow_width_keeps_at_least_the_active_chip() {
        let (lo, hi) = visible_window(&bedrock_tabs(4), 20);
        assert!(lo <= 4 && 4 <= hi);
    }
}
