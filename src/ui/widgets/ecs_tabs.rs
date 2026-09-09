use crate::app::{App, EcsView};
use crate::ui::theme;
use crate::ui::widgets::subtab_bar::subtab_bar_spans;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

pub fn render_ecs_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let v = app.ecs_view;
    let tabs = [
        ('1', "Clusters", v == EcsView::Clusters),
        ('2', "Services", v == EcsView::Services),
        ('3', "Task Definitions", v == EcsView::TaskDefinitions),
        ('4', "Tasks", v == EcsView::Tasks),
    ];
    let mut spans = subtab_bar_spans(app, area, &tabs);

    // On the Tasks view, surface the status-filter chip (cycled with `f`).
    if app.ecs_view == EcsView::Tasks {
        let f = app.ecs_task_status_filter;
        let active = f != crate::app::EcsTaskStatusFilter::All;
        spans.push(Span::styled("    f ", Style::default().fg(theme::accent())));
        spans.push(Span::styled(
            format!("status: {} ", f.label()),
            if active {
                Style::default()
                    .fg(Color::Black)
                    .bg(theme::aws_orange())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(crate::ui::theme::text_muted())
            },
        ));
    }

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}
