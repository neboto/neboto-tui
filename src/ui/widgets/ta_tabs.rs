use crate::app::App;
use crate::aws::services::trusted_advisor::TaScope;
use crate::ui::theme;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

pub fn render_ta_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let mut spans: Vec<Span> = vec![Span::raw(" ")];

    spans.push(Span::styled("t Scope ", Style::default().fg(theme::text_dim())));

    let scopes = [
        (TaScope::Account, "This account"),
        (TaScope::Organization, "Organization"),
        (TaScope::Priority, "Priority"),
    ];
    for (i, (scope, label)) in scopes.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" ", Style::default()));
        }
        let active = app.ta_scope == *scope;
        let style = if active {
            Style::default()
                .fg(Color::Black)
                .bg(theme::aws_orange())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme::text_dim())
        };
        spans.push(Span::styled(format!(" {} ", label), style));
    }

    match app.ta_scope {
        TaScope::Organization => spans.push(Span::styled(
            "  aggregated across org accounts — worst status wins",
            Style::default().fg(theme::text_dim()),
        )),
        TaScope::Priority => spans.push(Span::styled(
            "  account-team curated (Enterprise Support) — F filters active/closed",
            Style::default().fg(theme::text_dim()),
        )),
        TaScope::Account => {}
    }

    // Clicking anywhere on the toggle cycles the scope (`t`) — same pattern
    // as RAM's owner toggle.
    let toggle_w = Line::from(spans.clone()).width() as u16;
    app.push_click_region(
        Rect {
            x: area.x,
            y: area.y,
            width: toggle_w,
            height: 1,
        },
        crate::app::ClickAction::Key('t'),
    );

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}
