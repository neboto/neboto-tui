//! The AgentCore Memory session browser (`i`) — a three-level drill over the
//! data plane: **Actors → Sessions → Events**, plus a `t`-toggled **Records**
//! mode over long-term memory records.
//!
//! This exists because the event store *is* AgentCore Memory: the control
//! plane tells you a store's strategies and config, but only `ListEvents` says
//! what the agent actually remembered. It's the AgentCore analogue of the S3
//! object browser, and it's modelled on it deliberately — same in-pane
//! placement, same `Z` full-width toggle, same `/` filter and `n` paging.

use crate::aws::services::agentcore::MemoryBrowserRow;
use crate::ui::theme;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{
        block::{Position, Title},
        Paragraph,
    },
    Frame,
};

/// Which level the browser is showing. Actors/Sessions/Events are one drill
/// path; Records is a parallel view of the same store (long-term memory
/// rather than the raw event log), reached with `t`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryLevel {
    Actors,
    Sessions,
    Events,
    Records,
}

impl MemoryLevel {
    pub fn label(&self) -> &'static str {
        match self {
            MemoryLevel::Actors => "Actors",
            MemoryLevel::Sessions => "Sessions",
            MemoryLevel::Events => "Events",
            MemoryLevel::Records => "Records",
        }
    }
}

pub struct MemoryBrowserState {
    pub visible: bool,
    pub memory_id: String,
    pub level: MemoryLevel,
    /// The drill path so far. Empty at Actors; `actor` set at Sessions;
    /// both set at Events.
    pub actor_id: String,
    pub session_id: String,
    /// Records mode is scoped to one strategy at a time; `strategies` is the
    /// pick list (from the memory's lazy detail) and `strategy_idx` the cursor.
    pub strategies: Vec<(String, String)>,
    pub strategy_idx: usize,
    pub rows: Vec<MemoryBrowserRow>,
    pub selected: usize,
    pub filter: String,
    pub filtering: bool,
    pub next_token: Option<String>,
    pub loading: bool,
    pub error: Option<String>,
    /// Transient one-line notice (copied, nothing to open, …) — the real
    /// status bar is hidden under this overlay.
    pub message: Option<String>,
    /// Whether the payload preview panel is open (`i`).
    pub show_detail: bool,
    pub detail_scroll: usize,
    pub show_help: bool,
}

impl Default for MemoryBrowserState {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryBrowserState {
    pub fn new() -> Self {
        Self {
            visible: false,
            memory_id: String::new(),
            level: MemoryLevel::Actors,
            actor_id: String::new(),
            session_id: String::new(),
            strategies: Vec::new(),
            strategy_idx: 0,
            rows: Vec::new(),
            selected: 0,
            filter: String::new(),
            filtering: false,
            next_token: None,
            loading: false,
            error: None,
            message: None,
            show_detail: false,
            detail_scroll: 0,
            show_help: false,
        }
    }

    pub fn open(&mut self, memory_id: String, strategies: Vec<(String, String)>) {
        *self = Self::new();
        self.visible = true;
        self.memory_id = memory_id;
        self.strategies = strategies;
    }

    pub fn close(&mut self) {
        self.visible = false;
    }

    /// Reset everything that's scoped to one listing. Called on every level
    /// change so a stale filter/cursor/token can't leak across levels.
    pub fn reset_listing(&mut self) {
        self.rows.clear();
        self.selected = 0;
        self.next_token = None;
        self.filter.clear();
        self.filtering = false;
        self.error = None;
        self.show_detail = false;
        self.detail_scroll = 0;
    }

    pub fn current_strategy(&self) -> Option<&(String, String)> {
        self.strategies.get(self.strategy_idx)
    }

    /// Rows surviving the `/` filter, as indices into `rows`.
    pub fn filtered_indices(&self) -> Vec<usize> {
        if self.filter.is_empty() {
            return (0..self.rows.len()).collect();
        }
        let needle = self.filter.to_lowercase();
        (0..self.rows.len())
            .filter(|&i| {
                let r = &self.rows[i];
                r.primary.to_lowercase().contains(&needle)
                    || r.secondary.to_lowercase().contains(&needle)
                    || r.id.to_lowercase().contains(&needle)
            })
            .collect()
    }

    pub fn selected_row(&self) -> Option<&MemoryBrowserRow> {
        let idxs = self.filtered_indices();
        idxs.get(self.selected).map(|&i| &self.rows[i])
    }

