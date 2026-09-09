use crate::aws::services::cloudtrail::{CtEventQuery, CtEventRange, CtLookupAttr};
use crate::ui::theme;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        block::{Position, Title},
        Clear, List, ListItem, ListState, Paragraph,
    },
    Frame,
};

/// Which part of the filter modal owns the keyboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CtFilterPhase {
    /// Picking the lookup attribute (or "no filter").
    Attr,
    /// Typing the attribute value.
    Value,
}

/// CloudTrail server-side event filter (`f`): one lookup attribute + value
/// (the `LookupEvents` API accepts at most one) and a time-range preset.
/// Two-phase: pick the attribute, then type the value. `Tab` cycles the range
/// from either phase.
pub struct CtFilterModalState {
    pub visible: bool,
    pub phase: CtFilterPhase,
    /// 0 = "Recent events (no filter)", 1.. = `CtLookupAttr::ALL[i-1]`.
    pub attr_index: usize,
    pub value: String,
    pub range: CtEventRange,
}

impl CtFilterModalState {
    pub fn new() -> Self {
        Self {
            visible: false,
            phase: CtFilterPhase::Attr,
            attr_index: 0,
            value: String::new(),
            range: CtEventRange::default(),
        }
    }

    /// Open the modal seeded from the active query so editing a live filter
    /// starts from its current attribute/value/range.
    pub fn show(&mut self, current: &CtEventQuery) {
        self.visible = true;
        self.phase = CtFilterPhase::Attr;
        self.range = current.range;
        match &current.filter {
            Some((attr, value)) => {
                self.attr_index = CtLookupAttr::ALL
                    .iter()
                    .position(|a| a == attr)
                    .map(|i| i + 1)
                    .unwrap_or(0);
                self.value = value.clone();
            }
            None => {
                self.attr_index = 0;
                self.value.clear();
            }
        }
    }

    pub fn hide(&mut self) {
        self.visible = false;
    }

    /// Open pre-filled from a pivot (a row on an event's detail pane):
    /// attribute + value are set and the modal starts in the value phase, so
    /// a plain ⏎ applies while the value stays editable.
    pub fn show_seeded(&mut self, current: &CtEventQuery, attr: CtLookupAttr, value: String) {
        self.visible = true;
        self.phase = CtFilterPhase::Value;
        self.range = current.range;
        self.attr_index = CtLookupAttr::ALL
            .iter()
            .position(|a| *a == attr)
            .map(|i| i + 1)
            .unwrap_or(0);
        self.value = value;
    }

    /// The attribute the cursor is on; `None` for the "no filter" row.
    pub fn selected_attr(&self) -> Option<CtLookupAttr> {
        if self.attr_index == 0 {
            None
        } else {
            CtLookupAttr::ALL.get(self.attr_index - 1).copied()
        }
    }

    pub fn next(&mut self) {
        if self.attr_index < CtLookupAttr::ALL.len() {
            self.attr_index += 1;
        }
    }

    pub fn previous(&mut self) {
        if self.attr_index > 0 {
            self.attr_index -= 1;
        }
    }

    /// The query the modal currently describes; `None` while incomplete (an
    /// attribute is selected but the value is still empty).
    pub fn query(&self) -> Option<CtEventQuery> {
        match self.selected_attr() {
            None => Some(CtEventQuery {
                filter: None,
                range: self.range,
            }),
            Some(attr) => {
                let value = self.value.trim();
                if value.is_empty() {
                    None
                } else {
                    Some(CtEventQuery {
                        filter: Some((attr, value.to_string())),
                        range: self.range,
                    })
                }
            }
        }
    }
}

