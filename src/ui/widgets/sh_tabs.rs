use crate::app::{App, SecurityHubView, ShSeverityScope};
use crate::ui::theme;
use crate::ui::widgets::subtab_bar::subtab_bar_spans;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

pub fn render_sh_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let v = app.securityhub_view;
    let tabs = [
        ('1', "Overview", v == SecurityHubView::Overview),
        ('2', "Findings", v == SecurityHubView::Findings),
        ('3', "Controls", v == SecurityHubView::Controls),
        ('4', "Standards", v == SecurityHubView::Standards),
        ('5', "Insights", v == SecurityHubView::Insights),
        ('6', "Automations", v == SecurityHubView::Automations),
        ('7', "Integrations", v == SecurityHubView::Integrations),
        ('8', "Configuration", v == SecurityHubView::Configuration),
        ('9', "Accounts", v == SecurityHubView::Accounts),
    ];
    let mut spans = subtab_bar_spans(app, area, &tabs);

    // Severity scope toggle (cycled with `t`) — same pattern as WAF's scope.
    spans.push(Span::styled("     ", Style::default()));
    let toggle_col = Line::from(spans.clone()).width() as u16;
    spans.push(Span::styled("t Severity ", Style::default().fg(theme::text_dim())));

    let scopes = [
        ShSeverityScope::Critical,
        ShSeverityScope::High,
        ShSeverityScope::Medium,
        ShSeverityScope::All,
    ];
    for (i, scope) in scopes.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" ", Style::default()));
        }
        let label = scope.label();
        let active = app.sh_severity_scope == *scope;
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
