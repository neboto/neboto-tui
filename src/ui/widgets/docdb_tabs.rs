use crate::app::App;
use crate::ui::widgets::subtab_bar::render_subtab_bar;
use ratatui::{layout::Rect, Frame};

const LABELS: [&str; 5] = ["Clusters", "Instances", "Snapshots", "Param Groups", "Subnet Groups"];

/// DocumentDB and Neptune share one strip: both are cluster-first views of
/// the RDS API. `active` is the view's position (`DocDbView::index`).
pub fn render_cluster_db_tabs(app: &App, active: usize, area: Rect, frame: &mut Frame) {
    let keys = ['1', '2', '3', '4', '5'];
    let tabs: Vec<(char, &str, bool)> =
        LABELS.iter().enumerate().map(|(i, label)| (keys[i], *label, i == active)).collect();
    render_subtab_bar(app, area, frame, &tabs);
}
