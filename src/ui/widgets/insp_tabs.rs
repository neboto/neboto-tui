use crate::app::{App, InspSeverityScope, InspectorView};
use crate::ui::theme;
use crate::ui::widgets::subtab_bar::subtab_bar_spans;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

pub fn render_insp_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let v = app.inspector_view;
    let tabs = [
        ('1', "Findings", v == InspectorView::Findings),
        ('2', "By Resource", v == InspectorView::Resources),
        ('3', "Coverage", v == InspectorView::Coverage),
    ];
    let mut spans = subtab_bar_spans(app, area, &tabs);

    // Severity scope toggle (cycled with `t`) — same pattern as WAF's scope.
    spans.push(Span::styled("     ", Style::default()));
    let toggle_col = Line::from(spans.clone()).width() as u16;
    spans.push(Span::styled("t Severity ", Style::default().fg(theme::text_dim())));

    let scopes = [
        InspSeverityScope::Critical,
        InspSeverityScope::High,
        InspSeverityScope::Medium,
        InspSeverityScope::All,
    ];
    for (i, scope) in scopes.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" ", Style::default()));
        }
        let label = scope.label();
        let active = app.insp_severity_scope == *scope;
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

    // Clicking anywhere on the scope toggle cycles it (`t`).
    let toggle_w = (Line::from(spans.clone()).width() as u16).saturating_sub(toggle_col);
    app.push_click_region(
        Rect {
            x: area.x + toggle_col,
            y: area.y,
            width: toggle_w,
            height: 1,
        },
        crate::app::ClickAction::Key('t'),
    );

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}
