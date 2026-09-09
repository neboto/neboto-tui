//! "Referenced by" lens (`U`), rendered **in the detail pane** (the rich
//! detail-pane view + `Z` full-width pattern, like the timeline). Lists every
//! resource in the **warm caches** that mentions the selected one — which
//! instances use this security group, what runs as this role, what
//! encrypts with this key — with the field that carried the reference.
//!
//! Zero API: the walk is `App::open_refs_lens` over `cache.get_ref` for every
//! `ServiceType`, matched by `crate::references`. Coverage is stated in the
//! header (searched N loaded services · M not loaded) rather than implied.
//! `f` cycles a per-service filter, `⏎` jumps to the row (the `@all`
//! result jump), `y` copies its id.

use crate::aws::service::ServiceType;
use crate::references::RefRow;
use crate::theme;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

#[derive(Debug, Clone, Default)]
pub struct RefsLensState {
    pub title: String,
    /// Identifiers that were searched for (id + distinct name).
    pub keys: Vec<String>,
    /// Every match, sorted by service then name.
    pub all_rows: Vec<RefRow>,
    /// `all_rows` narrowed by `service_filter`.
    pub rows: Vec<RefRow>,
    pub service_filter: Option<ServiceType>,
    pub searched_services: usize,
    pub unloaded_services: usize,
    pub selected: usize,
    pub scroll: usize,
    pub message: Option<String>,
}

impl RefsLensState {
    pub fn open(title: String, keys: Vec<String>) -> Self {
        Self {
            title,
            keys,
            ..Default::default()
        }
    }

