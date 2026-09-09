use crate::app::{App, ControlTowerView};
use crate::ui::widgets::subtab_bar::render_subtab_bar;
use ratatui::{layout::Rect, Frame};

pub fn render_controltower_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let v = app.controltower_view;
    let tabs = [
        ('1', "Landing Zone", v == ControlTowerView::LandingZone),
        ('2', "Controls", v == ControlTowerView::Controls),
        ('3', "Baselines", v == ControlTowerView::Baselines),
        ('4', "Operations", v == ControlTowerView::Operations),
        ('5', "Catalog", v == ControlTowerView::Catalog),
        ('6', "Compliance", v == ControlTowerView::Compliance),
        ('7', "Accounts", v == ControlTowerView::Accounts),
    ];
    render_subtab_bar(app, area, frame, &tabs);
}