    /// Breadcrumb: `memory-id / actor / session`, trimmed to the active depth.
    pub fn breadcrumb(&self) -> String {
        let mut s = self.memory_id.clone();
        if self.level == MemoryLevel::Records {
            if let Some((_, name)) = self.current_strategy() {
                s.push_str(&format!("  ▸  records / {}", name));
            } else {
                s.push_str("  ▸  records");
            }
            return s;
        }
        if !self.actor_id.is_empty() {
            s.push_str(&format!("  ▸  {}", self.actor_id));
        }
        if !self.session_id.is_empty() {
            s.push_str(&format!("  ▸  {}", self.session_id));
        }
        s
    }
}

pub fn render_memory_browser(app: &crate::app::App, area: Rect, frame: &mut Frame) {
    let st = &app.memory_browser;
    if !st.visible {
        return;
    }

    let block = theme::popup_block(&format!("Memory — {}", st.level.label())).title(
        Title::from(Span::styled(
            " ⏎/l open · h up · i payload · / filter · n next · t records · e edit · y copy · Z width · ? help · Esc close ",
            Style::default().fg(theme::text_dim()),
        ))
        .position(Position::Bottom)
        .alignment(Alignment::Center),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // breadcrumb + filter
            Constraint::Length(1), // rule
            Constraint::Min(0),    // listing
            Constraint::Length(1), // status
        ])
        .split(inner);

    render_breadcrumb(st, chunks[0], frame);
    render_hr(chunks[1], frame);

    if st.show_help {
        render_help(chunks[2], frame);
    } else if st.show_detail {
        // Payload preview under the listing — events are the whole point and
        // a 200-char row preview isn't enough to read a turn.
        let split = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Percentage(45),
                Constraint::Length(1),
                Constraint::Min(0),
            ])
            .split(chunks[2]);
        render_listing(st, split[0], frame);
        render_hr(split[1], frame);
        render_payload(st, split[2], frame);
    } else {
        render_listing(st, chunks[2], frame);
    }

    render_status(st, chunks[3], frame);
}

fn render_breadcrumb(st: &MemoryBrowserState, area: Rect, frame: &mut Frame) {
    if st.filtering || !st.filter.is_empty() {
        let cursor = if st.filtering { "█" } else { "" };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(" filter: ", Style::default().fg(theme::text_dim())),
                Span::styled(
                    format!("{}{}", st.filter, cursor),
                    Style::default().fg(theme::accent()),
                ),
            ])),
            area,
        );
        return;
    }
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw(" "),
            Span::styled(
                st.breadcrumb(),
                Style::default().fg(theme::accent()).add_modifier(Modifier::BOLD),
            ),
        ])),
        area,
    );
}

fn render_listing(st: &MemoryBrowserState, area: Rect, frame: &mut Frame) {
    if st.loading && st.rows.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::styled(
                "  Loading…",
                Style::default().fg(theme::text_dim()),
            )),
            area,
        );
        return;
    }
    if let Some(err) = &st.error {
        frame.render_widget(
            Paragraph::new(vec![
                Line::styled(format!("  ✗ {}", err), Style::default().fg(theme::error())),
                Line::styled(
                    "  press y to copy · Esc to close",
                    Style::default().fg(theme::text_dim()),
                ),
            ]),
            area,
        );
        return;
    }

    let idxs = st.filtered_indices();
    if idxs.is_empty() {
        let msg = if !st.rows.is_empty() {
            "  no matches for filter".to_string()
        } else {
            match st.level {
                MemoryLevel::Actors => "  (no actors have written to this store)".to_string(),
                MemoryLevel::Sessions => "  (no sessions for this actor)".to_string(),
                MemoryLevel::Events => "  (no events in this session)".to_string(),
                MemoryLevel::Records => {
                    if st.strategies.is_empty() {
                        // Short-term-only stores have no strategies at all, so
                        // there is nothing Records could ever list.
                        "  (no long-term strategies — this store keeps events only)".to_string()
                    } else {
                        "  (no records for this strategy)".to_string()
                    }
                }
            }
        };
        frame.render_widget(
            Paragraph::new(Line::styled(msg, Style::default().fg(theme::text_dim()))),
            area,
        );
        return;
    }

    let height = area.height as usize;
    let offset = st.selected.saturating_sub(height.saturating_sub(1));
    // Secondary column is a fixed right-hand gutter (timestamp + roles);
    // the primary text takes whatever is left.
    let sec_w = 28usize.min(area.width as usize / 3);
    let prim_w = (area.width as usize).saturating_sub(sec_w + 5);

    let drillable = matches!(st.level, MemoryLevel::Actors | MemoryLevel::Sessions);
    let mut lines: Vec<Line> = Vec::new();
    for (row, &i) in idxs.iter().enumerate().skip(offset).take(height) {
        let r = &st.rows[i];
        let marker = if drillable { " ▸ " } else { "   " };
        let line = Line::from(vec![
            Span::styled(
                marker,
                Style::default().fg(if drillable {
                    theme::aws_orange()
                } else {
                    theme::text_dim()
                }),
            ),
            Span::styled(
                format!("{:<w$}", trunc(&r.primary, prim_w), w = prim_w),
                Style::default().fg(theme::text_primary()),
            ),
            Span::styled(
                format!("  {}", trunc(&r.secondary, sec_w)),
                Style::default().fg(theme::text_dim()),
            ),
        ]);
        lines.push(if row == st.selected {
            line.patch_style(theme::selection_style(true))
        } else {
            line
        });
    }
    frame.render_widget(Paragraph::new(lines), area);
}

