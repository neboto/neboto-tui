use crate::app::{App, EcrView};
use crate::ui::widgets::subtab_bar::render_subtab_bar;
use ratatui::{layout::Rect, Frame};

pub fn render_ecr_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let v = app.ecr_view;
    let tabs = [
        ('1', "Repositories", v == EcrView::Repositories),
        ('2', "Images", v == EcrView::Images),
    ];
    render_subtab_bar(app, area, frame, &tabs);
}
