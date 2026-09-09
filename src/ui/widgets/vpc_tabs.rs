use crate::app::{App, VpcView};
use crate::ui::widgets::subtab_bar::render_subtab_bar;
use ratatui::{layout::Rect, Frame};

pub fn render_vpc_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let v = app.vpc_view;
    let tabs = [
        ('1', "VPCs", v == VpcView::Vpcs),
        ('2', "Subnets", v == VpcView::Subnets),
        ('3', "Routing", v == VpcView::Routing),
        ('4', "Gateways", v == VpcView::Gateways),
        ('5', "Peering", v == VpcView::Peering),
        ('6', "Sec Groups", v == VpcView::SecurityGroups),
        ('7', "NACLs", v == VpcView::NetworkAcls),
        ('8', "Endpoints", v == VpcView::Endpoints),
        ('9', "VPN", v == VpcView::VpnConnections),
        ('0', "DHCP", v == VpcView::DhcpOptions),
    ];
    render_subtab_bar(app, area, frame, &tabs);
}