fn render_payload(st: &MemoryBrowserState, area: Rect, frame: &mut Frame) {
    let Some(row) = st.selected_row() else {
        return;
    };
    let lines: Vec<Line> = row
        .detail
        .lines()
        .skip(st.detail_scroll)
        .take(area.height as usize)
        .map(|l| Line::styled(format!(" {}", l), Style::default().fg(theme::text_primary())))
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
}

fn render_help(area: Rect, frame: &mut Frame) {
    let rows = [
        ("⏎ / l", "drill in (actors → sessions → events)"),
        ("h", "up a level"),
        ("j / k", "move"),
        ("i", "toggle the payload preview"),
        ("J / K", "scroll the payload preview"),
        ("/", "filter the current listing"),
        ("n", "fetch the next page"),
        ("t", "toggle Records (long-term memory)"),
        ("[ / ]", "previous / next strategy (Records only)"),
        ("r", "reload the current listing"),
        ("e", "open the selected payload in $EDITOR"),
        ("y", "copy the selected payload"),
        ("Z", "full-width / split"),
        ("Esc", "close"),
    ];
    let mut lines = vec![Line::raw("")];
    for (k, d) in rows {
        lines.push(Line::from(vec![
            Span::styled(
                format!("  {:<8}", k),
                Style::default().fg(theme::accent()).add_modifier(Modifier::BOLD),
            ),
            Span::styled(d.to_string(), Style::default().fg(theme::text_muted())),
        ]));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

fn render_status(st: &MemoryBrowserState, area: Rect, frame: &mut Frame) {
    if let Some(msg) = &st.message {
        frame.render_widget(
            Paragraph::new(Line::styled(
                format!(" {}", msg),
                Style::default().fg(theme::success()),
            )),
            area,
        );
        return;
    }
    let idxs = st.filtered_indices();
    let mut spans = vec![Span::styled(
        format!(" {} {}", idxs.len(), st.level.label().to_lowercase()),
        Style::default().fg(theme::text_dim()),
    )];
    if st.level == MemoryLevel::Records && st.strategies.len() > 1 {
        spans.push(Span::styled(
            format!(
                "  ·  strategy {}/{}",
                st.strategy_idx + 1,
                st.strategies.len()
            ),
            Style::default().fg(theme::text_dim()),
        ));
    }
    if st.next_token.is_some() {
        spans.push(Span::styled(
            "  ·  n more".to_string(),
            Style::default().fg(theme::accent()),
        ));
    }
    if st.loading {
        spans.push(Span::styled(
            "  ·  loading…".to_string(),
            Style::default().fg(theme::text_dim()),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_hr(area: Rect, frame: &mut Frame) {
    let line = "─".repeat(area.width as usize);
    frame.render_widget(
        Paragraph::new(Line::styled(
            line,
            Style::default().fg(theme::border_dim()),
        )),
        area,
    );
}

fn trunc(s: &str, w: usize) -> String {
    // Row text comes straight from a model's output, so it can carry newlines
    // and control characters that would corrupt the frame.
    let flat: String = s
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    if flat.chars().count() <= w {
        return flat;
    }
    if w <= 1 {
        return "…".to_string();
    }
    let mut out: String = flat.chars().take(w - 1).collect();
    out.push('…');
    out
}
