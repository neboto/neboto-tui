use crate::app::{App, MqView};
use crate::ui::widgets::subtab_bar::render_subtab_bar;
use ratatui::{layout::Rect, Frame};

pub fn render_mq_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let v = app.mq_view;
    let tabs = [
        ('1', "Brokers", v == MqView::Brokers),
        ('2', "Configurations", v == MqView::Configurations),
    ];
    render_subtab_bar(app, area, frame, &tabs);
}
