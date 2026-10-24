//! The `:` command picker: every key the help lists that can run here, by
//! name. Rows come from the command table (`src/commands.rs`) every frame,
//! filtered to the focused pane and the selection's gates, so the list is
//! always what pressing the key would actually do. `⏎` closes the picker
//! and replays the row's key. This state is only the cursor and filter.

use crate::app::App;
use crate::commands;
use crate::ui::theme;
use crate::ui::widgets::region_selector::render_search_line;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Clear, List, ListItem, ListState, Paragraph},
    Frame,
};

#[derive(Default)]
pub struct CommandPickerState {
    pub visible: bool,
    pub query: String,
    pub selected_index: usize,
    /// List-area height recorded at render time, for `Ctrl-d`/`Ctrl-u`.
    viewport: std::cell::Cell<u16>,
}

impl CommandPickerState {
    pub fn show(&mut self) {
        self.visible = true;
        self.query.clear();
        self.selected_index = 0;
    }

    pub fn hide(&mut self) {
        self.visible = false;
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

pub fn render_command_picker(app: &App, frame: &mut Frame) {
    let state = &app.command_picker;
    if !state.visible {
        return;
    }
    let area = centered_rect(64, 60, frame.area());
    frame.render_widget(Clear, area);

    let block = theme::popup_block("Commands").title_bottom(
        theme::hint_line(&[("type", "fuzzy filter"), ("↑/↓", "move"), ("⏎", "run"), ("Esc", "close")])
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

    let rows = commands::picker_rows(app, &state.query);
    if rows.is_empty() {
        let note = vec![
            Line::raw(""),
            Line::styled(
                format!("  No command matches \"{}\"", state.query.trim()),
                Style::default().fg(theme::text_muted()),
            ),
        ];
        frame.render_widget(Paragraph::new(note), chunks[1]);
        return;
    }

    let key_w = rows.iter().map(|c| c.key.chars().count()).max().unwrap_or(0);
    let width = chunks[1].width as usize;
    let items: Vec<ListItem> = rows
        .iter()
        .map(|c| {
            // Right-hand tag: "here" for a key that only some selections
            // have, else the help section it's listed under.
            let (tag, tag_fg) = if c.gate.is_some() {
                ("here", theme::accent())
            } else {
                (c.section.title(), theme::text_dim())
            };
            let pad = " ".repeat(key_w.saturating_sub(c.key.chars().count()) + 2);
            // 2 for the highlight symbol, 2 between label and tag.
            let budget = width.saturating_sub(2 + key_w + 2 + 2 + tag.chars().count());
            let label = truncate(c.label, budget);
            let gap = budget.saturating_sub(label.chars().count()) + 2;
            ListItem::new(Line::from(vec![
                Span::styled(c.key, Style::default().fg(theme::accent())),
                Span::raw(pad),
                Span::styled(label, Style::default().fg(theme::text_primary())),
                Span::raw(" ".repeat(gap)),
                Span::styled(tag, Style::default().fg(tag_fg)),
            ]))
        })
        .collect();
    let widget = List::new(items)
        .highlight_style(theme::selection_style(true))
        .highlight_symbol("▌ ");
    let mut list_state = ListState::default();
    list_state.select(Some(state.selected_index.min(rows.len() - 1)));
    frame.render_stateful_widget(widget, chunks[1], &mut list_state);
    app.popup_hits
        .borrow_mut()
        .record_list(area, chunks[1], list_state.offset(), rows.len());
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else if max == 0 {
        String::new()
    } else {
        let mut s: String = text.chars().take(max - 1).collect();
        s.push('…');
        s
    }
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
