use crate::app::{App, GdSeverityScope, GuardDutyView};
use crate::ui::theme;
use crate::ui::widgets::subtab_bar::subtab_bar_spans;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

pub fn render_gd_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let v = app.guardduty_view;
    let tabs = [
        ('1', "Findings", v == GuardDutyView::Findings),
        ('2', "Summary", v == GuardDutyView::Summary),
        ('3', "Coverage", v == GuardDutyView::Coverage),
        ('4', "Filters", v == GuardDutyView::Filters),
        ('5', "Lists", v == GuardDutyView::Lists),
        ('6', "Accounts", v == GuardDutyView::Accounts),
        ('7', "Malware", v == GuardDutyView::Malware),
        ('8', "Detectors", v == GuardDutyView::Detectors),
    ];
    let mut spans = subtab_bar_spans(app, area, &tabs);

    // Severity scope toggle (cycled with `t`) — same pattern as WAF's scope.
    spans.push(Span::styled("     ", Style::default()));
    let toggle_col = Line::from(spans.clone()).width() as u16;
    spans.push(Span::styled("t Severity ", Style::default().fg(theme::text_dim())));

    let scopes = [
        GdSeverityScope::Critical,
        GdSeverityScope::High,
        GdSeverityScope::Medium,
        GdSeverityScope::All,
    ];
    for (i, scope) in scopes.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" ", Style::default()));
        }
        let label = scope.label();
        let active = app.gd_severity_scope == *scope;
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
