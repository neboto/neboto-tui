//! The keycast box (`--show-keys`): the last few keys and what they did,
//! floating bottom-right just above the status bar. See `src/keycast.rs`.

use crate::app::App;
use crate::ui::theme;
use ratatui::{
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph},
    Frame,
};
use std::time::Instant;

pub fn render_keycast(app: &App, status_area: Rect, frame: &mut Frame) {
    let Some(kc) = &app.keycast else {
        return;
    };
    let entries: Vec<_> = kc.visible(Instant::now()).collect();
    let Some(newest) = entries.len().checked_sub(1) else {
        return;
    };

    // One chip per key, newest last. The newest is the loud one — the same
    // orange chip as an active sub-tab — and the trail behind it is quieter.
    let chips: Vec<Vec<Span>> = entries
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let (key_style, action_style) = if i == newest {
                (
                    Style::default()
                        .fg(Color::Black)
                        .bg(theme::aws_orange())
                        .add_modifier(Modifier::BOLD),
                    Style::default().fg(theme::text_primary()),
                )
            } else {
                (
                    Style::default().fg(theme::aws_orange()).add_modifier(Modifier::BOLD),
                    Style::default().fg(theme::text_dim()),
                )
            };
            let mut chip = vec![Span::styled(format!(" {} ", e.key), key_style)];
            if e.count > 1 {
                chip.push(Span::styled(format!(" ×{}", e.count), action_style));
            }
            if let Some(action) = &e.action {
                chip.push(Span::styled(format!(" {action}"), action_style));
            }
            chip
        })
        .collect();

    // Border + one cell of padding each side. When the trail won't fit, drop
    // chips from the oldest end rather than clipping the newest.
    let max_inner = frame.size().width.saturating_sub(8) as usize;
    let width = |c: &[Vec<Span>]| -> usize {
        c.iter().map(|chip| chip.iter().map(Span::width).sum::<usize>()).sum::<usize>()
            + 2 * c.len().saturating_sub(1)
    };
    let mut first = 0;
    while first < newest && width(&chips[first..]) > max_inner {
        first += 1;
    }
    let mut spans: Vec<Span> = Vec::new();
    for (i, chip) in chips.into_iter().skip(first).enumerate() {
        if i > 0 {
            spans.push(Span::raw("  "));
        }
        spans.extend(chip);
    }
    let line = Line::from(spans);

    let w = (line.width() as u16 + 4).min(frame.size().width.saturating_sub(4));
    if w < 8 || status_area.y < 4 {
        return;
    }
    let area = Rect {
        x: status_area.right().saturating_sub(w + 2),
        y: status_area.y - 4,
        width: w,
        height: 3,
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::aws_orange()))
        .title(Span::styled(" keys ", Style::default().fg(theme::text_dim())));
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(line)
            .alignment(Alignment::Center)
            .block(block),
        area,
    );
}
