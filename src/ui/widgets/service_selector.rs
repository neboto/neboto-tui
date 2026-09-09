use crate::aws::service::ServiceType;
use crate::ui::theme;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{
        block::{Position, Title},
        Clear, List, ListItem, ListState, Paragraph,
    },
    Frame,
};

/// One display row in the picker: a non-selectable category header or a
/// selectable service. Headers only exist while no query is typed — a search
/// flattens the list (standard fuzzy-picker behaviour), which keeps
/// filtering simple and the best match on top.
enum SelectorRow {
    Header(&'static str),
    Service(ServiceType),
}

pub struct ServiceSelectorState {
    pub visible: bool,
    pub search: String,
    pub selected_index: usize,
    rows: Vec<SelectorRow>,
    all: Vec<ServiceType>,
    /// List-area height recorded at render time (`Cell`: render only sees
    /// `&self`), so `Ctrl-d`/`Ctrl-u` page jumps scale with the actual popup.
    viewport: std::cell::Cell<u16>,
}

impl ServiceSelectorState {
    pub fn new() -> Self {
        let all = ServiceType::all();
        let mut state = Self {
            visible: false,
            search: String::new(),
            selected_index: 0,
            rows: Vec::new(),
            all,
            viewport: std::cell::Cell::new(0),
        };
        state.rebuild();
        state
    }

    pub fn show(&mut self, current: Option<ServiceType>) {
        self.visible = true;
        self.search.clear();
        self.rebuild();
        // Pre-select the current service's row
        self.selected_index = current
            .and_then(|s| {
                self.rows
                    .iter()
                    .position(|r| matches!(r, SelectorRow::Service(f) if *f == s))
            })
            .unwrap_or_else(|| self.first_service_row());
    }

    pub fn hide(&mut self) {
        self.visible = false;
        self.search.clear();
    }

    pub fn push_char(&mut self, c: char) {
        self.search.push(c);
        self.rebuild();
    }

    pub fn pop_char(&mut self) {
        self.search.pop();
        self.rebuild();
    }

    /// Recompute the rows for the current query: grouped by category when the
    /// query is empty, a flat filtered list otherwise.
    fn rebuild(&mut self) {
        self.rows.clear();
        if self.search.is_empty() {
            for category in ServiceType::CATEGORIES {
                let members: Vec<ServiceType> = self
                    .all
                    .iter()
                    .filter(|s| s.category() == *category)
                    .copied()
                    .collect();
                if members.is_empty() {
                    continue;
                }
                self.rows.push(SelectorRow::Header(category));
                self.rows
                    .extend(members.into_iter().map(SelectorRow::Service));
            }
        } else {
            let q = self.search.to_lowercase();
            self.rows = self
                .all
                .iter()
                .filter(|s| {
                    let name = s.name().to_lowercase();
                    let short = s.short_name().to_lowercase();
                    let desc = s.description().to_lowercase();
                    let cat = s.category().to_lowercase();
                    name.contains(&q)
                        || short.contains(&q)
                        || desc.contains(&q)
                        || cat.contains(&q)
                })
                .map(|s| SelectorRow::Service(*s))
                .collect();
        }
        self.selected_index = self.first_service_row();
    }

    /// Index of the first selectable (service) row — 0 when the list is flat,
    /// 1 when it opens with a category header.
    fn first_service_row(&self) -> usize {
        self.rows
            .iter()
            .position(|r| matches!(r, SelectorRow::Service(_)))
            .unwrap_or(0)
    }

    pub fn next(&mut self) {
        let mut i = self.selected_index;
        while i + 1 < self.rows.len() {
            i += 1;
            if matches!(self.rows[i], SelectorRow::Service(_)) {
                self.selected_index = i;
                return;
            }
        }
    }

    pub fn previous(&mut self) {
        let mut i = self.selected_index;
        while i > 0 {
            i -= 1;
            if matches!(self.rows[i], SelectorRow::Service(_)) {
                self.selected_index = i;
                return;
            }
        }
    }

    pub fn selected_service(&self) -> Option<ServiceType> {
        match self.rows.get(self.selected_index) {
            Some(SelectorRow::Service(s)) => Some(*s),
            _ => None,
        }
    }

    /// Half the rendered list height, with a sane fallback before the first
    /// frame has recorded it.
    fn page_stride(&self) -> usize {
        match self.viewport.get() {
            0 => 10,
            v => (v as usize / 2).max(1),
        }
    }

    pub fn page_down(&mut self) {
        self.jump(self.page_stride() as isize);
    }

    pub fn page_up(&mut self) {
        self.jump(-(self.page_stride() as isize));
    }

    pub fn select_first(&mut self) {
        self.selected_index = self.first_service_row();
    }

    pub fn select_last(&mut self) {
        if let Some(i) = self
            .rows
            .iter()
            .rposition(|r| matches!(r, SelectorRow::Service(_)))
        {
            self.selected_index = i;
        }
    }

