use crate::app::LayoutMode;
use ratatui::layout::{Constraint, Direction, Layout, Rect};

pub struct AppLayout {
    pub banner_area: Option<Rect>,
    pub tabs_area: Rect,
    pub search_area: Rect,
    pub sub_tabs_area: Option<Rect>,
    pub resource_list_area: Rect,
    pub details_area: Rect,
    pub status_area: Rect,
}

impl AppLayout {
    pub fn new(area: Rect, show_banner: bool, show_sub_tabs: bool, layout_mode: LayoutMode) -> Self {
        // Build constraints dynamically based on flags
        let mut constraints: Vec<Constraint> = Vec::new();
        if show_banner {
            constraints.push(Constraint::Length(8)); // Banner
        }
        constraints.push(Constraint::Length(1)); // Service tabs
        constraints.push(Constraint::Length(3)); // Search bar
        if show_sub_tabs {
            constraints.push(Constraint::Length(1)); // Sub-tabs (IDC)
        }
        constraints.push(Constraint::Min(0));    // Main content
        constraints.push(Constraint::Length(1)); // Status bar

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints(constraints)
            .split(area);

        // Map indices based on which optional rows are present
        let mut idx = 0usize;
        let banner_area = if show_banner {
            let a = Some(chunks[idx]);
            idx += 1;
            a
        } else {
            None
        };
        let tabs_area = chunks[idx]; idx += 1;
        let search_area = chunks[idx]; idx += 1;
        let sub_tabs_area = if show_sub_tabs {
            let a = Some(chunks[idx]);
            idx += 1;
            a
        } else {
            None
        };
        let main_area = chunks[idx]; idx += 1;
        let status_area = chunks[idx];

        // Horizontal split with layout-mode-aware constraints
        let (list_pct, details_pct) = match layout_mode {
            LayoutMode::Split => (40, 60),
            LayoutMode::ListOnly => (100, 0),
            LayoutMode::DetailsOnly => (0, 100),
        };

        let horizontal_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(list_pct),
                Constraint::Percentage(details_pct),
            ])
            .split(main_area);

        Self {
            banner_area,
            tabs_area,
            search_area,
            sub_tabs_area,
            resource_list_area: horizontal_chunks[0],
            details_area: horizontal_chunks[1],
            status_area,
        }
    }
}
