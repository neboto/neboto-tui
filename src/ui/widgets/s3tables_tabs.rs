use crate::app::{App, S3TablesView};
use crate::ui::widgets::subtab_bar::render_subtab_bar;
use ratatui::{layout::Rect, Frame};

pub fn render_s3tables_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let v = app.s3tables_view;
    let tabs = [
        ('1', "Table Buckets", v == S3TablesView::Buckets),
        ('2', "Tables", v == S3TablesView::Tables),
    ];
    render_subtab_bar(app, area, frame, &tabs);
}
