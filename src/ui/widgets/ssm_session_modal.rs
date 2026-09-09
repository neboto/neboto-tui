//! SSM Session Manager action modal.
//!
//! Opened with `s` on an EC2 instance or a Fleet (SSM-managed) instance. Offers
//! a plain shell session, a local→instance **port forward**, or a **remote-host
//! tunnel** (reach an RDS/other host *through* the instance). The port-forward
//! options collect their ports/host via a single-line input stage; the built
//! `aws ssm start-session` command is then launched by `App` (tmux window / new
//! OS window / inline), exactly like the plain shell session.

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

/// The action chosen from the menu (also the input kind once past the menu).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SsmSessionAction {
    Shell,
    PortForward,
    RemoteHost,
}

/// Which screen of the modal is showing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SsmModalStage {
    Menu,
    Input(SsmSessionAction),
}

pub struct SsmSessionModalState {
    pub visible: bool,
    pub instance_id: String,
    /// Display name (node name / instance id) for the header.
    pub label: String,
    pub stage: SsmModalStage,
    pub menu_index: usize,
    pub input: String,
    /// Set when a parse/validation error should be shown under the input.
    pub input_error: Option<String>,
}

pub const MENU: [(SsmSessionAction, &str, &str); 3] = [
    (SsmSessionAction::Shell, "Shell session", "interactive shell on the instance"),
    (
        SsmSessionAction::PortForward,
        "Port forward",
        "forward a local port to a port on the instance",
    ),
    (
        SsmSessionAction::RemoteHost,
        "Remote host tunnel",
        "tunnel to another host (RDS, …) through the instance",
    ),
];

impl SsmSessionModalState {
    pub fn new() -> Self {
        Self {
            visible: false,
            instance_id: String::new(),
            label: String::new(),
            stage: SsmModalStage::Menu,
            menu_index: 0,
            input: String::new(),
            input_error: None,
        }
    }

    pub fn show(&mut self, instance_id: String, label: String) {
        self.visible = true;
        self.instance_id = instance_id;
        self.label = label;
        self.stage = SsmModalStage::Menu;
        self.menu_index = 0;
        self.input.clear();
        self.input_error = None;
    }

    pub fn hide(&mut self) {
        self.visible = false;
    }

    pub fn next(&mut self) {
        if self.menu_index + 1 < MENU.len() {
            self.menu_index += 1;
        }
    }

    pub fn previous(&mut self) {
        if self.menu_index > 0 {
            self.menu_index -= 1;
        }
    }

    pub fn selected_action(&self) -> SsmSessionAction {
        MENU[self.menu_index].0
    }

    pub fn push_char(&mut self, c: char) {
        self.input.push(c);
        self.input_error = None;
    }

    pub fn pop_char(&mut self) {
        self.input.pop();
        self.input_error = None;
    }
}

fn input_prompt(action: SsmSessionAction) -> &'static str {
    match action {
        SsmSessionAction::PortForward => "remote-port [local-port]   e.g.  3306 13306",
        SsmSessionAction::RemoteHost => "host remote-port [local-port]   e.g.  db.internal 5432 15432",
        SsmSessionAction::Shell => "",
    }
}

pub fn render_ssm_session_modal(state: &SsmSessionModalState, frame: &mut Frame) {
    if !state.visible {
        return;
    }

    let area = centered_rect(64, 40, frame.size());
    frame.render_widget(Clear, area);

    let title = format!("SSM session · {}", state.label);

    match state.stage {
        SsmModalStage::Menu => {
            let block = theme::popup_block(&title).title(
                Title::from(theme::hint_line(&[
                    ("↑/↓", "navigate"),
                    ("⏎", "select"),
                    ("Esc", "close"),
                ]))
                .position(Position::Bottom)
                .alignment(Alignment::Center),
            );
            let inner = block.inner(area);
            frame.render_widget(block, area);

            let items: Vec<ListItem> = MENU
                .iter()
                .map(|(_, label, desc)| {
                    ListItem::new(Line::from(vec![
                        Span::styled(
                            format!("  {:<20}", label),
                            Style::default()
                                .fg(crate::ui::theme::text_primary())
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(desc.to_string(), Style::default().fg(theme::text_dim())),
                    ]))
                })
                .collect();

            let list = List::new(items)
                .highlight_style(theme::selection_style(true))
                .highlight_symbol("▌ ");
            let mut list_state = ListState::default();
            list_state.select(Some(state.menu_index));
            frame.render_stateful_widget(list, inner, &mut list_state);
        }
        SsmModalStage::Input(action) => {
            let block = theme::popup_block(&title).title(
                Title::from(theme::hint_line(&[
                    ("type", "params"),
                    ("⏎", "connect"),
                    ("Esc", "back"),
                ]))
                .position(Position::Bottom)
                .alignment(Alignment::Center),
            );
            let inner = block.inner(area);
            frame.render_widget(block, area);

            let mut lines = vec![
                Line::raw(""),
                Line::from(Span::styled(
                    format!("  {}", input_prompt(action)),
                    Style::default().fg(theme::text_dim()),
                )),
                Line::raw(""),
                Line::from(vec![
                    Span::raw("  ❯ "),
                    Span::styled(
                        state.input.clone(),
                        Style::default().fg(theme::aws_orange()),
                    ),
                    Span::styled("▏", Style::default().fg(theme::aws_orange())),
                ]),
            ];
            if let Some(err) = &state.input_error {
                lines.push(Line::raw(""));
                lines.push(Line::from(Span::styled(
                    format!("  {}", err),
                    Style::default().fg(theme::error()),
                )));
            }
            frame.render_widget(Paragraph::new(lines), inner);
        }
    }
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
