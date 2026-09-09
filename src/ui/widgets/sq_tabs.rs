use crate::app::App;
use crate::ui::theme;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

/// Service Quotas sub-tab bar: shows the scoped service code + the toggle hints.
/// Unlike a normal sub-tab row this isn't a list filter — it's the variant
/// scope (which service's quotas are shown).
pub fn render_sq_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let spans: Vec<Span> = vec![
        Span::styled(" Service: ", Style::default().fg(theme::text_dim())),
        Span::styled(
            app.quota_service_code.clone(),
            Style::default()
                .fg(Color::Black)
                .bg(theme::aws_orange())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("    ", Style::default()),
        Span::styled("t", Style::default().fg(theme::accent())),
        Span::styled(" cycle common", Style::default().fg(theme::text_dim())),
        Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
        Span::styled("c", Style::default().fg(theme::accent())),
        Span::styled(" pick service", Style::default().fg(theme::text_dim())),
    ];

    // Map the `t` (cycle) and `c` (pick) hints to clickable regions. Column of
    // each span = cumulative width of the spans before it.
    let col_at = |n: usize| -> u16 {
        spans.iter().take(n).map(|s| s.width() as u16).sum()
    };
    // `t cycle common` = spans[3..=4]; `c pick service` = spans[6..=7].
    app.push_click_region(
        Rect {
            x: area.x + col_at(3),
            y: area.y,
            width: col_at(5).saturating_sub(col_at(3)),
            height: 1,
        },
        crate::app::ClickAction::Key('t'),
    );
    app.push_click_region(
        Rect {
            x: area.x + col_at(6),
            y: area.y,
            width: col_at(8).saturating_sub(col_at(6)),
            height: 1,
        },
        crate::app::ClickAction::Key('c'),
    );

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}
