//! The Cost `9` / `0` picker: choose the cost-allocation tag key or cost
//! category the spend list groups by. The key list itself lives on the
//! `LazyStore` (`cost_group_keys`, fetched when the picker opens — each page
//! is a billed Cost Explorer request); this state is only the modal's cursor
//! and filter, so the renderer reads the list from `App` every frame and the
//! picker fills in live when the fetch lands.

use crate::aws::services::cost::CostGroupBy;
use crate::lazy::Lazy;
use crate::ui::theme;
use crate::ui::widgets::region_selector::render_search_line;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, List, ListItem, ListState, Paragraph},
    Frame,
};

pub struct CostKeyPickerState {
    pub visible: bool,
    /// `CostGroupBy::Tag` or `CostGroupBy::CostCategory`.
    pub kind: CostGroupBy,
    pub query: String,
    pub selected_index: usize,
    /// List-area height recorded at render time, for `Ctrl-d`/`Ctrl-u`.
    viewport: std::cell::Cell<u16>,
}

impl Default for CostKeyPickerState {
    fn default() -> Self {
        Self::new()
    }
}

impl CostKeyPickerState {
    pub fn new() -> Self {
        Self {
            visible: false,
            kind: CostGroupBy::Tag,
            query: String::new(),
            selected_index: 0,
            viewport: std::cell::Cell::new(0),
        }
    }

    /// Open for `kind`, with the cursor on `current` once the list is known.
    pub fn show(&mut self, kind: CostGroupBy, keys: &[String], current: Option<&str>) {
        self.visible = true;
        self.kind = kind;
        self.query.clear();
        self.selected_index = current
            .and_then(|c| keys.iter().position(|k| k == c))
            .unwrap_or(0);
    }

    pub fn hide(&mut self) {
        self.visible = false;
    }

    /// The keys matching the filter (case-insensitive substring).
    pub fn filtered(&self, keys: &[String]) -> Vec<String> {
        let q = self.query.to_lowercase();
        keys.iter()
            .filter(|k| q.is_empty() || k.to_lowercase().contains(&q))
            .cloned()
            .collect()
    }

    /// What `⏎` picks: the highlighted match, or — when nothing matches —
    /// the typed text itself, so a key the list doesn't carry (no spend in
    /// the window yet, or `GetTags` denied) can still be grouped by.
    pub fn choice(&self, keys: &[String]) -> Option<String> {
        let filtered = self.filtered(keys);
        if let Some(k) = filtered.get(self.selected_index.min(filtered.len().saturating_sub(1))) {
            return Some(k.clone());
        }
        let typed = self.query.trim();
        (!typed.is_empty()).then(|| typed.to_string())
    }

    pub fn next(&mut self, len: usize) {
        if len > 0 && self.selected_index + 1 < len {
            self.selected_index += 1;
        }
    }

    pub fn previous(&mut self) {
        self.selected_index = self.selected_index.saturating_sub(1);
    }

    fn page_stride(&self) -> usize {
        match self.viewport.get() {
            0 => 10,
            v => (v as usize / 2).max(1),
        }
    }

    pub fn page_down(&mut self, len: usize) {
        if len > 0 {
            self.selected_index = (self.selected_index + self.page_stride()).min(len - 1);
        }
    }

    pub fn page_up(&mut self) {
        self.selected_index = self.selected_index.saturating_sub(self.page_stride());
    }

    pub fn push_char(&mut self, c: char) {
        self.query.push(c);
        self.selected_index = 0;
    }

    pub fn pop_char(&mut self) {
        self.query.pop();
        self.selected_index = 0;
    }
}

fn title(kind: CostGroupBy) -> &'static str {
    if kind == CostGroupBy::CostCategory {
        "Group cost by category"
    } else {
        "Group cost by tag"
    }
}

/// Why the list is empty — the usual reason, not an error.
fn empty_note(kind: CostGroupBy) -> [&'static str; 2] {
    if kind == CostGroupBy::CostCategory {
        [
            "No cost categories defined.",
            "Create one in Billing → Cost categories, or type a name and ⏎.",
        ]
    } else {
        [
            "No cost-allocation tags with spend in the last 3 months.",
            "Tags only appear once activated in Billing → Cost allocation tags (~24h). Type a key and ⏎.",
        ]
    }
}

