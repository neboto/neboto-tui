//! Live CloudWatch Logs tail, rendered **in the detail pane** (the canonical
//! "rich detail-pane view + `Z` full-width" pattern, like the S3 object browser
//! and metrics overlays). A background loop polls `filter_log_events` and sends
//! `LogTailBatch` events; this view appends them, auto-following the bottom
//! until the user scrolls up.

use crate::aws::services::cloudwatch::{LogSearchRange, LogTailLine};
use crate::theme;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

/// Keep the buffer bounded — a chatty service would grow it without limit.
const MAX_LINES: usize = 10_000;

/// Whether the pane is a live tail or a one-shot historical search. Both render
/// identically; only the header/footer chrome and the input handling differ.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LogPaneMode {
    #[default]
    Tail,
    Search,
}

#[derive(Debug, Clone, Default)]
pub struct LogTailState {
    pub visible: bool,
    pub title: String,
    pub lines: Vec<LogTailLine>,
    /// Index of the top visible line when **not** following (manual scroll).
    pub scroll: usize,
    /// The top line index the renderer actually drew last frame (filtered
    /// space). While following, the renderer computes the bottom-pinned top
    /// itself and `scroll` goes stale — pausing must seed `scroll` from this
    /// or the view jumps to wherever `scroll` last was (the top of the
    /// buffer, on a fresh tail) instead of freezing in place. A `Cell`
    /// because widgets render from `&App` (the `click_regions` pattern).
    pub render_top: std::cell::Cell<usize>,
    /// Auto-scroll to the newest line. Set false when the user scrolls up.
    pub follow: bool,
    pub filter: String,
    pub filter_active: bool,
    pub error: Option<String>,
    /// Transient status (e.g. "copied"), shown in the header.
    pub message: Option<String>,
    /// Wrap long lines onto continuation rows (hanging indent under the
    /// message column) instead of clipping at the pane edge. Seeded from
    /// `App.log_wrap` on open; `w` toggles.
    pub wrap: bool,

    // ── Search mode ──
    pub mode: LogPaneMode,
    /// The log group + streams the pane is bound to (so search can re-run and
    /// the tail can flip to search without re-resolving). Empty for tail sources
    /// whose group is resolved asynchronously (ECS/NFW/CodeBuild).
    pub group: String,
    pub streams: Vec<String>,
    /// The CloudWatch Logs filter pattern being edited / last run.
    pub query: String,
    pub query_active: bool,
    pub range: LogSearchRange,
    /// A search request is in flight.
    pub searching: bool,
    /// The last search hit the result cap (so results are incomplete).
    pub capped: bool,
}

impl LogTailState {
    pub fn open(title: String) -> Self {
        Self {
            visible: true,
            title,
            lines: Vec::new(),
            scroll: 0,
            render_top: std::cell::Cell::new(0),
            follow: true,
            filter: String::new(),
            filter_active: false,
            error: None,
            message: None,
            wrap: false,
            mode: LogPaneMode::Tail,
            group: String::new(),
            streams: Vec::new(),
            query: String::new(),
            query_active: false,
            range: LogSearchRange::default(),
            searching: false,
            capped: false,
        }
    }

    /// Open the pane in search mode bound to a specific log group + streams.
    pub fn open_search(title: String, group: String, streams: Vec<String>) -> Self {
        Self {
            mode: LogPaneMode::Search,
            group,
            streams,
            follow: true,
            searching: true,
            ..Self::open(title)
        }
    }

    /// Indices (into `lines`) that pass the current filter.
    pub fn filtered(&self) -> Vec<usize> {
        if self.filter.is_empty() {
            return (0..self.lines.len()).collect();
        }
        let needle = self.filter.to_lowercase();
        self.lines
            .iter()
            .enumerate()
            .filter(|(_, l)| {
                l.message.to_lowercase().contains(&needle)
                    || l.stream.to_lowercase().contains(&needle)
            })
            .map(|(i, _)| i)
            .collect()
    }

    pub fn append(&mut self, mut batch: Vec<LogTailLine>) {
        if batch.is_empty() {
            return;
        }
        self.lines.append(&mut batch);
        if self.lines.len() > MAX_LINES {
            let drop = self.lines.len() - MAX_LINES;
            self.lines.drain(0..drop);
            self.scroll = self.scroll.saturating_sub(drop);
        }
    }

    pub fn scroll_up(&mut self, n: usize) {
        if self.follow {
            // Leaving follow: freeze at the position actually on screen, not
            // wherever `scroll` was left before follow took over.
            self.follow = false;
            self.scroll = self.render_top.get();
        }
        self.scroll = self.scroll.saturating_sub(n);
    }

    pub fn scroll_down(&mut self, n: usize, viewport: usize) {
        let max_top = self.filtered().len().saturating_sub(viewport);
        self.scroll = (self.scroll + n).min(max_top);
        // Re-enable follow once scrolled back to the bottom.
        if self.scroll >= max_top {
            self.follow = true;
        }
    }

