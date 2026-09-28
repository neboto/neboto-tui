use crate::app::{App, BatchJobStatusFilter, BatchView};
use crate::ui::theme;
use crate::ui::widgets::subtab_bar::subtab_bar_spans;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

pub fn render_batch_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let v = app.batch_view;
    let tabs = [
        ('1', "Job Queues", v == BatchView::Queues),
        ('2', "Compute Environments", v == BatchView::ComputeEnvironments),
        ('3', "Jobs", v == BatchView::Jobs),
        ('4', "Job Definitions", v == BatchView::JobDefinitions),
    ];
    let mut spans = subtab_bar_spans(app, area, &tabs);

    // On the Jobs view, surface the status-group chip (cycled with `f`).
    if v == BatchView::Jobs {
        let f = app.batch_job_filter;
        spans.push(Span::styled("    f ", Style::default().fg(theme::accent())));
        spans.push(Span::styled(
            format!("status: {} ", f.label()),
            if f != BatchJobStatusFilter::All {
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
