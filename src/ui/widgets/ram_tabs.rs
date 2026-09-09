use crate::app::App;
use crate::ui::theme;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

pub fn render_ram_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let mut spans: Vec<Span> = vec![Span::raw(" ")];

    spans.push(Span::styled("t Owner ", Style::default().fg(theme::text_dim())));

    let owners = [("SELF", "Shared by me"), ("OTHER-ACCOUNTS", "Shared with me")];
    for (i, (key, label)) in owners.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" ", Style::default()));
        }
        let active = app.ram_owner == *key;
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

    // Clicking anywhere on the toggle cycles the owner scope (`t`) — same
    // pattern as WAF's scope toggle.
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
