use crate::app::{App, RedshiftView};
use crate::ui::widgets::subtab_bar::render_subtab_bar;
use ratatui::{layout::Rect, Frame};

pub fn render_redshift_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let v = app.redshift_view;
    let tabs = [
        ('1', "Clusters", v == RedshiftView::Clusters),
        ('2', "Serverless", v == RedshiftView::Workgroups),
        ('3', "Snapshots", v == RedshiftView::Snapshots),
    ];
    render_subtab_bar(app, area, frame, &tabs);
}
