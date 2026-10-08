use crate::app::{App, BeanstalkView};
use crate::ui::widgets::subtab_bar::render_subtab_bar;
use ratatui::{layout::Rect, Frame};

pub fn render_beanstalk_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let v = app.beanstalk_view;
    let tabs = [
        ('1', "Environments", v == BeanstalkView::Environments),
        ('2', "Applications", v == BeanstalkView::Applications),
        ('3', "Versions", v == BeanstalkView::Versions),
    ];
    render_subtab_bar(app, area, frame, &tabs);
}
