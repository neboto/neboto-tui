use crate::app::{ApiPortalView, App};
use crate::ui::widgets::subtab_bar::render_subtab_bar;
use ratatui::{layout::Rect, Frame};

pub fn render_apiportal_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let v = app.apiportal_view;
    let tabs = [
        ('1', "Inventory", v == ApiPortalView::Inventory),
        ('2', "Managed APIs", v == ApiPortalView::ManagedApis),
        ('3', "Portals", v == ApiPortalView::Portals),
        ('4', "Products", v == ApiPortalView::Products),
        ('5', "Discovery", v == ApiPortalView::Discovery),
    ];
    render_subtab_bar(app, area, frame, &tabs);
}