    /// Move by `delta` rows, then snap to a service row in the direction of
    /// travel (falling back to the other direction at the list edges).
    fn jump(&mut self, delta: isize) {
        if self.rows.is_empty() {
            return;
        }
        let target = (self.selected_index as isize + delta)
            .clamp(0, self.rows.len() as isize - 1) as usize;
        self.selected_index = self.snap(target, delta >= 0);
    }

    /// Nearest service row at-or-after (`forward`) / at-or-before `from`,
    /// trying the other direction when the edge is all headers.
    fn snap(&self, from: usize, forward: bool) -> usize {
        let fwd = (from..self.rows.len())
            .find(|&i| matches!(self.rows[i], SelectorRow::Service(_)));
        let back = (0..=from.min(self.rows.len().saturating_sub(1)))
            .rev()
            .find(|&i| matches!(self.rows[i], SelectorRow::Service(_)));
        if forward { fwd.or(back) } else { back.or(fwd) }.unwrap_or(from)
    }

    /// Jump to the first service of the next category (grouped view only —
    /// a typed query flattens the list and removes the headers).
    pub fn next_category(&mut self) {
        if let Some(h) = (self.selected_index + 1..self.rows.len())
            .find(|&i| matches!(self.rows[i], SelectorRow::Header(_)))
        {
            self.selected_index = self.snap(h, true);
        }
    }

    /// Jump to the first service of the previous category (or the top of the
    /// current one when already in the first).
    pub fn prev_category(&mut self) {
        let own_header = (0..self.selected_index)
            .rev()
            .find(|&i| matches!(self.rows[i], SelectorRow::Header(_)));
        if let Some(own) = own_header {
            let prev = (0..own)
                .rev()
                .find(|&i| matches!(self.rows[i], SelectorRow::Header(_)));
            self.selected_index = self.snap(prev.unwrap_or(own), true);
        }
    }

