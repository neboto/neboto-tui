use crate::app::{App, MessageLevel};
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

/// Message-history viewer (`M`): every status-bar error and success toast,
/// newest first, so a 3-second toast is reviewable after it vanishes.
/// `↑`/`↓` move, `y` copies the selected message, `Esc`/`M` closes.
pub fn render_message_log(app: &App, frame: &mut Frame) {
    let area = centered_rect(76, 60, frame.size());
    frame.render_widget(Clear, area);

    let block = theme::popup_block("Messages").title(
        Title::from(Span::styled(
            " ↑↓ move · y copy · Esc close ",
            Style::default().fg(theme::text_dim()),
        ))
        .position(Position::Bottom)
        .alignment(Alignment::Center),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let total = app.message_history.len();
    if total == 0 {
        frame.render_widget(
            Paragraph::new(Line::styled(
                "No messages yet",
                Style::default().fg(theme::text_dim()),
            ))
            .alignment(Alignment::Center),
            inner,
        );
        return;
    }

    let height = inner.height as usize;
    let selected = app.message_history_selected.min(total - 1);
    let offset = selected.saturating_sub(height.saturating_sub(1));

    // Right-aligned relative-age column width ("12m ago" = 7).
    const AGE_W: usize = 8;

    let lines: Vec<Line> = app
        .message_history
        .iter()
        .enumerate()
        .skip(offset)
        .take(height)
        .map(|(i, entry)| {
            let (icon, icon_color) = match entry.level {
                MessageLevel::Error => ("✗", theme::error()),
                MessageLevel::Success => ("✓", theme::success()),
            };
            let age = format!("{:>AGE_W$}", fmt_ago(entry.at.elapsed().as_secs()));
            // Single-line rows: truncate long messages (y copies the full text).
            let text_w = (inner.width as usize).saturating_sub(AGE_W + 5);
            let text: String = if entry.text.chars().count() > text_w {
                let mut t: String = entry.text.chars().take(text_w.saturating_sub(1)).collect();
                t.push('…');
                t
            } else {
                entry.text.clone()
            };

            if i == selected {
                Line::from(vec![
                    Span::styled("▸ ", Style::default().fg(theme::aws_orange())),
                    Span::styled(icon.to_string(), Style::default().fg(icon_color)),
                    Span::raw(" "),
                    Span::styled(
                        text,
                        theme::selection_style(true).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(age, Style::default().fg(theme::text_dim())),
                ])
            } else {
                Line::from(vec![
                    Span::raw("  "),
                    Span::styled(icon.to_string(), Style::default().fg(icon_color)),
                    Span::raw(" "),
                    Span::styled(text, Style::default().fg(crate::ui::theme::text_primary())),
                    Span::styled(age, Style::default().fg(theme::text_dim())),
                ])
            }
        })
        .collect();

    frame.render_widget(Paragraph::new(lines), inner);
}

/// Relative age label: "now", "42s ago", "3m ago", "2h ago".
fn fmt_ago(secs: u64) -> String {
    if secs < 5 {
        "now".to_string()
    } else if secs < 60 {
        format!("{}s ago", secs)
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else {
        format!("{}h ago", secs / 3600)
    }
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
