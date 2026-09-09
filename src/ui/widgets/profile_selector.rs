use crate::aws::client::list_profiles;
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

/// Synthetic first entry shown while a member-account role is assumed —
/// selecting it drops the role and returns to the base profile credentials
/// (`Event::OrgRoleExitRequested`) instead of switching profile.
pub const EXIT_ASSUMED_ROLE_ENTRY: &str = "↩ exit assumed role (back to base account)";

/// Synthetic entry that opens the assume-by-account-id modal — cross-account
/// browsing without `organizations:ListAccounts` (CI accounts, invited
/// accounts, delegated setups).
pub const ASSUME_BY_ID_ENTRY: &str = "⇄ assume role by account id…";

pub struct ProfileSelectorState {
    pub visible: bool,
    pub selected_index: usize,
    pub query: String,
    profiles: Vec<String>,
    /// List-area height recorded at render time (`Cell`: render only sees
    /// `&self`), so `Ctrl-d`/`Ctrl-u` page jumps scale with the actual popup.
    viewport: std::cell::Cell<u16>,
}

impl ProfileSelectorState {
    pub fn new() -> Self {
        Self {
            visible: false,
            selected_index: 0,
            query: String::new(),
            profiles: Vec::new(),
            viewport: std::cell::Cell::new(0),
        }
    }

    /// Re-read the profile list from disk and show the modal, selecting the
    /// active profile (or `default`) if present. The assume-by-account-id
    /// action always leads the list; while a member-account role is assumed,
    /// the "exit assumed role" entry comes first.
    pub fn show(&mut self, current_profile: Option<&str>, role_assumed: bool) {
        self.profiles = list_profiles();
        self.profiles.insert(0, ASSUME_BY_ID_ENTRY.to_string());
        if role_assumed {
            self.profiles.insert(0, EXIT_ASSUMED_ROLE_ENTRY.to_string());
        }
        self.visible = true;
        self.query.clear();
        let target = current_profile.unwrap_or("default");
        self.selected_index = self
            .profiles
            .iter()
            .position(|p| p == target)
            .unwrap_or(0);
    }

    pub fn hide(&mut self) {
        self.visible = false;
    }

    /// Profiles matching the current search query.
    pub fn filtered(&self) -> Vec<String> {
        if self.query.is_empty() {
            return self.profiles.clone();
        }
        let q = self.query.to_lowercase();
        self.profiles
            .iter()
            .filter(|p| p.to_lowercase().contains(&q))
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

    pub fn selected_profile(&self) -> Option<String> {
        self.filtered().get(self.selected_index).cloned()
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

    /// 1-based selection position and total — the `3/45` corner badge.
    pub fn position(&self) -> (usize, usize) {
        let len = self.filtered().len();
        ((self.selected_index + 1).min(len), len)
    }
}

pub fn render_profile_selector(
    state: &ProfileSelectorState,
    current_profile: Option<&str>,
    frame: &mut Frame,
) {
    if !state.visible {
        return;
    }

    let area = centered_rect(60, 60, frame.size());
    frame.render_widget(Clear, area);

    // Empty state: no profiles found on disk.
    if state.profiles.is_empty() {
        let block = theme::popup_block("Switch AWS Profile").title(
            Title::from(theme::hint_line(&[("Esc", "close")]))
                .position(Position::Bottom)
                .alignment(Alignment::Center),
        );
        let msg = Paragraph::new(vec![
            Line::raw(""),
            Line::styled(
                "  No profiles found in ~/.aws/config or ~/.aws/credentials",
                Style::default().fg(crate::ui::theme::text_muted()),
            ),
        ])
        .block(block);
        frame.render_widget(msg, area);
        return;
    }

    let (pos, total) = state.position();
    let block = theme::popup_block("Switch AWS Profile")
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

    let active = current_profile.unwrap_or("default");
    let filtered = state.filtered();
    let items: Vec<ListItem> = filtered
        .iter()
        .map(|profile| {
            let is_current = profile == active;
            let is_exit = profile == EXIT_ASSUMED_ROLE_ENTRY || profile == ASSUME_BY_ID_ENTRY;
            let marker = if is_current {
                Span::styled("● ", Style::default().fg(theme::success()))
            } else {
                Span::raw("  ")
            };
            let name_style = if is_exit {
                Style::default()
                    .fg(theme::aws_orange())
                    .add_modifier(Modifier::BOLD)
            } else if is_current {
                Style::default()
                    .fg(theme::success())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(crate::ui::theme::text_primary())
            };
            ListItem::new(Line::from(vec![
                marker,
                Span::styled(profile.clone(), name_style),
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
