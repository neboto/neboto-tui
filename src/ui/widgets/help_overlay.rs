use crate::app::App;
use crate::search::fuzzy::FuzzyMatcher;
use crate::ui::theme;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{
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
            ("⌫ / Ctrl-O", "history back"),
            ("Ctrl-F", "history forward"),
            ("`", "jump list (history)"),
            ("B / '", "bookmark / bookmarks"),
            ("Tab · 1–9", "sub-tabs"),
            ("H / L", "prev / next sub-tab"),
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
            ("H / L", "prev / next sub-tab"),
            ("h / ← / Esc", "back to list"),
            ("[[ / ]]", "prev / next header"),
            ("/", "filter body text"),
            ("V · J / K", "visual select · extend"),
            ("Ctrl-A", "select all"),
            ("y / c", "copy selection or row"),
            ("\\", "flat view (all sections)"),
            ("Ctrl-W", "wrap long values"),
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
            ("C", "copy a CLI command (read · connect · change)"),
            ("e", "open in $EDITOR"),
            ("m", "metrics charts (c: change markers from the W timeline)"),
            ("t", "live log tail ([/] window)"),
            ("f", "log search · event filter"),
            ("W", "CloudTrail lens (who did this)"),
            ("U", "Referenced-by lens (what uses this, from loaded services)"),
            ("N", "Network-access lens (merged security-group rules + subnet NACLs)"),
            ("o / i", "S3 objects / DynamoDB items"),
            ("s", "SSM session · ECS exec"),
            ("x / Y", "reveal / copy secret value · x: test a DNS record's answer"),
            ("d", "download Lambda package"),
            ("X / ^X", "export detail / list"),
            ("M", "message history"),
            (",", "macros (⏎ run · n record)"),
            ("b", "toggle banner"),
            ("? / q", "help (type to filter) / quit"),
        ],
    },
    Section {
        title: "Search",
        entries: &[
            ("/", "fuzzy search"),
            ("@svc text", "switch service + search"),
            ("@all text", "search all cached services (⇥ steps through them)"),
            ("tag:k=v", "exact tag filter"),
            ("Tab", "complete @prefix"),
            ("↑ / ↓", "earlier searches (this session)"),
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
        title: "Mouse",
        entries: &[
            ("click", "select · tabs · hints · badges"),
            ("double-click", "⏎ (open · follow · confirm)"),
            ("click → · title bar", "follow link · full width"),
            ("click ⌫ back · ⌃F fwd", "back · forward through history"),
            ("click ↑ vX.Y.Z", "copy the upgrade command"),
            ("wheel · drag", "scroll · select rows"),
            ("right-click", "back · close a popup or view"),
            ("Shift-drag", "terminal text select (iTerm: ⌥)"),
        ],
    },
    Section {
        title: "Toggles",
        entries: &[
            ("1–6 · 7", "cost group-by (5 tag, 6 category) · anomalies"),
            ("[ ] · t", "cost period: step · cycle"),
            ("t", "WAF scope · RAM owner"),
            ("f", "ECS task · execution filter"),
        ],
    },
];

/// One section as drawn: the full table, or the entries left by a filter.
struct Group<'a> {
    title: &'a str,
    entries: Vec<(&'a str, &'a str)>,
}

