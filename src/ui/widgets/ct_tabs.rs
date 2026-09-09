use crate::app::{App, CloudTrailView};
use crate::ui::theme;
use crate::ui::widgets::subtab_bar::subtab_bar_spans;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

/// CloudTrail sub-tabs (Events / Trails) + the Events view's server-side
/// query chip (`f` — attribute filter + time range, the variant scope that
/// fed the list).
pub fn render_ct_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let v = app.cloudtrail_view;
    let tabs = [
        ('1', "Events", v == CloudTrailView::Events),
        ('2', "Trails", v == CloudTrailView::Trails),
        ('3', "Insights", v == CloudTrailView::Insights),
    ];
    let mut spans = subtab_bar_spans(app, area, &tabs);

    // Trails aren't affected by the query — only chip the Events view.
    if v == CloudTrailView::Events {
        spans.push(Span::styled("     ", Style::default()));
        let chip_col = Line::from(spans.clone()).width() as u16;
        spans.push(Span::styled("f", Style::default().fg(theme::accent())));
        spans.push(Span::styled(" filter: ", Style::default().fg(theme::text_dim())));
        let scope = app
            .ct_query
            .chip()
            .unwrap_or_else(|| "recent (90d)".to_string());
        spans.push(Span::styled(
            format!(" {} ", scope),
            Style::default()
                .fg(Color::Black)
                .bg(theme::aws_orange())
                .add_modifier(Modifier::BOLD),
        ));

        // Clicking anywhere on the chip opens the filter modal (`f`).
        let chip_w = (Line::from(spans.clone()).width() as u16).saturating_sub(chip_col);
        app.push_click_region(
            Rect {
                x: area.x + chip_col,
                y: area.y,
                width: chip_w,
                height: 1,
            },
            crate::app::ClickAction::Key('f'),
        );
    }

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}
