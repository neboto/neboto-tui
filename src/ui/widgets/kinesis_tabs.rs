use crate::app::{App, KinesisView};
use crate::ui::widgets::subtab_bar::render_subtab_bar;
use ratatui::{layout::Rect, Frame};

pub fn render_kinesis_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let v = app.kinesis_view;
    let tabs = [
        ('1', "Data Streams", v == KinesisView::Streams),
        ('2', "Firehose", v == KinesisView::Firehose),
    ];
    render_subtab_bar(app, area, frame, &tabs);
}