pub fn render_ct_filter_modal(state: &CtFilterModalState, frame: &mut Frame) {
    if !state.visible {
        return;
    }

    // Exact height: 8 attribute rows + value line + 2 range rows + borders.
    let area = centered_rect(56, (CtLookupAttr::ALL.len() + 1 + 1 + 2 + 2) as u16, frame.size());
    frame.render_widget(Clear, area);

    let hints: &[(&str, &str)] = match state.phase {
        CtFilterPhase::Attr => &[
            ("↑/↓", "attribute"),
            ("⇥", "range"),
            ("⏎", "next"),
            ("Esc", "close"),
        ],
        CtFilterPhase::Value => &[
            ("type", "value"),
            ("⇥", "range"),
            ("⏎", "apply"),
            ("Esc", "back"),
        ],
    };
    let block = theme::popup_block("Filter events (server-side)").title(
        Title::from(theme::hint_line(hints))
            .position(Position::Bottom)
            .alignment(Alignment::Center),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // attribute list | value line | range line
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length((CtLookupAttr::ALL.len() + 1) as u16),
            Constraint::Length(1),
            Constraint::Length(2),
        ])
        .split(inner);

    let attr_focused = state.phase == CtFilterPhase::Attr;
    let mut items: Vec<ListItem> = vec![ListItem::new(Line::from(vec![Span::styled(
        "Recent events (no filter)",
        Style::default().fg(crate::ui::theme::text_primary()),
    )]))];
    items.extend(CtLookupAttr::ALL.iter().map(|attr| {
        ListItem::new(Line::from(vec![
            Span::styled(attr.label().to_string(), Style::default().fg(crate::ui::theme::text_primary())),
            Span::styled(
                format!("  {}", attr.hint()),
                Style::default().fg(theme::text_dim()),
            ),
        ]))
    }));
    let list = List::new(items)
        .highlight_style(theme::selection_style(attr_focused))
        .highlight_symbol("▌ ");
    let mut list_state = ListState::default();
    list_state.select(Some(state.attr_index));
    frame.render_stateful_widget(list, chunks[0], &mut list_state);

    // Value input line — only meaningful when an attribute is selected.
    let value_line = match state.selected_attr() {
        None => Line::from(vec![Span::styled(
            "  (no value — shows the most recent events)",
            Style::default().fg(theme::text_dim()),
        )]),
        Some(_) => {
            let cursor = if state.phase == CtFilterPhase::Value {
                Span::styled("▌", Style::default().fg(theme::accent()))
            } else {
                Span::raw("")
            };
            let value_span = if state.value.is_empty() {
                Span::styled("value…", Style::default().fg(theme::text_dim()))
            } else {
                Span::styled(
                    state.value.clone(),
                    Style::default().fg(crate::ui::theme::text_primary()).add_modifier(Modifier::BOLD),
                )
            };
            Line::from(vec![
                Span::styled("  Value: ", Style::default().fg(theme::accent())),
                value_span,
                cursor,
            ])
        }
    };
    frame.render_widget(Paragraph::new(value_line), chunks[1]);

    // Range preset row — active preset highlighted; ⇥ cycles.
    let mut range_spans = vec![Span::styled(
        "  Range: ",
        Style::default().fg(theme::accent()),
    )];
    for (i, r) in CtEventRange::ALL.iter().enumerate() {
        if i > 0 {
            range_spans.push(Span::raw("  "));
        }
        if *r == state.range {
            range_spans.push(Span::styled(
                format!(" {} ", r.label()),
                Style::default()
                    .fg(Color::Black)
                    .bg(theme::aws_orange())
                    .add_modifier(Modifier::BOLD),
            ));
        } else {
            range_spans.push(Span::styled(
                r.label().to_string(),
                Style::default().fg(theme::text_dim()),
            ));
        }
    }
    let range_lines = vec![Line::raw(""), Line::from(range_spans)];
    frame.render_widget(Paragraph::new(range_lines), chunks[2]);
}

fn centered_rect(percent_x: u16, height: u16, r: Rect) -> Rect {
    let width = (r.width * percent_x / 100).min(r.width);
    let height = height.min(r.height);
    Rect {
        x: r.x + (r.width.saturating_sub(width)) / 2,
        y: r.y + (r.height.saturating_sub(height)) / 2,
        width,
        height,
    }
}