    pub fn set_rows(&mut self, mut rows: Vec<RefRow>, searched: usize, unloaded: usize) {
        rows.sort_by(|a, b| {
            a.service
                .name()
                .cmp(b.service.name())
                .then_with(|| a.resource_type.cmp(&b.resource_type))
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        self.all_rows = rows;
        self.searched_services = searched;
        self.unloaded_services = unloaded;
        self.rebuild();
    }

    /// Services present in the results, in display order — the `f` cycle.
    pub fn services_present(&self) -> Vec<ServiceType> {
        let mut out: Vec<ServiceType> = Vec::new();
        for r in &self.all_rows {
            if !out.contains(&r.service) {
                out.push(r.service);
            }
        }
        out
    }

    pub fn rebuild(&mut self) {
        self.rows = self
            .all_rows
            .iter()
            .filter(|r| self.service_filter.is_none_or(|f| f == r.service))
            .cloned()
            .collect();
        if self.rows.is_empty() {
            self.selected = 0;
            self.scroll = 0;
        } else if self.selected >= self.rows.len() {
            self.selected = self.rows.len() - 1;
        }
    }

    pub fn cycle_service_filter(&mut self) {
        let present = self.services_present();
        self.service_filter = match self.service_filter {
            None => present.first().copied(),
            Some(cur) => present
                .iter()
                .position(|s| *s == cur)
                .and_then(|i| present.get(i + 1).copied()),
        };
        self.rebuild();
    }

    pub fn selected_row(&self) -> Option<&RefRow> {
        self.rows.get(self.selected)
    }

    pub fn scroll_up(&mut self, n: usize) {
        self.selected = self.selected.saturating_sub(n);
    }

    pub fn scroll_down(&mut self, n: usize) {
        if !self.rows.is_empty() {
            self.selected = (self.selected + n).min(self.rows.len() - 1);
        }
    }

    pub fn goto_top(&mut self) {
        self.selected = 0;
    }

    pub fn goto_bottom(&mut self) {
        self.selected = self.rows.len().saturating_sub(1);
    }
}

pub fn render_refs_lens(app: &crate::app::App, area: Rect, frame: &mut Frame) {
    let st = match &app.refs_in_pane {
        Some(s) => s,
        None => return,
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::border_dim()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // header
            Constraint::Length(1), // coverage · keys
            Constraint::Min(0),    // body
            Constraint::Length(1), // footer
        ])
        .split(inner);

    // ── Header: badge · title · count · filter · message ──
    let badge = Span::styled(
        " ⇠ REFERENCED BY ",
        Style::default()
            .fg(Color::Black)
            .bg(theme::accent())
            .add_modifier(Modifier::BOLD),
    );
    let mut header = vec![
        badge,
        Span::raw(" "),
        Span::styled(
            st.title.clone(),
            Style::default()
                .fg(theme::aws_orange())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(
                "  {} ref{}",
                st.rows.len(),
                if st.rows.len() == 1 { "" } else { "s" }
            ),
            Style::default().fg(theme::text_dim()),
        ),
    ];
    if let Some(f) = st.service_filter {
        header.push(Span::styled(
            format!("  · @{} only", f.prefix()),
            Style::default().fg(theme::accent()),
        ));
    }
    if let Some(m) = &st.message {
        header.push(Span::styled(
            format!("   {}", m),
            Style::default().fg(theme::success()),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(header)), chunks[0]);

    // ── Coverage row: honest about what was and wasn't searched ──
    let mut cov = vec![
        Span::raw(" "),
        Span::styled(
            format!(
                "searched {} loaded service{}",
                st.searched_services,
                if st.searched_services == 1 { "" } else { "s" }
            ),
            Style::default().fg(theme::text_dim()),
        ),
    ];
    if st.unloaded_services > 0 {
        cov.push(Span::styled(
            format!(
                " · {} not loaded (open a service to include it)",
                st.unloaded_services
            ),
            Style::default().fg(theme::text_dim()),
        ));
    }
    cov.push(Span::styled(
        format!("  · keys: {}", st.keys.join(", ")),
        Style::default().fg(theme::text_dim()),
    ));
    frame.render_widget(Paragraph::new(Line::from(cov)), chunks[1]);

    // ── Body ──
    let viewport = chunks[2].height as usize;
    let top = if st.selected < st.scroll {
        st.selected
    } else if viewport > 0 && st.selected >= st.scroll + viewport {
        st.selected + 1 - viewport
    } else {
        st.scroll
    };

    let mut body: Vec<Line> = Vec::with_capacity(viewport);
    if st.rows.is_empty() {
        let empty = if st.searched_services == 0 {
            "  Nothing loaded yet — open the services that might reference this, then U again"
        } else if st.service_filter.is_some() {
            "  No references from this service — f cycles"
        } else {
            "  No loaded resource references this one (only loaded services were searched)"
        };
        body.push(Line::styled(empty, Style::default().fg(theme::text_dim())));
    } else {
        for (i, row) in st.rows.iter().enumerate().skip(top).take(viewport) {
            let selected = i == st.selected;
            let marker = if selected { "▸ " } else { "  " };
            let base = if selected {
                Style::default()
                    .fg(theme::text_primary())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme::text_primary())
            };
            let spans = vec![
                Span::styled(marker, Style::default().fg(theme::accent())),
                Span::styled(
                    format!("{:<10} ", trim(&format!("@{}", row.service.prefix()), 10)),
                    Style::default().fg(theme::heading()),
                ),
                Span::styled(
                    format!("{:<22} ", trim(&row.resource_type, 22)),
                    base.fg(theme::text_dim()),
                ),
                Span::styled(format!("{:<32} ", trim(&row.name, 32)), base),
                Span::styled(
                    format!("{:<26} ", trim(&row.id, 26)),
                    base.fg(theme::text_dim()),
                ),
                Span::styled(
                    format!("via {}", trim(&row.via, 40)),
                    base.fg(theme::accent()),
                ),
            ];
            body.push(Line::from(spans));
        }
    }
    frame.render_widget(Paragraph::new(body), chunks[2]);

    // ── Footer ──
    let hint = |k: &'static str, d: &'static str| {
        vec![
            Span::styled(k, Style::default().fg(theme::warning())),
            Span::styled(d, Style::default().fg(theme::text_dim())),
        ]
    };
    let mut spans = vec![Span::raw("  ")];
    spans.extend(hint("j/k", " select  "));
    spans.extend(hint("⏎", " go to  "));
    spans.extend(hint("f", " service  "));
    spans.extend(hint("r", " rescan  "));
    spans.extend(hint("y", " copy id  "));
    spans.extend(hint("Z", " width  "));
    spans.extend(hint("Esc", " close"));
    frame.render_widget(Paragraph::new(Line::from(spans)), chunks[3]);
}

fn trim(s: &str, width: usize) -> String {
    if s.chars().count() <= width {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(width.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}
