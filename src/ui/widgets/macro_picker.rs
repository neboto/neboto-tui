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

/// Macro picker (`,`): saved macros on the left of the body, the selected
/// macro's steps listed beneath it so you can see what a macro will do before
/// running it. Doubles as the name prompt for a just-finished recording.
pub fn render_macro_picker(app: &App, frame: &mut Frame) {
    let area = centered_rect(72, 60, frame.size());
    frame.render_widget(Clear, area);

    // Naming a fresh recording takes over the whole popup — there is nothing
    // to pick until it has a name.
    if let Some(name) = &app.macro_name_input {
        render_name_prompt(app, name, area, frame);
        return;
    }

    let block = theme::popup_block("Macros").title(
        Title::from(Span::styled(
            " ↑↓ move · ⏎ run · n record · d delete · Esc close ",
            Style::default().fg(theme::text_dim()),
        ))
        .position(Position::Bottom)
        .alignment(Alignment::Center),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let total = app.macros.len();
    if total == 0 {
        frame.render_widget(
            Paragraph::new(vec![
                Line::styled(
                    "No macros yet",
                    Style::default().fg(theme::text_dim()),
                ),
                Line::raw(""),
                Line::styled(
                    "Press n to start recording, then , again to stop.",
                    Style::default().fg(theme::text_dim()),
                ),
            ])
            .alignment(Alignment::Center),
            inner,
        );
        return;
    }

    // Top half lists the macros, bottom half previews the selected one's steps.
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(inner);

    let height = chunks[0].height as usize;
    let selected = app.macro_picker_selected.min(total - 1);
    let offset = selected.saturating_sub(height.saturating_sub(1));

    let rows: Vec<Line> = app
        .macros
        .iter()
        .enumerate()
        .skip(offset)
        .take(height)
        .map(|(i, m)| {
            if i == selected {
                Line::from(vec![
                    Span::styled("▸", Style::default().fg(theme::aws_orange())),
                    Span::raw(" "),
                    Span::styled(
                        m.name.clone(),
                        theme::selection_style(true).add_modifier(Modifier::BOLD),
                    ),
                    Span::raw("  "),
                    Span::styled(m.summary(), Style::default().fg(theme::text_dim())),
                ])
            } else {
                Line::from(vec![
                    Span::styled("⏵ ", Style::default().fg(theme::text_dim())),
                    Span::styled(m.name.clone(), Style::default().fg(theme::text_primary())),
                    Span::raw("  "),
                    Span::styled(m.summary(), Style::default().fg(theme::text_dim())),
                ])
            }
        })
        .collect();
    frame.render_widget(Paragraph::new(rows), chunks[0]);

    // Step preview.
    let mut steps: Vec<Line> = vec![Line::from(Span::styled(
        "Steps",
        Style::default()
            .fg(theme::heading())
            .add_modifier(Modifier::BOLD),
    ))];
    if let Some(m) = app.macros.get(selected) {
        let room = (chunks[1].height as usize).saturating_sub(1);
        for (i, step) in m.steps.iter().take(room).enumerate() {
            steps.push(Line::from(vec![
                Span::styled(format!(" {:>2}. ", i + 1), Style::default().fg(theme::text_dim())),
                Span::styled(step.label(), Style::default().fg(theme::text_primary())),
            ]));
        }
        if m.steps.len() > room {
            steps.push(Line::styled(
                format!("     … {} more", m.steps.len() - room),
                Style::default().fg(theme::text_dim()),
            ));
        }
    }
    frame.render_widget(Paragraph::new(steps), chunks[1]);
}

/// Name prompt shown when `,` stops a recording.
fn render_name_prompt(app: &App, name: &str, area: Rect, frame: &mut Frame) {
    let block = theme::popup_block("Name this macro").title(
        Title::from(Span::styled(
            " ⏎ save · Esc discard ",
            Style::default().fg(theme::text_dim()),
        ))
        .position(Position::Bottom)
        .alignment(Alignment::Center),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let recorded = app
        .macro_recorder
        .as_ref()
        .map(|r| r.steps.len())
        .unwrap_or(0);

    let mut lines = vec![
        Line::from(vec![
            Span::styled(" › ", Style::default().fg(theme::aws_orange())),
            Span::styled(
                name.to_string(),
                Style::default()
                    .fg(theme::text_primary())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("▏", Style::default().fg(theme::aws_orange())),
        ]),
        Line::raw(""),
        Line::styled(
            format!(" {} step(s) recorded", recorded),
            Style::default().fg(theme::text_dim()),
        ),
        Line::raw(""),
    ];
    if let Some(rec) = &app.macro_recorder {
        let room = (inner.height as usize).saturating_sub(lines.len());
        for (i, step) in rec.steps.iter().take(room).enumerate() {
            lines.push(Line::from(vec![
                Span::styled(format!(" {:>2}. ", i + 1), Style::default().fg(theme::text_dim())),
                Span::styled(step.label(), Style::default().fg(theme::text_primary())),
            ]));
        }
    }
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
