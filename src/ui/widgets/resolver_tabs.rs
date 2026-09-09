use crate::app::{App, ResolverView};
use crate::ui::widgets::subtab_bar::render_subtab_bar;
use ratatui::{layout::Rect, Frame};

pub fn render_resolver_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let v = app.resolver_view;
    let tabs = [
        ('1', "Endpoints", v == ResolverView::Endpoints),
        ('2', "Rules", v == ResolverView::Rules),
    ];
    render_subtab_bar(app, area, frame, &tabs);
}
