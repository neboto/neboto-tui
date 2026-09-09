use crate::app::App;
use crate::ui::theme;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{
        block::{Position, Title},
        Clear, Paragraph,
    },
    Frame,
};

struct Section {
    title: &'static str,
    entries: &'static [(&'static str, &'static str)],
}

// Help entries are one line each — short labels, never sentences. There is no
// word wrap: anything that can't fit the column is truncated with `…`, so
// keep descriptions tight and put detail in the feature itself.
const LEFT_SECTIONS: &[Section] = &[
    Section {
        title: "List pane",
        entries: &[
            ("j / k ↑ / ↓", "move"),
            ("gg / G", "top / bottom"),
            ("^d / ^u", "half page"),
            ("l / → / ⏎", "open details · follow →"),
            ("h / ← / ⌫", "back"),
            ("Ctrl-O", "history back"),
            ("`", "jump list (history)"),
            ("B / '", "bookmark / bookmarks"),
            ("Tab · 1–9", "sub-tabs"),
            ("V · J / K", "visual select · extend"),
            ("Ctrl-A", "select all rows"),
            ("y · X · ^X", "copy · export selection"),
            ("a", "hide noisy rows"),
            ("z", "cycle sort"),
            ("F", "cycle state filter"),
            ("w", "watch mode (+/- interval)"),
            ("r / F5", "refresh service"),
        ],
    },
    Section {
        title: "Detail pane",
        entries: &[
            ("Tab · 1–9", "sections"),
            ("[[ / ]]", "prev / next header"),
            ("/", "filter body text"),
            ("V · J / K", "visual select · extend"),
            ("Ctrl-A", "select all"),
            ("y / c", "copy selection or row"),
            ("\\", "flat view (all sections)"),
            ("Z", "full-width pane"),
            ("r", "refresh this resource"),
        ],
    },
];

const RIGHT_SECTIONS: &[Section] = &[
    Section {
        title: "Actions",
        entries: &[
            ("O", "open in AWS Console"),
            ("y", "copy ARN / id"),
            ("C", "copy AWS CLI command"),
            ("e", "open in $EDITOR"),
            ("m", "metrics charts"),
            ("t", "live log tail ([/] window)"),
            ("f", "log search · event filter"),
            ("W", "CloudTrail lens (who did this)"),
            ("U", "Referenced-by lens (what uses this, from loaded services)"),
            ("N", "Network-access lens (merged security-group rules)"),
            ("o / i", "S3 objects / DynamoDB items"),
            ("s", "SSM session · ECS exec"),
            ("x / Y", "reveal / copy secret value"),
            ("d", "download Lambda package"),
            ("X / ^X", "export detail / list"),
            ("M", "message history"),
            (",", "macros (⏎ run · n record)"),
            ("b", "toggle banner"),
            ("? / q", "help / quit"),
        ],
    },
    Section {
        title: "Search",
        entries: &[
            ("/", "fuzzy search"),
            ("@svc text", "switch service + search"),
            ("@all text", "search all cached services"),
            ("tag:k=v", "exact tag filter"),
            ("Tab", "complete @prefix"),
        ],
    },
    Section {
        title: "Context",
        entries: &[
            ("S / R / P", "service / region / profile"),
            ("@ec2 …", "switch via prefix"),
            ("s", "assume role (org account)"),
        ],
    },
    Section {
        title: "Toggles",
        entries: &[
            ("1–4 · t", "cost group-by · period"),
            ("t", "WAF scope · RAM owner"),
            ("f", "ECS task · execution filter"),
        ],
    },
];

