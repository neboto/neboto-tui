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

/// Jump-list picker: the navigation history, most-recent first. `↑`/`↓` move,
/// `⏎` jumps to the selected place, `Esc`/`` ` `` closes.
pub fn render_jump_list(app: &App, frame: &mut Frame) {
    let area = centered_rect(72, 60, frame.size());
    frame.render_widget(Clear, area);

    let block = theme::popup_block("Jump list").title(
        Title::from(Span::styled(
            " ↑↓ move · ⏎ go · Esc close ",
            Style::default().fg(theme::text_dim()),
        ))
        .position(Position::Bottom)
        .alignment(Alignment::Center),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let total = app.nav_history.len();
    if total == 0 {
        frame.render_widget(
            Paragraph::new(Line::styled(
                "No history yet",
                Style::default().fg(theme::text_dim()),
            ))
            .alignment(Alignment::Center),
            inner,
        );
        return;
    }

    // Display most-recent first; keep the selected row in view.
    let height = inner.height as usize;
    let selected = app.jump_list_selected.min(total - 1);
    let offset = selected.saturating_sub(height.saturating_sub(1));

    // The cursor (where `Ctrl-O` has parked us) as a display row, so the user
    // sees that going back walks a position rather than consuming the list.
    let cursor_display = app.nav_cursor.map(|vec_idx| total - 1 - vec_idx);

    let lines: Vec<Line> = app
        .nav_history
        .iter()
        .rev()
        .enumerate()
        .skip(offset)
        .take(height)
        .map(|(i, loc)| {
            let at_cursor = cursor_display == Some(i);
            // Marker column: ● = current position, • = a recorded waypoint.
            let marker = if at_cursor { "● " } else { "  " };
            if i == selected {
                Line::from(vec![
                    Span::styled(
                        if at_cursor { "●" } else { "▸" },
                        Style::default().fg(theme::aws_orange()),
                    ),
                    Span::raw(" "),
                    Span::styled(
                        loc.label.clone(),
                        theme::selection_style(true).add_modifier(Modifier::BOLD),
                    ),
                ])
            } else {
                Line::from(vec![
                    Span::styled(
                        marker,
                        Style::default().fg(if at_cursor {
                            theme::aws_orange()
                        } else {
                            theme::text_dim()
                        }),
                    ),
                    Span::styled(loc.label.clone(), Style::default().fg(crate::ui::theme::text_primary())),
                ])
            }
        })
        .collect();

    frame.render_widget(Paragraph::new(lines), inner);
}

/// Bookmarks picker: persistent, user-curated saved locations. `↑`/`↓` move,
/// `⏎` jumps, `d` deletes the selected bookmark, `Esc`/`'` closes.
pub fn render_bookmarks(app: &App, frame: &mut Frame) {
    let area = centered_rect(72, 60, frame.size());
    frame.render_widget(Clear, area);

    let block = theme::popup_block("Bookmarks").title(
        Title::from(Span::styled(
            " ↑↓ move · ⏎ go · d delete · Esc close ",
            Style::default().fg(theme::text_dim()),
        ))
        .position(Position::Bottom)
        .alignment(Alignment::Center),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let total = app.bookmarks.len();
    if total == 0 {
        frame.render_widget(
            Paragraph::new(Line::styled(
                "No bookmarks yet — press B on a resource to add one",
                Style::default().fg(theme::text_dim()),
            ))
            .alignment(Alignment::Center),
            inner,
        );
        return;
    }

    let height = inner.height as usize;
    let selected = app.bookmarks_selected.min(total - 1);
    let offset = selected.saturating_sub(height.saturating_sub(1));

    let lines: Vec<Line> = app
        .bookmarks
        .iter()
        .enumerate()
        .skip(offset)
        .take(height)
        .map(|(i, loc)| {
            if i == selected {
                Line::from(vec![
                    Span::styled("▸", Style::default().fg(theme::aws_orange())),
                    Span::raw(" "),
                    Span::styled(
                        loc.label.clone(),
                        theme::selection_style(true).add_modifier(Modifier::BOLD),
                    ),
                ])
            } else {
                Line::from(vec![
                    Span::styled("★ ", Style::default().fg(theme::text_dim())),
                    Span::styled(loc.label.clone(), Style::default().fg(crate::ui::theme::text_primary())),
                ])
            }
        })
        .collect();

    frame.render_widget(Paragraph::new(lines), inner);
}

/// Centered rectangle for the popup (percentages of the frame).
fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let vert = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vert[1])[1]
}