    pub fn goto_top(&mut self) {
        self.follow = false;
        self.scroll = 0;
    }

    pub fn goto_bottom(&mut self) {
        self.follow = true;
    }
}

/// Columns the timestamp/stream prefix occupies (continuation rows of a
/// multi-line event pad the same timestamp width, so both shapes share it).
fn prefix_width(l: &LogTailLine) -> usize {
    let mut w = l.ts.chars().count() + 1;
    if !l.continuation && !l.stream.is_empty() {
        w += l.stream.chars().count() + 3; // "[…] "
    }
    w
}

/// Screen rows this line occupies at `width` — 1 when clipping; with wrap on,
/// wrapped rows hang under the message column so every chunk gets the same
/// available width.
fn line_row_count(l: &LogTailLine, width: usize, wrap: bool) -> usize {
    if !wrap {
        return 1;
    }
    let avail = width.saturating_sub(prefix_width(l)).max(1);
    let msg = l.message.chars().count();
    if msg <= avail { 1 } else { msg.div_ceil(avail) }
}

/// Render one buffer line into screen rows. Clipping mode emits the single
/// row exactly as before; wrap mode chunks the message at the available width
/// and indents wrapped rows to the message column (hanging indent — the same
/// visual convention multi-line events already use for their continuation
/// rows, so a wrapped record reads as one block).
fn push_line_rows(rows: &mut Vec<Line<'static>>, l: &LogTailLine, width: usize, wrap: bool) {
    let mut first = if l.continuation {
        vec![Span::raw(" ".repeat(l.ts.chars().count() + 1))]
    } else {
        vec![Span::styled(
            format!("{} ", l.ts),
            Style::default().fg(theme::text_dim()),
        )]
    };
    if !l.continuation && !l.stream.is_empty() {
        first.push(Span::styled(
            format!("[{}] ", l.stream),
            Style::default().fg(theme::accent()),
        ));
    }
    let msg_style = Style::default().fg(crate::ui::theme::text_primary());
    if !wrap {
        first.push(Span::styled(l.message.clone(), msg_style));
        rows.push(Line::from(first));
        return;
    }
    let pw = prefix_width(l);
    let avail = width.saturating_sub(pw).max(1);
    let chars: Vec<char> = l.message.chars().collect();
    if chars.len() <= avail {
        first.push(Span::styled(l.message.clone(), msg_style));
        rows.push(Line::from(first));
        return;
    }
    for (n, chunk) in chars.chunks(avail).enumerate() {
        let text: String = chunk.iter().collect();
        if n == 0 {
            let mut spans = std::mem::take(&mut first);
            spans.push(Span::styled(text, msg_style));
            rows.push(Line::from(spans));
        } else {
            rows.push(Line::from(vec![
                Span::raw(" ".repeat(pw)),
                Span::styled(text, msg_style),
            ]));
        }
    }
}

