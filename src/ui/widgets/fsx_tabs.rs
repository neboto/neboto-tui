use crate::app::{App, FsxView};
use crate::ui::widgets::subtab_bar::render_subtab_bar;
use ratatui::{layout::Rect, Frame};

pub fn render_fsx_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let v = app.fsx_view;
    let tabs = [
        ('1', "File Systems", v == FsxView::FileSystems),
        ('2', "Volumes", v == FsxView::Volumes),
    ];
    render_subtab_bar(app, area, frame, &tabs);
}
