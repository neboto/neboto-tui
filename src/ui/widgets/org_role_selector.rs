//! Assume-role modal for the member-account switch. Two entry points:
//!
//! - **Picker** — `s` on an Organizations account row with more than one
//!   configured role (`org_access_roles`): pick which role to assume in that
//!   account. With a single role the switch fires directly and this modal
//!   never appears.
//! - **Manual** — the `P` selector's "assume role by account id" entry: type
//!   a 12-digit account id and pick a role. Needs no `organizations:
//!   ListAccounts`, so it works from accounts that can't see the org (e.g. a
//!   CI account hopping into deploy targets).

use crate::ui::theme;
use ratatui::{
    layout::{Alignment, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{
        block::{Position, Title},
        Clear, List, ListItem, ListState, Paragraph,
    },
    Frame,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrgRoleModalMode {
    /// Role choice for a known account (picked from the Organizations list).
    Picker,
    /// Free-form: type the target account id, then pick a role.
    Manual,
}

pub struct OrgRoleSelectorState {
    pub visible: bool,
    pub mode: OrgRoleModalMode,
    pub selected_index: usize,
    roles: Vec<String>,
    /// Picker mode: the account the picked role will be assumed in —
    /// `(id, name)`. Stashed on open so the Enter handler can fire the switch
    /// event without re-resolving the (by then possibly changed) selection.
    pub pending_account: Option<(String, String)>,
    /// Manual mode: the typed target account id (digits only, max 12).
    pub account_input: String,
    /// Manual mode: shown under the input when Enter is pressed too early.
    pub input_error: Option<String>,
}

impl OrgRoleSelectorState {
    pub fn new() -> Self {
        Self {
            visible: false,
            mode: OrgRoleModalMode::Picker,
            selected_index: 0,
            roles: Vec::new(),
            pending_account: None,
            account_input: String::new(),
            input_error: None,
        }
    }

    /// Picker mode for `account`, preselecting `last_used` (the role from the
    /// previous switch) so repeat hops are ⏎-⏎.
    pub fn show(
        &mut self,
        roles: &[String],
        last_used: &str,
        account_id: String,
        account_name: String,
    ) {
        self.prime(roles, last_used);
        self.mode = OrgRoleModalMode::Picker;
        self.pending_account = Some((account_id, account_name));
        self.visible = true;
    }

    /// Manual mode: empty account-id input + the same role list.
    pub fn show_manual(&mut self, roles: &[String], last_used: &str) {
        self.prime(roles, last_used);
        self.mode = OrgRoleModalMode::Manual;
        self.pending_account = None;
        self.visible = true;
    }

    fn prime(&mut self, roles: &[String], last_used: &str) {
        self.roles = roles.to_vec();
        self.selected_index = self
            .roles
            .iter()
            .position(|r| r == last_used)
            .unwrap_or(0);
        self.account_input.clear();
        self.input_error = None;
    }

    pub fn hide(&mut self) {
        self.visible = false;
        self.pending_account = None;
        self.account_input.clear();
        self.input_error = None;
    }

    pub fn next(&mut self) {
        if !self.roles.is_empty() && self.selected_index + 1 < self.roles.len() {
            self.selected_index += 1;
        }
    }

    pub fn previous(&mut self) {
        if self.selected_index > 0 {
            self.selected_index -= 1;
        }
    }

    pub fn push_digit(&mut self, c: char) {
        if self.mode == OrgRoleModalMode::Manual
            && c.is_ascii_digit()
            && self.account_input.len() < 12
        {
            self.account_input.push(c);
            self.input_error = None;
        }
    }

    pub fn pop_digit(&mut self) {
        self.account_input.pop();
        self.input_error = None;
    }

    pub fn selected_role(&self) -> Option<String> {
        self.roles.get(self.selected_index).cloned()
    }

    /// Manual mode: the typed account id, when complete (12 digits).
    pub fn manual_account_id(&self) -> Option<String> {
        (self.account_input.len() == 12).then(|| self.account_input.clone())
    }
}

pub fn render_org_role_selector(state: &OrgRoleSelectorState, frame: &mut Frame) {
    if !state.visible {
        return;
    }

    let manual = state.mode == OrgRoleModalMode::Manual;
    let title = if manual {
        "Assume role by account id".to_string()
    } else {
        match &state.pending_account {
            Some((id, name)) if !name.is_empty() => format!("Assume role in {} ({})", name, id),
            Some((id, _)) => format!("Assume role in {}", id),
            None => return,
        }
    };

    // Size to content: input/error rows (manual) + one row per role + borders.
    let extra_rows: u16 = if manual {
        2 + state.input_error.is_some() as u16
    } else {
        0
    };
    let width = (title.chars().count() as u16 + 6)
        .max(
            state
                .roles
                .iter()
                .map(|r| r.chars().count() as u16 + 8)
                .max()
                .unwrap_or(30),
        )
        .max(40)
        .min(frame.size().width.saturating_sub(4));
    let height =
        (state.roles.len() as u16 + extra_rows + 3).min(frame.size().height.saturating_sub(4));
    let area = centered_rect(width, height, frame.size());
    frame.render_widget(Clear, area);

    let hints: &[(&str, &str)] = if manual {
        &[
            ("0-9", "account id"),
            ("↑/↓", "role"),
            ("⏎", "assume (read-only)"),
            ("Esc", "cancel"),
        ]
    } else {
        &[
            ("↑/↓", "navigate"),
            ("⏎", "assume (read-only)"),
            ("Esc", "cancel"),
        ]
    };
    let block = theme::popup_block(&title).title(
        Title::from(theme::hint_line(hints))
            .position(Position::Bottom)
            .alignment(Alignment::Center),
    );
    let mut inner = block.inner(area);
    frame.render_widget(block, area);

    if manual {
        // Input line: "Account ID: 123456789012▌" with a pad showing the
        // remaining digits.
        let typed = state.account_input.as_str();
        let placeholder = "_".repeat(12 - typed.len().min(12));
        let input_line = Line::from(vec![
            Span::styled("  Account ID: ", Style::default().fg(theme::accent())),
            Span::styled(
                typed.to_string(),
                Style::default().fg(crate::ui::theme::text_primary()).add_modifier(Modifier::BOLD),
            ),
            Span::styled(placeholder, Style::default().fg(theme::text_dim())),
        ]);
        let mut lines = vec![input_line];
        if let Some(err) = &state.input_error {
            lines.push(Line::styled(
                format!("  ⚠ {}", err),
                Style::default().fg(theme::warning()),
            ));
        }
        lines.push(Line::raw(""));
        let rows = lines.len() as u16;
        frame.render_widget(
            Paragraph::new(lines),
            Rect { height: rows.min(inner.height), ..inner },
        );
        inner.y += rows.min(inner.height);
        inner.height = inner.height.saturating_sub(rows);
    }

    let items: Vec<ListItem> = state
        .roles
        .iter()
        .map(|role| {
            ListItem::new(Line::from(vec![
                Span::raw("  "),
                Span::styled(role.clone(), Style::default().fg(crate::ui::theme::text_primary())),
            ]))
        })
        .collect();

    let list = List::new(items)
        .highlight_style(theme::selection_style(true))
        .highlight_symbol("▌ ");

    let mut list_state = ListState::default();
    if !state.roles.is_empty() {
        list_state.select(Some(state.selected_index.min(state.roles.len() - 1)));
    }

    frame.render_stateful_widget(list, inner, &mut list_state);
}

fn centered_rect(width: u16, height: u16, r: Rect) -> Rect {
    let x = r.x + r.width.saturating_sub(width) / 2;
    let y = r.y + r.height.saturating_sub(height) / 2;
    Rect {
        x,
        y,
        width: width.min(r.width),
        height: height.min(r.height),
    }
}