/// Render the help overlay: two aligned key/description columns, sized to the
/// content (capped at 80% of the terminal), scrollable when it doesn't fit.
pub fn render_help_overlay(app: &App, frame: &mut Frame) {
    let size = frame.size();

    // Width: 76% of the terminal, capped so ultrawide screens don't stretch
    // the two columns apart. Column width drives description truncation.
    let width = (size.width * 76 / 100).clamp(40, 110);
    let col_width = (width.saturating_sub(2 + 4) / 2) as usize; // borders + margins

    let left_lines = section_lines(LEFT_SECTIONS, col_width);
    let right_lines = section_lines(RIGHT_SECTIONS, col_width);
    let content_h = left_lines.len().max(right_lines.len()) as u16;

    // Height: fit the content; cap at 80% of the terminal and scroll the rest.
    let max_h = (size.height * 80 / 100).max(10);
    let height = (content_h + 2).min(max_h);
    let area = centered_rect(width, height, size);
    let viewport = height.saturating_sub(2);
    let max_scroll = content_h.saturating_sub(viewport);
    app.help_max_scroll.set(max_scroll);
    let offset = app.help_scroll.min(max_scroll);

    frame.render_widget(Clear, area);

    let mut block = theme::popup_block("Help").title(
        Title::from(Span::styled(
            if max_scroll > 0 {
                " j/k scroll · ? / Esc close "
            } else {
                " ? / Esc close "
            },
            Style::default().fg(theme::text_dim()),
        ))
        .position(Position::Bottom)
        .alignment(Alignment::Center),
    );
    if max_scroll > 0 {
        // Overflow indicator: which slice of the content is on screen.
        block = block.title(
            Title::from(Span::styled(
                format!(" {}–{}/{} ", offset + 1, (offset + viewport).min(content_h), content_h),
                Style::default().fg(theme::text_dim()),
            ))
            .position(Position::Bottom)
            .alignment(Alignment::Right),
        );
    }
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .horizontal_margin(2)
        .split(inner);

    frame.render_widget(
        Paragraph::new(left_lines).scroll((offset, 0)),
        columns[0],
    );
    frame.render_widget(
        Paragraph::new(right_lines).scroll((offset, 0)),
        columns[1],
    );
}

/// Build one column's lines: section titles + `key  description` rows with
/// the keys right-padded to a shared column. No wrapping — a description
/// that can't fit is truncated with `…`.
fn section_lines(sections: &[Section], col_width: usize) -> Vec<Line<'static>> {
    let key_width = sections
        .iter()
        .flat_map(|s| s.entries.iter())
        .map(|(key, _)| key.chars().count())
        .max()
        .unwrap_or(0);
    let desc_budget = col_width.saturating_sub(2 + key_width + 2);

    let mut lines = vec![Line::raw("")];
    for section in sections {
        lines.push(Line::from(Span::styled(
            section.title,
            Style::default()
                .fg(theme::warning())
                .add_modifier(Modifier::BOLD),
        )));
        for (key, desc) in section.entries {
            let pad = " ".repeat(key_width.saturating_sub(key.chars().count()) + 2);
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(
                    *key,
                    Style::default()
                        .fg(theme::accent())
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(pad),
                Span::styled(truncate(desc, desc_budget), Style::default().fg(crate::ui::theme::text_muted())),
            ]));
        }
        lines.push(Line::raw(""));
    }
    // Drop the trailing blank so content height is exact.
    lines.pop();
    lines
}

/// Truncate to `max` display characters, ellipsized.
fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else if max == 0 {
        String::new()
    } else {
        let mut s: String = text.chars().take(max - 1).collect();
        s.push('…');
        s
    }
}

/// Centered rectangle with a fixed size (not percentage-based — the popup is
/// sized to its content by the caller).
fn centered_rect(width: u16, height: u16, r: Rect) -> Rect {
    let width = width.min(r.width);
    let height = height.min(r.height);
    Rect {
        x: r.x + (r.width - width) / 2,
        y: r.y + (r.height - height) / 2,
        width,
        height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_line_exceeds_the_column_budget() {
        // The whole point of the redesign: nothing wraps. Every built line
        // must fit the column it was built for, at a typical column width.
        for col_width in [30usize, 40, 52] {
            for sections in [LEFT_SECTIONS, RIGHT_SECTIONS] {
                for line in section_lines(sections, col_width) {
                    assert!(
                        line.width() <= col_width,
                        "line {:?} is {} wide, budget {}",
                        line,
                        line.width(),
                        col_width
                    );
                }
            }
        }
    }

    #[test]
    fn truncate_ellipsizes() {
        assert_eq!(truncate("short", 10), "short");
        assert_eq!(truncate("exactly-10", 10), "exactly-10");
        assert_eq!(truncate("longer than that", 10), "longer th…");
    }
}
