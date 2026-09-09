use crate::ui::theme;
use crate::ui::widgets::region_selector::render_search_line;
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

/// Type-to-filter picker over every AWS service that has quotas (populated from
/// `ListServices`). `c` opens it on the Service Quotas service.
pub struct QuotaServiceSelectorState {
    pub visible: bool,
    pub selected_index: usize,
    pub query: String,
    /// (service_code, service_name), name-sorted.
    services: Vec<(String, String)>,
    loading: bool,
    /// List-area height recorded at render time (`Cell`: render only sees
    /// `&self`), so `Ctrl-d`/`Ctrl-u` page jumps scale with the actual popup.
    viewport: std::cell::Cell<u16>,
}

impl QuotaServiceSelectorState {
    pub fn new() -> Self {
        Self {
            visible: false,
            selected_index: 0,
            query: String::new(),
            services: Vec::new(),
            loading: false,
            viewport: std::cell::Cell::new(0),
        }
    }

    /// Open the modal. `loading` marks that the service list is still being
    /// fetched (the list fills in via `set_services` when it lands).
    pub fn show(&mut self, current_code: &str) {
        self.visible = true;
        self.query.clear();
        self.loading = self.services.is_empty();
        self.selected_index = self
            .services
            .iter()
            .position(|(code, _)| code == current_code)
            .unwrap_or(0);
    }

    pub fn hide(&mut self) {
        self.visible = false;
    }

    /// Populate the service list (from the async `ListServices` fetch).
    pub fn set_services(&mut self, services: Vec<(String, String)>) {
        self.services = services;
        self.loading = false;
    }

    pub fn filtered(&self) -> Vec<(String, String)> {
        if self.query.is_empty() {
            return self.services.clone();
        }
        let q = self.query.to_lowercase();
        self.services
            .iter()
            .filter(|(code, name)| {
                code.to_lowercase().contains(&q) || name.to_lowercase().contains(&q)
            })
            .cloned()
            .collect()
    }

    pub fn next(&mut self) {
        let len = self.filtered().len();
        if len > 0 && self.selected_index + 1 < len {
            self.selected_index += 1;
        }
    }

    pub fn previous(&mut self) {
        if self.selected_index > 0 {
            self.selected_index -= 1;
        }
    }

    pub fn push_char(&mut self, c: char) {
        self.query.push(c);
        self.selected_index = 0;
    }

    pub fn pop_char(&mut self) {
        self.query.pop();
        self.selected_index = 0;
    }

    /// The selected service code.
    pub fn selected_code(&self) -> Option<String> {
        self.filtered().get(self.selected_index).map(|(c, _)| c.clone())
    }

    fn page_stride(&self) -> usize {
        match self.viewport.get() {
            0 => 10,
            v => (v as usize / 2).max(1),
        }
    }

    pub fn page_down(&mut self) {
        let len = self.filtered().len();
        if len > 0 {
            self.selected_index = (self.selected_index + self.page_stride()).min(len - 1);
        }
    }

    pub fn page_up(&mut self) {
        self.selected_index = self.selected_index.saturating_sub(self.page_stride());
    }

    pub fn select_first(&mut self) {
        self.selected_index = 0;
    }

    pub fn select_last(&mut self) {
        self.selected_index = self.filtered().len().saturating_sub(1);
    }

    /// 1-based selection position and total — the `12/240` corner badge.
    pub fn position(&self) -> (usize, usize) {
        let len = self.filtered().len();
        ((self.selected_index + 1).min(len), len)
    }
}

pub fn render_quota_service_selector(
    state: &QuotaServiceSelectorState,
    current_code: &str,
    frame: &mut Frame,
) {
    if !state.visible {
        return;
    }

    let area = centered_rect(60, 60, frame.size());
    frame.render_widget(Clear, area);

    if state.loading {
        let block = theme::popup_block("Pick service (quotas)").title(
            Title::from(theme::hint_line(&[("Esc", "close")]))
                .position(Position::Bottom)
                .alignment(Alignment::Center),
        );
        let msg = Paragraph::new(vec![
            Line::raw(""),
            Line::styled(
                "  Loading services…",
                Style::default().fg(crate::ui::theme::text_muted()),
            ),
        ])
        .block(block);
        frame.render_widget(msg, area);
        return;
    }

    let (pos, total) = state.position();
    let block = theme::popup_block("Pick service (quotas)")
        .title(
            Title::from(theme::hint_line(&[
                ("type", "filter"),
                ("↑/↓", "move"),
                ("^d/^u", "page"),
                ("⏎", "select"),
                ("Esc", "close"),
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
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(inner);

    render_search_line(&state.query, chunks[0], frame);
    state.viewport.set(chunks[1].height);

    let filtered = state.filtered();
    let items: Vec<ListItem> = filtered
        .iter()
        .map(|(code, name)| {
            let is_current = code == current_code;
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
                Span::styled(name.clone(), name_style),
                Span::styled(
                    format!("  ({})", code),
                    Style::default().fg(theme::text_dim()),
                ),
            ]))
        })
        .collect();

    let list = List::new(items)
        .highlight_style(theme::selection_style(true))
        .highlight_symbol("▌ ");

    let mut list_state = ListState::default();
    if !filtered.is_empty() {
        list_state.select(Some(state.selected_index.min(filtered.len() - 1)));
    }

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
