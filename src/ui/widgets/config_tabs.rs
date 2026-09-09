use crate::app::{App, ConfigView};
use crate::ui::widgets::subtab_bar::render_subtab_bar;
use ratatui::{layout::Rect, Frame};

pub fn render_config_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let v = app.config_view;
    let tabs = [
        ('1', "Rules", v == ConfigView::Rules),
        ('2', "Conformance Packs", v == ConfigView::ConformancePacks),
    ];
    render_subtab_bar(app, area, frame, &tabs);
}
