use crate::app::{App, ChangesView};
use crate::aws::services::changes::ChangesWindow;
use crate::ui::theme;
use crate::ui::widgets::subtab_bar::subtab_bar_spans;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

/// `@changes` tab strip (one tab per source group) + the window chips
/// (`[`/`]` step them; clicking a chip's side steps too) — the X-Ray shape.
pub fn render_changes_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let keys = ['1', '2', '3', '4', '5', '6'];
    let tabs: Vec<(char, &str, bool)> = ChangesView::ALL
        .iter()
        .zip(keys)
        .map(|(v, k)| (k, v.label(), app.changes_view == *v))
        .collect();
    let mut spans = subtab_bar_spans(app, area, &tabs);

    spans.push(Span::styled("     ", Style::default()));
    spans.push(Span::styled("[ ] Window ", Style::default().fg(theme::text_dim())));
    let chips_col = Line::from(spans.clone()).width() as u16;
    for (i, w) in ChangesWindow::ALL.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" ", Style::default()));
        }
        let style = if app.changes_window == *w {
            Style::default()
                .fg(Color::Black)
                .bg(theme::aws_orange())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme::text_dim())
        };
        spans.push(Span::styled(format!(" {} ", w.label()), style));
    }

    let chips_w = (Line::from(spans.clone()).width() as u16).saturating_sub(chips_col);
    let half = chips_w / 2;
    app.push_click_region(
        Rect { x: area.x + chips_col, y: area.y, width: half, height: 1 },
        crate::app::ClickAction::Key('['),
    );
    app.push_click_region(
        Rect { x: area.x + chips_col + half, y: area.y, width: chips_w - half, height: 1 },
        crate::app::ClickAction::Key(']'),
    );

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}
