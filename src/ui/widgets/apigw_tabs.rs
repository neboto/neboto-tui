use crate::app::{App, ApiGatewayView};
use crate::ui::widgets::subtab_bar::render_subtab_bar;
use ratatui::{layout::Rect, Frame};

pub fn render_apigw_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let v = app.apigw_view;
    let tabs = [
        ('1', "REST APIs", v == ApiGatewayView::RestApis),
        ('2', "HTTP/WS APIs", v == ApiGatewayView::HttpApis),
        ('3', "Custom Domains", v == ApiGatewayView::CustomDomains),
        ('4', "Usage Plans", v == ApiGatewayView::UsagePlans),
        ('5', "VPC Links", v == ApiGatewayView::VpcLinks),
    ];
    render_subtab_bar(app, area, frame, &tabs);
}