pub fn render_cost_key_picker(
    state: &CostKeyPickerState,
    keys: Option<&Lazy<Vec<String>>>,
    current: Option<&str>,
    hits: &std::cell::RefCell<crate::app::PopupHits>,
    frame: &mut Frame,
) {
    if !state.visible {
        return;
    }
    let area = centered_rect(60, 60, frame.area());
    frame.render_widget(Clear, area);

    let block = theme::popup_block(title(state.kind)).title_bottom(
        theme::hint_line(&[
            ("type", "filter"),
            ("↑/↓", "move"),
            ("⏎", "group by"),
            ("Esc", "close"),
        ])
        .centered(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(inner);
    render_search_line(&state.query, chunks[0], frame);
    state.viewport.set(chunks[1].height);

    let list: &[String] = match keys {
        None | Some(Lazy::Loading) => {
            note(vec!["Loading…".to_string()], chunks[1], frame);
            return;
        }
        Some(Lazy::Error(e)) => {
            note(
                vec![format!("⚠ {}", e), "Type a key and ⏎ to group by it anyway.".to_string()],
                chunks[1],
                frame,
            );
            return;
        }
        Some(Lazy::Loaded(v)) => v,
    };
    let filtered = state.filtered(list);
    if filtered.is_empty() {
        let lines = if list.is_empty() {
            empty_note(state.kind).iter().map(|s| s.to_string()).collect()
        } else {
            vec![format!("No match — ⏎ groups by \"{}\" as typed.", state.query.trim())]
        };
        note(lines, chunks[1], frame);
        return;
    }

    let items: Vec<ListItem> = filtered
        .iter()
        .map(|k| {
            let is_current = Some(k.as_str()) == current;
            let marker = if is_current {
                Span::styled("● ", Style::default().fg(theme::success()))
            } else {
                Span::raw("  ")
            };
            let style = if is_current {
                Style::default().fg(theme::success()).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme::text_primary())
            };
            ListItem::new(Line::from(vec![marker, Span::styled(k.clone(), style)]))
        })
        .collect();
    let widget = List::new(items)
        .highlight_style(theme::selection_style(true))
        .highlight_symbol("▌ ");
    let mut list_state = ListState::default();
    list_state.select(Some(state.selected_index.min(filtered.len() - 1)));
    frame.render_stateful_widget(widget, chunks[1], &mut list_state);
    hits.borrow_mut().record_list(area, chunks[1], list_state.offset(), filtered.len());
}

/// A dim message in place of the list (loading / error / empty).
fn note(lines: Vec<String>, area: Rect, frame: &mut Frame) {
    let mut out = vec![Line::raw("")];
    out.extend(
        lines
            .into_iter()
            .map(|l| Line::styled(format!("  {}", l), Style::default().fg(theme::text_muted()))),
    );
    frame.render_widget(Paragraph::new(out), area);
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let rows = Layout::default()
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
        .split(rows[1])[1]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys() -> Vec<String> {
        ["team", "env", "CostCenter"].iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn choice_is_the_highlighted_match_else_the_typed_text() {
        let mut s = CostKeyPickerState::new();
        s.show(CostGroupBy::Tag, &keys(), Some("env"));
        assert_eq!(s.choice(&keys()).as_deref(), Some("env"), "cursor starts on the current key");
        s.push_char('c');
        s.push_char('o');
        assert_eq!(s.choice(&keys()).as_deref(), Some("CostCenter"), "filter is case-insensitive");
        s.push_char('x');
        assert_eq!(s.choice(&keys()).as_deref(), Some("cox"), "no match → the typed key");
        let mut empty = CostKeyPickerState::new();
        empty.show(CostGroupBy::CostCategory, &[], None);
        assert_eq!(empty.choice(&[]), None, "nothing typed, nothing listed → no choice");
    }
}
