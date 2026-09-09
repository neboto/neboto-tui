use crate::app::{App, CodeView, ExecStatusFilter};
use crate::ui::theme;
use crate::ui::widgets::subtab_bar::subtab_bar_spans;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

pub fn render_code_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let v = app.code_view;
    let tabs = [
        ('1', "Repositories", v == CodeView::Repositories),
        ('2', "Build Projects", v == CodeView::BuildProjects),
        ('3', "Pipelines", v == CodeView::Pipelines),
        ('4', "Deployments", v == CodeView::Deployments),
        ('5', "Artifacts", v == CodeView::Artifacts),
        ('6', "Executions", v == CodeView::Executions),
        ('7', "Pull Requests", v == CodeView::PullRequests),
    ];
    let mut spans = subtab_bar_spans(app, area, &tabs);

    // On the Executions view, surface the status-filter chip (cycled with `f`).
    if v == CodeView::Executions {
        let f = app.code_exec_status_filter;
        let active = f != ExecStatusFilter::All;
        spans.push(Span::styled("    f ", Style::default().fg(theme::accent())));
        spans.push(Span::styled(
            format!("status: {} ", f.label()),
            if active {
                Style::default()
                    .fg(Color::Black)
                    .bg(theme::aws_orange())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme::text_muted())
            },
        ));
    }

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}
