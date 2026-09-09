use crate::app::{App, RdsView};
use crate::ui::widgets::subtab_bar::render_subtab_bar;
use ratatui::{layout::Rect, Frame};

pub fn render_rds_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let v = app.rds_view;
    let tabs = [
        ('1', "Instances", v == RdsView::Instances),
        ('2', "Clusters", v == RdsView::Clusters),
        ('3', "Snapshots", v == RdsView::Snapshots),
        ('4', "Param Groups", v == RdsView::ParamGroups),
        ('5', "Option Groups", v == RdsView::OptionGroups),
        ('6', "Subnet Groups", v == RdsView::SubnetGroups),
    ];
    render_subtab_bar(app, area, frame, &tabs);
}
