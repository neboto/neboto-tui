use crate::app::{App, AthenaView};
use crate::ui::widgets::subtab_bar::render_subtab_bar;
use ratatui::{layout::Rect, Frame};

pub fn render_athena_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let v = app.athena_view;
    let tabs = [
        ('1', "Workgroups", v == AthenaView::Workgroups),
        ('2', "Data Catalogs", v == AthenaView::DataCatalogs),
        ('3', "Databases", v == AthenaView::Databases),
        ('4', "Recent Queries", v == AthenaView::Queries),
        ('5', "Saved Queries", v == AthenaView::SavedQueries),
    ];
    render_subtab_bar(app, area, frame, &tabs);
}
