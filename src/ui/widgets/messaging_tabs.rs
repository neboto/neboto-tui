use crate::app::{App, MsgView};
use crate::ui::widgets::subtab_bar::render_subtab_bar;
use ratatui::{layout::Rect, Frame};

pub fn render_messaging_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let v = app.msg_view;
    let tabs = [
        ('1', "Queues", v == MsgView::Queues),
        ('2', "Topics", v == MsgView::Topics),
    ];
    render_subtab_bar(app, area, frame, &tabs);
}
