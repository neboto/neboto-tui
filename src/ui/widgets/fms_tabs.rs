use crate::app::{App, FmsView};
use crate::ui::widgets::subtab_bar::render_subtab_bar;
use ratatui::{layout::Rect, Frame};

pub fn render_fms_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let v = app.fms_view;
    let tabs = [
        ('1', "Policies", v == FmsView::Policies),
        ('2', "Apps Lists", v == FmsView::AppsLists),
        ('3', "Protocols Lists", v == FmsView::ProtocolsLists),
        ('4', "Resource Sets", v == FmsView::ResourceSets),
    ];
    render_subtab_bar(app, area, frame, &tabs);
}
