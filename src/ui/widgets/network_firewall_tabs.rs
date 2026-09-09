use crate::app::{App, NfwView};
use crate::ui::widgets::subtab_bar::render_subtab_bar;
use ratatui::{layout::Rect, Frame};

pub fn render_network_firewall_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let v = app.nfw_view;
    let tabs = [
        ('1', "Firewalls", v == NfwView::Firewalls),
        ('2', "Firewall Policies", v == NfwView::Policies),
        ('3', "Rule Groups", v == NfwView::RuleGroups),
    ];
    render_subtab_bar(app, area, frame, &tabs);
}
