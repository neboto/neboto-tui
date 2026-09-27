use crate::app::{App, DmsView};
use crate::ui::widgets::subtab_bar::render_subtab_bar;
use ratatui::{layout::Rect, Frame};

pub fn render_dms_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let v = app.dms_view;
    let tabs = [
        ('1', "Tasks", v == DmsView::Tasks),
        ('2', "Replication Instances", v == DmsView::Instances),
        ('3', "Endpoints", v == DmsView::Endpoints),
        ('4', "Serverless", v == DmsView::Serverless),
    ];
    render_subtab_bar(app, area, frame, &tabs);
}
