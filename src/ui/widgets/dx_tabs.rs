use crate::app::{App, DxView};
use crate::ui::widgets::subtab_bar::render_subtab_bar;
use ratatui::{layout::Rect, Frame};

pub fn render_dx_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let v = app.dx_view;
    let tabs = [
        ('1', "Connections", v == DxView::Connections),
        ('2', "Virtual Interfaces", v == DxView::VirtualInterfaces),
        ('3', "LAGs", v == DxView::Lags),
        ('4', "Gateways", v == DxView::Gateways),
    ];
    render_subtab_bar(app, area, frame, &tabs);
}
