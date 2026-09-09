use crate::app::{App, ExecStatusFilter, SfnView};
use crate::ui::theme;
use crate::ui::widgets::subtab_bar::subtab_bar_spans;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

pub fn render_sfn_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let v = app.sfn_view;
    let tabs = [
        ('1', "State Machines", v == SfnView::StateMachines),
        ('2', "Executions", v == SfnView::Executions),
    ];
    let mut spans = subtab_bar_spans(app, area, &tabs);

    // On the Executions view, surface the status-filter chip (cycled with `f`).
    if v == SfnView::Executions {
        let f = app.sfn_exec_status_filter;
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
