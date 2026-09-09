use crate::app::{App, TgwView};
use crate::ui::widgets::subtab_bar::render_subtab_bar;
use ratatui::{layout::Rect, Frame};

pub fn render_tgw_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let v = app.tgw_view;
    let tabs = [
        ('1', "Transit Gateways", v == TgwView::TransitGateways),
        ('2', "Attachments", v == TgwView::Attachments),
        ('3', "Route Tables", v == TgwView::RouteTables),
    ];
    render_subtab_bar(app, area, frame, &tabs);
}