pub fn render_log_tail(app: &crate::app::App, area: Rect, frame: &mut Frame) {
    let st = &app.log_tail;

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::border_dim()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // header
            Constraint::Min(0),    // body
            Constraint::Length(1), // footer / filter
        ])
        .split(inner);

    // ── Header: title · live/paused/search · counts · transient message ──
    let indices = st.filtered();
    let search_mode = st.mode == LogPaneMode::Search;
    let badge = if search_mode {
        Span::styled(
            " 🔍 SEARCH ",
            Style::default().fg(Color::Black).bg(theme::accent()).add_modifier(Modifier::BOLD),
        )
    } else if st.follow {
        Span::styled(
            " ● LIVE ",
            Style::default().fg(Color::Black).bg(theme::success()).add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled(
            " ❚❚ PAUSED ",
            Style::default().fg(Color::Black).bg(theme::warning()).add_modifier(Modifier::BOLD),
        )
    };
    let mut header = vec![
        badge,
        Span::raw(" "),
        Span::styled(
            st.title.clone(),
            Style::default().fg(theme::aws_orange()).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  {} lines", st.lines.len()),
            Style::default().fg(theme::text_dim()),
        ),
    ];
    if st.wrap {
        header.push(Span::styled(
            "  · wrap",
            Style::default().fg(theme::accent()),
        ));
    }
    if !search_mode {
        // The lookback window the tail is seeded from (console-style).
        header.push(Span::styled(
            format!("  · last {}", st.range.label()),
            Style::default().fg(theme::accent()),
        ));
    }
    if search_mode {
        let pat = if st.query.is_empty() { "(all)" } else { st.query.as_str() };
        header.push(Span::styled(
            format!("  pattern: {}", pat),
            Style::default().fg(crate::ui::theme::warning()),
        ));
        header.push(Span::styled(
            format!("  range: {}", st.range.label()),
            Style::default().fg(theme::accent()),
        ));
        if st.searching {
            header.push(Span::styled(
                format!("  {} searching…", theme::spinner(app.tick_count)),
                Style::default().fg(theme::warning()),
            ));
        } else if st.capped {
            header.push(Span::styled(
                "  ⚠ capped",
                Style::default().fg(theme::warning()),
            ));
        }
    }
    if !st.filter.is_empty() {
        header.push(Span::styled(
            format!("  /{}  ({} match)", st.filter, indices.len()),
            Style::default().fg(crate::ui::theme::warning()),
        ));
    }
    if let Some(m) = &st.message {
        header.push(Span::styled(
            format!("   {}", m),
            Style::default().fg(theme::success()),
        ));
    }
    if let Some(e) = &st.error {
        header.push(Span::styled(
            format!("   ⚠ {}", e),
            Style::default().fg(theme::error()),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(header)), chunks[0]);

    // ── Body: newest at the bottom; follow pins to the tail ──
    let viewport = chunks[1].height as usize;
    let width = chunks[1].width as usize;

    let mut body: Vec<Line> = Vec::with_capacity(viewport);
    if indices.is_empty() {
        let empty = if search_mode {
            if st.searching {
                "  Searching…"
            } else {
                "  No matching events in range — edit the pattern (/) or widen the range (⇥)"
            }
        } else {
            "  Waiting for log events…"
        };
        body.push(Line::styled(empty, Style::default().fg(theme::text_dim())));
        st.render_top.set(0);
    } else {
        // With wrap on, one buffer line can span several screen rows, so the
        // follow window is computed bottom-up by accumulated row count —
        // Paragraph::wrap can't do this (it wraps a top-anchored slice and
        // clips the newest rows, exactly the ones a tail exists to show).
        let start = if st.follow {
            let mut rows = 0usize;
            let mut start = indices.len();
            while start > 0 && rows < viewport {
                start -= 1;
                rows += line_row_count(&st.lines[indices[start]], width, st.wrap);
            }
            start
        } else {
            st.scroll.min(indices.len().saturating_sub(viewport.min(indices.len())))
        };
        // Record what's actually on screen so pausing can freeze in place.
        st.render_top.set(start);
        for &i in indices.iter().skip(start) {
            push_line_rows(&mut body, &st.lines[i], width, st.wrap);
            if !st.follow && body.len() >= viewport {
                break;
            }
        }
        if st.follow && body.len() > viewport {
            // Keep the newest rows (a partial first line shows its tail).
            body = body.split_off(body.len() - viewport);
        } else {
            body.truncate(viewport);
        }
    }
    frame.render_widget(Paragraph::new(body), chunks[1]);

    // ── Footer: pattern/filter input line, or key hints ──
    let hint = |k: &'static str, d: &'static str| {
        vec![
            Span::styled(k, Style::default().fg(crate::ui::theme::warning())),
            Span::styled(d, Style::default().fg(theme::text_dim())),
        ]
    };
    let footer = if st.query_active {
        // Editing the search pattern.
        Line::from(vec![
            Span::styled("  pattern ", Style::default().fg(theme::accent())),
            Span::styled("❯ ", Style::default().fg(crate::ui::theme::warning())),
            Span::styled(st.query.clone(), Style::default().fg(crate::ui::theme::text_primary())),
            Span::styled("█", Style::default().fg(crate::ui::theme::warning())),
            Span::styled("   ⏎ run · Esc cancel", Style::default().fg(theme::text_dim())),
        ])
    } else if st.filter_active {
        Line::from(vec![
            Span::styled("  /", Style::default().fg(crate::ui::theme::warning())),
            Span::styled(st.filter.clone(), Style::default().fg(crate::ui::theme::text_primary())),
            Span::styled("█", Style::default().fg(crate::ui::theme::warning())),
            Span::styled("   Esc clear", Style::default().fg(theme::text_dim())),
        ])
    } else if search_mode {
        let mut spans = vec![Span::raw("  ")];
        spans.extend(hint("/", " pattern  "));
        spans.extend(hint("⇥", " range  "));
        spans.extend(hint("⏎", " run  "));
        spans.extend(hint("j/k", " scroll  "));
        spans.extend(hint("y", " copy  "));
        spans.extend(hint("e", " editor  "));
        spans.extend(hint("w", " wrap  "));
        spans.extend(hint("Z", " width  "));
        spans.extend(hint("Esc", " close"));
        Line::from(spans)
    } else {
        let mut spans = vec![Span::raw("  ")];
        spans.extend(hint("j/k", " scroll  "));
        spans.extend(hint("f", " follow  "));
        spans.extend(hint("G", " bottom  "));
        spans.extend(hint("[/]", " window  "));
        spans.extend(hint("/", " filter  "));
        spans.extend(hint("s", " search  "));
        spans.extend(hint("c", " clear  "));
        spans.extend(hint("y", " copy  "));
        spans.extend(hint("e", " editor  "));
        spans.extend(hint("w", " wrap  "));
        spans.extend(hint("Z", " width  "));
        spans.extend(hint("Esc", " stop"));
        Line::from(spans)
    };
    frame.render_widget(Paragraph::new(footer), chunks[2]);
}
