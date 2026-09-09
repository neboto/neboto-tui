use crate::app::{App, Ec2View};
use crate::ui::widgets::subtab_bar::render_subtab_bar;
use ratatui::{layout::Rect, Frame};

pub fn render_ec2_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let v = app.ec2_view;
    let tabs = [
        ('1', "Instances", v == Ec2View::Instances),
        ('2', "Security Groups", v == Ec2View::SecurityGroups),
        ('3', "Volumes", v == Ec2View::EbsVolumes),
        ('4', "Network Interfaces", v == Ec2View::NetworkInterfaces),
        ('5', "AMIs", v == Ec2View::Amis),
        ('6', "Snapshots", v == Ec2View::Snapshots),
        ('7', "Launch Templates", v == Ec2View::LaunchTemplates),
        ('8', "Elastic IPs", v == Ec2View::ElasticIps),
    ];
    render_subtab_bar(app, area, frame, &tabs);
}
