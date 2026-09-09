use crate::app::{App, ElbView};
use crate::ui::widgets::subtab_bar::render_subtab_bar;
use ratatui::{layout::Rect, Frame};

pub fn render_elb_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let v = app.elb_view;
    let tabs = [
        ('1', "Load Balancers", v == ElbView::LoadBalancers),
        ('2', "Target Groups", v == ElbView::TargetGroups),
    ];
    render_subtab_bar(app, area, frame, &tabs);
}
