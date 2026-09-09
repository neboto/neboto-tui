use crate::app::{App, IcView};
use crate::ui::widgets::subtab_bar::render_subtab_bar;
use ratatui::{layout::Rect, Frame};

pub fn render_idc_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let v = app.ic_view;
    let tabs = [
        ('1', "Instance", v == IcView::Instance),
        ('2', "Permission Sets", v == IcView::PermissionSets),
        ('3', "Users", v == IcView::Users),
        ('4', "Groups", v == IcView::Groups),
        ('5', "Applications", v == IcView::Applications),
    ];
    render_subtab_bar(app, area, frame, &tabs);
}
