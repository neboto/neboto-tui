use crate::app::{App, XRayView};
use crate::aws::services::xray::XRayWindow;
use crate::ui::theme;
use crate::ui::widgets::subtab_bar::subtab_bar_spans;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

/// X-Ray tab strip + the look-back window chips (`[`/`]` step through them;
/// clicking a chip's side steps too).
pub fn render_xray_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let v = app.xray_view;
    let tabs = [
        ('1', "Service Map", v == XRayView::ServiceMap),
        ('2', "Traces", v == XRayView::Traces),
        ('3', "Groups & Sampling", v == XRayView::Config),
    ];
    let mut spans = subtab_bar_spans(app, area, &tabs);

    spans.push(Span::styled("     ", Style::default()));
    spans.push(Span::styled("[ ] Window ", Style::default().fg(theme::text_dim())));
    let chips_col = Line::from(spans.clone()).width() as u16;
    for (i, w) in XRayWindow::ALL.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" ", Style::default()));
        }
        let style = if app.xray_window == *w {
            Style::default()
                .fg(Color::Black)
                .bg(theme::aws_orange())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme::text_dim())
        };
        spans.push(Span::styled(format!(" {} ", w.label()), style));
    }

    // Left half of the chips narrows, right half widens.
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