    /// 1-based position of the selection among the service rows, and the
    /// total service-row count — the `12/60` corner badge.
    pub fn position(&self) -> (usize, usize) {
        if self.rows.is_empty() {
            return (0, 0);
        }
        let total = self
            .rows
            .iter()
            .filter(|r| matches!(r, SelectorRow::Service(_)))
            .count();
        let pos = self.rows[..=self.selected_index.min(self.rows.len() - 1)]
            .iter()
            .filter(|r| matches!(r, SelectorRow::Service(_)))
            .count();
        (pos, total)
    }
}

pub fn render_service_selector(
    state: &ServiceSelectorState,
    current: Option<ServiceType>,
    frame: &mut Frame,
) {
    if !state.visible {
        return;
    }

    let area = centered_rect(50, 75, frame.size());
    frame.render_widget(Clear, area);

    // Split into search bar + list
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // search box
            Constraint::Min(0),    // list
        ])
        .split(area);

    // Search bar
    let search_block = theme::pane_block("Switch Service", false);
    let search_inner = search_block.inner(chunks[0]);
    frame.render_widget(search_block, chunks[0]);

    let search_text = if state.search.is_empty() {
        Line::from(vec![
            Span::styled("Search… ", Style::default().fg(theme::text_dim())),
            Span::styled("█", Style::default().fg(theme::aws_orange())),
        ])
    } else {
        Line::from(vec![
            Span::styled(state.search.clone(), Style::default().fg(crate::ui::theme::text_primary())),
            Span::styled("█", Style::default().fg(theme::aws_orange())),
        ])
    };
    frame.render_widget(Paragraph::new(search_text), search_inner);

    // Service list — category headers render dim-bold; navigation skips them,
    // so the highlight only ever lands on a service row.
    let items: Vec<ListItem> = state
        .rows
        .iter()
        .map(|row| match row {
            SelectorRow::Header(category) => ListItem::new(Line::from(Span::styled(
                format!("  {}", category),
                Style::default()
                    .fg(theme::text_dim())
                    .add_modifier(Modifier::BOLD),
            ))),
            SelectorRow::Service(service) => {
                let is_current = current == Some(*service);
                let marker = if is_current {
                    Span::styled("● ", Style::default().fg(theme::success()))
                } else {
                    Span::raw("  ")
                };

                let name_style = if is_current {
                    Style::default()
                        .fg(theme::success())
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(crate::ui::theme::text_primary())
                };

                ListItem::new(Line::from(vec![
                    marker,
                    Span::styled(format!("{:<10}", service.short_name()), name_style),
                    Span::styled(
                        service.description().to_string(),
                        Style::default().fg(crate::ui::theme::text_dim()),
                    ),
                ]))
            }
        })
        .collect();

    let (pos, total) = state.position();
    let list_block = ratatui::widgets::Block::default()
        .borders(ratatui::widgets::Borders::LEFT | ratatui::widgets::Borders::RIGHT | ratatui::widgets::Borders::BOTTOM)
        .border_style(Style::default().fg(theme::border_dim()))
        .title(
            Title::from(theme::hint_line(&[
                ("type", "filter"),
                ("↑/↓", "move"),
                ("^d/^u", "page"),
                ("←/→", "category"),
                ("⏎", "select"),
                ("Esc", "cancel"),
            ]))
            .position(Position::Bottom)
            .alignment(Alignment::Center),
        )
        .title(
            Title::from(Span::styled(
                format!(" {}/{} ", pos, total),
                Style::default().fg(theme::text_dim()),
            ))
            .position(Position::Bottom)
            .alignment(Alignment::Right),
        );

    let list = List::new(items)
        .block(list_block)
        .highlight_style(theme::selection_style(true))
        .highlight_symbol("▌ ");

    // Record the visible list height so page jumps match the popup size.
    state.viewport.set(chunks[1].height.saturating_sub(1));

    let mut list_state = ListState::default();
    list_state.select(if state.rows.is_empty() {
        None
    } else {
        Some(state.selected_index)
    });

    frame.render_stateful_widget(list, chunks[1], &mut list_state);
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_category_is_in_the_ordered_list() {
        for service in ServiceType::all() {
            assert!(
                ServiceType::CATEGORIES.contains(&service.category()),
                "{:?} has category {:?} missing from ServiceType::CATEGORIES",
                service,
                service.category()
            );
        }
    }

    #[test]
    fn grouped_rows_cover_every_service_once() {
        let state = ServiceSelectorState::new();
        let services: Vec<ServiceType> = state
            .rows
            .iter()
            .filter_map(|r| match r {
                SelectorRow::Service(s) => Some(*s),
                _ => None,
            })
            .collect();
        assert_eq!(services.len(), ServiceType::all().len());
        // First row is a header, so the initial selection must skip past it.
        assert!(matches!(state.rows[0], SelectorRow::Header(_)));
        assert!(matches!(
            state.rows[state.first_service_row()],
            SelectorRow::Service(_)
        ));
    }

    #[test]
    fn navigation_skips_headers() {
        let mut state = ServiceSelectorState::new();
        state.show(None);
        // Walk the whole list — the selection must always be a service row.
        for _ in 0..state.rows.len() {
            assert!(state.selected_service().is_some());
            state.next();
        }
        for _ in 0..state.rows.len() {
            assert!(state.selected_service().is_some());
            state.previous();
        }
        assert_eq!(state.selected_index, state.first_service_row());
    }

    #[test]
    fn filtering_flattens_and_matches_category() {
        let mut state = ServiceSelectorState::new();
        state.show(None);
        for c in "contain".chars() {
            state.push_char(c);
        }
        assert!(!state.rows.is_empty());
        assert!(state
            .rows
            .iter()
            .all(|r| matches!(r, SelectorRow::Service(_))));
        // Category text is searchable: "contain" hits the Containers group.
        assert!(state.rows.iter().any(
            |r| matches!(r, SelectorRow::Service(s) if *s == ServiceType::ECS)
        ));
        // Clearing the query restores the grouped view.
        for _ in 0.."contain".len() {
            state.pop_char();
        }
        assert!(matches!(state.rows[0], SelectorRow::Header(_)));
    }

    #[test]
    fn show_preselects_the_current_service() {
        let mut state = ServiceSelectorState::new();
        state.show(Some(ServiceType::Kms));
        assert_eq!(state.selected_service(), Some(ServiceType::Kms));
    }

    #[test]
    fn page_jumps_land_on_service_rows_and_clamp() {
        let mut state = ServiceSelectorState::new();
        state.show(None);
        // Page all the way down: selection must stay on service rows and
        // eventually pin to the last one.
        for _ in 0..50 {
            state.page_down();
            assert!(state.selected_service().is_some());
        }
        let (pos, total) = state.position();
        assert_eq!(pos, total);
        // And back up to the first service row.
        for _ in 0..50 {
            state.page_up();
            assert!(state.selected_service().is_some());
        }
        assert_eq!(state.selected_index, state.first_service_row());
        assert_eq!(state.position().0, 1);
    }

    #[test]
    fn category_jumps_walk_the_group_tops() {
        let mut state = ServiceSelectorState::new();
        state.show(None);
        let first = state.selected_service();
        state.next_category();
        let second_cat = state.selected_service().unwrap();
        assert_ne!(Some(second_cat), first);
        // The landing row is the first service of its category: the row above
        // it must be that category's header.
        assert!(matches!(
            state.rows[state.selected_index - 1],
            SelectorRow::Header(_)
        ));
        state.prev_category();
        assert_eq!(state.selected_service(), first);
        // At the very top, another prev is a no-op (clamped to first group).
        state.prev_category();
        assert_eq!(state.selected_service(), first);
    }

    #[test]
    fn home_end_jump_to_first_and_last_service() {
        let mut state = ServiceSelectorState::new();
        state.show(None);
        state.select_last();
        let (pos, total) = state.position();
        assert_eq!(pos, total);
        assert!(state.selected_service().is_some());
        state.select_first();
        assert_eq!(state.position().0, 1);
        assert!(state.selected_service().is_some());
    }
}
