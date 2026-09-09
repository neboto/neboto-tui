use crate::app::{App, CwView};
use crate::ui::widgets::subtab_bar::render_subtab_bar;
use ratatui::{layout::Rect, Frame};

pub fn render_cw_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let v = app.cw_view;
    let tabs = [
        ('1', "Alarms", v == CwView::Alarms),
        ('2', "Log Groups", v == CwView::LogGroups),
        ('3', "Dashboards", v == CwView::Dashboards),
        ('4', "Metrics", v == CwView::Metrics),
        ('5', "Cross-Account", v == CwView::CrossAccount),
        ('6', "Streams", v == CwView::Streams),
        ('7', "Insights", v == CwView::Insights),
        ('8', "Account Policies", v == CwView::AccountPolicies),
    ];
    render_subtab_bar(app, area, frame, &tabs);
}
