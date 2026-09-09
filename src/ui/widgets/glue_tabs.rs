use crate::app::{App, GlueView};
use crate::ui::widgets::subtab_bar::render_subtab_bar;
use ratatui::{layout::Rect, Frame};

pub fn render_glue_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let v = app.glue_view;
    let tabs = [
        ('1', "Databases", v == GlueView::Databases),
        ('2', "Tables", v == GlueView::Tables),
        ('3', "Crawlers", v == GlueView::Crawlers),
        ('4', "Jobs", v == GlueView::Jobs),
        ('5', "Job Runs", v == GlueView::JobRuns),
    ];
    render_subtab_bar(app, area, frame, &tabs);
}
