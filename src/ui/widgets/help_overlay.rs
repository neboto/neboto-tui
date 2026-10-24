use crate::app::App;
use crate::commands::{self, Section};
use crate::search::fuzzy::FuzzyMatcher;
use crate::ui::theme;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Clear, Paragraph,
    },
    Frame,
};

/// One section as drawn: the full table, or the entries left by a filter.
struct Group<'a> {
    title: &'a str,
    entries: Vec<(&'a str, &'a str)>,
}

fn groups(sections: &[Section]) -> Vec<Group<'static>> {
    sections
        .iter()
        .map(|s| Group {
            title: s.title(),
            entries: commands::in_section(*s).map(|c| (c.key, c.label)).collect(),
        })
        .collect()
}

/// The left column: "Here" (the keys that work on the selection, from the
/// command table's gates) over the table's left sections. "Here" repeats
/// rows listed below it, so it's left out while filtering.
fn left_groups(app: &App) -> Vec<Group<'static>> {
    let here: Vec<(&str, &str)> = commands::available(app).map(|c| (c.key, c.label)).collect();
    let mut out = Vec::new();
    if !here.is_empty() {
        out.push(Group { title: HERE, entries: here });
    }
    out.extend(groups(Section::LEFT));
    out
}

const HERE: &str = "Here";

/// The entries `query` matches, both columns in reading order (left, then
/// right), each kept under its section title. An entry matches on its key,
/// its description or its section's title, so `mouse` lists the whole Mouse
/// section. Order is the table's, not the score's: the result still reads
/// as the help, just shorter.
fn filter_groups(query: &str) -> Vec<Group<'static>> {
    let all: Vec<(&'static str, &'static str, &'static str)> = Section::LEFT
        .iter()
        .chain(Section::RIGHT)
        .flat_map(|s| commands::in_section(*s).map(move |c| (s.title(), c.key, c.label)))
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

    let left_lines = section_lines(&left_groups(app), col_width);
    let right_lines = section_lines(&groups(Section::RIGHT), col_width);
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
                " type to filter · : run a command · ↑/↓ scroll · Esc close "
            } else {
                " type to filter · : run a command · Esc close "
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
    frame.render_widget(Paragraph::new(filter_line(query, rows[0].width as usize)), rows[0]);

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

/// The filter row: the query with a cursor (or a dim placeholder) on the
/// left, and the `:` command picker called out on the right, set apart so
/// it's the first thing a reader of the help notices. It gives way to the
/// query when the row is too narrow for both.
fn filter_line(query: &str, width: usize) -> Line<'static> {
    let mut spans = vec![Span::styled("  / ", Style::default().fg(theme::accent()))];
    if query.is_empty() {
        spans.push(Span::styled("type to filter keys", Style::default().fg(theme::text_dim())));
    } else {
        spans.push(Span::styled(query.to_string(), Style::default().fg(theme::text_primary())));
        spans.push(Span::styled("▏", Style::default().fg(theme::accent())));
    }
    let tip = [
        Span::styled(
            " : ",
            Style::default()
                .fg(Color::Black)
                .bg(theme::accent())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            " run any key by name ",
            Style::default().fg(theme::accent()).add_modifier(Modifier::BOLD),
        ),
    ];
    let used: usize = spans.iter().map(|s| s.width()).sum();
    let tip_w: usize = tip.iter().map(|s| s.width()).sum();
    if used + 2 + tip_w < width {
        spans.push(Span::raw(" ".repeat(width - used - tip_w - 1)));
        spans.extend(tip);
    }
    Line::from(spans)
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
        // "Here" is about the selection, not the keymap: set it apart.
        let title_fg = if section.title == HERE { theme::accent() } else { theme::warning() };
        lines.push(Line::from(Span::styled(
            section.title,
            Style::default().fg(title_fg).add_modifier(Modifier::BOLD),
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
            for sections in [Section::LEFT, Section::RIGHT] {
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
        let section = hits.iter().find(|g| g.title == "Mouse").unwrap();
        assert_eq!(section.entries.len(), commands::in_section(Section::Mouse).count());
    }

    #[test]
    fn filter_keeps_table_order_across_both_columns() {
        // `refresh` hits both left-column sections and at least one on the
        // right; the sections come out in table order whatever their scores.
        let order: Vec<&str> = Section::LEFT.iter().chain(Section::RIGHT).map(|s| s.title()).collect();
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