fn groups(sections: &'static [Section]) -> Vec<Group<'static>> {
    sections
        .iter()
        .map(|s| Group { title: s.title, entries: s.entries.to_vec() })
        .collect()
}

/// The entries `query` matches, both columns in reading order (left, then
/// right), each kept under its section title. An entry matches on its key,
/// its description or its section's title, so `mouse` lists the whole Mouse
/// section. Order is the table's, not the score's: the result still reads
/// as the help, just shorter.
fn filter_groups(query: &str) -> Vec<Group<'static>> {
    let all: Vec<(&'static str, &'static str, &'static str)> = LEFT_SECTIONS
        .iter()
        .chain(RIGHT_SECTIONS)
        .flat_map(|s| s.entries.iter().map(move |(k, d)| (s.title, *k, *d)))
        .collect();
    let mut hits: Vec<usize> = FuzzyMatcher::new()
        .filter_fields(query, &all, |(title, key, desc)| vec![*key, *desc, *title])
        .into_iter()
        .map(|(i, _)| i)
        .collect();
    hits.sort_unstable();
    let mut out: Vec<Group<'static>> = Vec::new();
    for i in hits {
        let (title, key, desc) = all[i];
        match out.last_mut() {
            Some(g) if g.title == title => g.entries.push((key, desc)),
            _ => out.push(Group { title, entries: vec![(key, desc)] }),
        }
    }
    out
}

/// Render the help overlay: a filter row over two aligned key/description
/// columns, sized to the content (capped at 80% of the terminal), scrollable
/// when it doesn't fit. Typing filters it to one full-width column, so a
/// matched entry has room for its whole description.
pub fn render_help_overlay(app: &App, frame: &mut Frame) {
    let size = frame.area();

    // Width: 76% of the terminal, capped so ultrawide screens don't stretch
    // the two columns apart. Column width drives description truncation.
    let width = (size.width * 76 / 100).clamp(40, 110);
    let col_width = (width.saturating_sub(2 + 4) / 2) as usize; // borders + margins

    let left_lines = section_lines(&groups(LEFT_SECTIONS), col_width);
    let right_lines = section_lines(&groups(RIGHT_SECTIONS), col_width);
    let full_h = left_lines.len().max(right_lines.len()) as u16;

    let query = app.help_query.as_str();
    let filtered = (!query.trim().is_empty()).then(|| {
        let matches = filter_groups(query);
        if matches.is_empty() {
            vec![
                Line::raw(""),
                Line::from(Span::styled(
                    format!("  No keys match \"{}\"", query.trim()),
                    Style::default().fg(theme::text_dim()),
                )),
            ]
        } else {
            section_lines(&matches, width.saturating_sub(2 + 4) as usize)
        }
    });
    let content_h = filtered.as_ref().map_or(full_h, |l| l.len() as u16);

    // Height: fit the unfiltered content plus the filter row, so the popup
    // holds still while you type; cap at 80% of the terminal and scroll.
    let max_h = (size.height * 80 / 100).max(10);
    let height = (full_h + 1 + 2).min(max_h);
    let area = centered_rect(width, height, size);
    let viewport = height.saturating_sub(2 + 1);
    let max_scroll = content_h.saturating_sub(viewport);
    app.help_max_scroll.set(max_scroll);
    app.popup_hits.borrow_mut().record_area(area);
    let offset = app.help_scroll.min(max_scroll);

    frame.render_widget(Clear, area);

    let mut block = theme::popup_block("Help").title_bottom(
        Line::from(Span::styled(
            if max_scroll > 0 {
                " type to filter · ↑/↓ scroll · Esc close "
            } else {
                " type to filter · Esc close "
            },
            Style::default().fg(theme::text_dim()),
        ))
        .centered(),
    );
    if max_scroll > 0 {
        // Overflow indicator: which slice of the content is on screen.
        block = block.title_bottom(
            Line::from(Span::styled(
                format!(" {}–{}/{} ", offset + 1, (offset + viewport).min(content_h), content_h),
                Style::default().fg(theme::text_dim()),
            ))
            .right_aligned(),
        );
    }
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(inner);
    frame.render_widget(Paragraph::new(filter_line(query)), rows[0]);

    if let Some(lines) = filtered {
        let body = Layout::default()
            .horizontal_margin(2)
            .constraints([Constraint::Min(0)])
            .split(rows[1]);
        frame.render_widget(Paragraph::new(lines).scroll((offset, 0)), body[0]);
        return;
    }

    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .horizontal_margin(2)
        .split(rows[1]);

    frame.render_widget(
        Paragraph::new(left_lines).scroll((offset, 0)),
        columns[0],
    );
    frame.render_widget(
        Paragraph::new(right_lines).scroll((offset, 0)),
        columns[1],
    );
}

/// The filter row: the query with a cursor, or a dim placeholder.
fn filter_line(query: &str) -> Line<'static> {
    let prompt = Span::styled("  / ", Style::default().fg(theme::accent()));
    if query.is_empty() {
        Line::from(vec![
            prompt,
            Span::styled("type to filter keys", Style::default().fg(theme::text_dim())),
        ])
    } else {
        Line::from(vec![
            prompt,
            Span::styled(query.to_string(), Style::default().fg(theme::text_primary())),
            Span::styled("▏", Style::default().fg(theme::accent())),
        ])
    }
}

/// Build one column's lines: section titles + `key  description` rows with
/// the keys right-padded to a shared column. No wrapping — a description
/// that can't fit is truncated with `…`.
fn section_lines(sections: &[Group<'static>], col_width: usize) -> Vec<Line<'static>> {
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
        for (key, desc) in &section.entries {
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
                for line in section_lines(&groups(sections), col_width) {
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

    fn keys<'a>(groups: &[Group<'a>]) -> Vec<&'a str> {
        groups.iter().flat_map(|g| g.entries.iter().map(|(k, _)| *k)).collect()
    }

    #[test]
    fn filter_matches_descriptions_and_keeps_section_titles() {
        let hits = filter_groups("log tail");
        assert_eq!(keys(&hits), vec!["t"], "{:?}", keys(&hits));
        assert_eq!(hits[0].title, "Actions");
    }

    #[test]
    fn filter_on_a_section_title_lists_the_section() {
        let hits = filter_groups("mouse");
        let mouse = RIGHT_SECTIONS.iter().find(|s| s.title == "Mouse").unwrap();
        let section = hits.iter().find(|g| g.title == "Mouse").unwrap();
        assert_eq!(section.entries.len(), mouse.entries.len());
    }

    #[test]
    fn filter_keeps_table_order_across_both_columns() {
        // `refresh` hits both left-column sections and at least one on the
        // right; the sections come out in table order whatever their scores.
        let order: Vec<&str> = LEFT_SECTIONS.iter().chain(RIGHT_SECTIONS).map(|s| s.title).collect();
        let hits = filter_groups("refresh");
        let pos: Vec<usize> =
            hits.iter().map(|g| order.iter().position(|t| *t == g.title).unwrap()).collect();
        assert!(pos.len() >= 2 && pos.windows(2).all(|w| w[0] < w[1]), "{pos:?}");
        assert_eq!(&order[pos[0]], &"List pane");
        assert!(filter_groups("zzqqxx").is_empty());
    }

    #[test]
    fn truncate_ellipsizes() {
        assert_eq!(truncate("short", 10), "short");
        assert_eq!(truncate("exactly-10", 10), "exactly-10");
        assert_eq!(truncate("longer than that", 10), "longer th…");
    }
}
