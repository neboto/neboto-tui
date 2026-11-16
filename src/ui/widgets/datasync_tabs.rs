use crate::app::{App, DataSyncView};
use crate::ui::widgets::subtab_bar::render_subtab_bar;
use ratatui::{layout::Rect, Frame};

pub fn render_datasync_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let v = app.datasync_view;
    let tabs = [
        ('1', "Tasks", v == DataSyncView::Tasks),
        ('2', "Locations", v == DataSyncView::Locations),
        ('3', "Agents", v == DataSyncView::Agents),
    ];
    render_subtab_bar(app, area, frame, &tabs);
}
