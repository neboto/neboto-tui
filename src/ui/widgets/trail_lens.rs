//! Change timeline ("what changed around this resource?"), rendered **in
//! the detail pane** (the canonical rich detail-pane view + `Z` full-width
//! pattern, like the log tail and metric overlays). `W` on any resource
//! merges four sources into one time-sorted list, each row wearing a
//! colored source badge:
//!
//! - `CT`  — CloudTrail `LookupEvents` for the resource's
//!   `trail_lookup_keys` (one-shot — throttled at 2 TPS, so no polling).
//! - `ALM` — state changes of alarms matched to the resource by walking the
//!   **warm CloudWatch cache** (dimension values vs id/name), then
//!   `DescribeAlarmHistory` per match (capped).
//! - `CFN` — the owning stack's `DescribeStackEvents`, when the ownership
//!   resolver (`crate::ownership`) finds a stack tag; filtered to this
//!   resource's logical id plus stack-level rows.
//! - `DEP` — ECS deployments + service events, already on the struct
//!   (zero fetch).
//!
//! `f` cycles a source filter; `Enter`/`v` opens the selected row (raw JSON
//! for CloudTrail, a text summary otherwise) in `$EDITOR`. Sources land
//! independently (`Event::TrailLensLoaded` / `TrailLensAux`) — the lens
//! stays **off** the LazyStore on purpose: like the metrics maps it
//! deliberately refetches per open. The fan-out lives in
//! `App::seed_trail_lens`; the row model in `crate::timeline`.

use crate::aws::services::cloudtrail::TrailRange;
use crate::theme;
use crate::timeline::{TimelineRow, TimelineSource};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

#[derive(Debug, Clone, Default)]
pub struct TrailLensState {
    pub title: String,
    /// Per-source rows, replaced whole when a source lands.
    pub trail_rows: Vec<TimelineRow>,
    pub alarm_rows: Vec<TimelineRow>,
    pub cfn_rows: Vec<TimelineRow>,
    pub deploy_rows: Vec<TimelineRow>,
    /// The merged, filtered, time-sorted view the body renders — rebuilt by
    /// [`rebuild`](Self::rebuild) on every batch arrival / filter change.
    pub rows: Vec<TimelineRow>,
    /// Currently selected row (index into `rows`).
    pub selected: usize,
    /// Top visible row (recomputed on scroll to keep `selected` on screen).
    pub scroll: usize,
    pub range: TrailRange,
    /// Include read-only CloudTrail events (default: mutations only). `a`.
    pub include_reads: bool,
    /// Sources with a fetch still in flight.
    pub pending: Vec<TimelineSource>,
    /// `f`: show only one source; `None` = all.
    pub source_filter: Option<TimelineSource>,
    /// CloudTrail (the primary source) failure — headline slot.
    pub error: Option<String>,
    /// Dim per-source notes: an aux-source failure, or the cold-cache
    /// "alarms unavailable" coverage note. Reset by [`begin`](Self::begin).
    pub aux_notes: Vec<String>,
    /// Transient status (e.g. "copied"), shown in the header.
    pub message: Option<String>,
    /// Ownership summary line (`crate::ownership`), shown in the header.
    pub ownership: Option<String>,
}

impl TrailLensState {
    pub fn open(title: String, range: TrailRange, include_reads: bool) -> Self {
        Self {
            title,
            range,
            include_reads,
            ..Default::default()
        }
    }

    /// Start (or restart) a fetch pass: these sources are now in flight.
    /// Existing rows stay visible until their replacement lands.
    pub fn begin(&mut self, sources: Vec<TimelineSource>) {
        self.pending = sources;
        self.error = None;
        self.aux_notes.clear();
    }

    pub fn is_loading(&self) -> bool {
        !self.pending.is_empty()
    }

    /// A source's rows landed: store, clear its pending mark, re-merge.
    pub fn set_rows(&mut self, source: TimelineSource, rows: Vec<TimelineRow>) {
        match source {
            TimelineSource::Trail => self.trail_rows = rows,
            TimelineSource::Alarm => self.alarm_rows = rows,
            TimelineSource::Cfn => self.cfn_rows = rows,
            TimelineSource::Deploy => self.deploy_rows = rows,
        }
        self.pending.retain(|s| *s != source);
        self.rebuild();
    }

    /// A source's fetch failed. CloudTrail is the headline slot; auxiliary
    /// sources degrade to a dim note — their absence shouldn't read like
    /// the lens itself broke.
    pub fn set_source_error(&mut self, source: TimelineSource, err: String) {
        self.pending.retain(|s| *s != source);
        match source {
            TimelineSource::Trail => self.error = Some(err),
            s => self.aux_notes.push(format!("{}: {}", s.label(), err)),
        }
    }

    /// Re-merge `rows` from the per-source lists, honoring the source
    /// filter, newest first (rows without a timestamp sink to the bottom).
    pub fn rebuild(&mut self) {
        let mut merged: Vec<TimelineRow> = Vec::with_capacity(
            self.trail_rows.len()
                + self.alarm_rows.len()
                + self.cfn_rows.len()
                + self.deploy_rows.len(),
        );
        for list in [
            &self.trail_rows,
            &self.alarm_rows,
            &self.cfn_rows,
            &self.deploy_rows,
        ] {
            for row in list.iter() {
                if self.source_filter.is_none_or(|f| f == row.source) {
                    merged.push(row.clone());
                }
            }
        }
        merged.sort_by_key(|r| std::cmp::Reverse(r.ts_secs));
        self.rows = merged;
        if self.selected >= self.rows.len() {
            self.selected = self.rows.len().saturating_sub(1);
        }
    }

    /// `f`: all → CT → ALM → CFN → DEP → all.
    pub fn cycle_source_filter(&mut self) {
        self.source_filter = match self.source_filter {
            None => Some(TimelineSource::ALL[0]),
            Some(cur) => TimelineSource::ALL
                .iter()
                .position(|s| *s == cur)
                .and_then(|i| TimelineSource::ALL.get(i + 1))
                .copied(),
        };
        self.selected = 0;
        self.scroll = 0;
        self.rebuild();
    }

    pub fn selected_row(&self) -> Option<&TimelineRow> {
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

fn source_color(source: TimelineSource) -> Color {
    match source {
        TimelineSource::Trail => theme::accent(),
        TimelineSource::Alarm => theme::warning(),
        TimelineSource::Cfn => theme::heading(),
        TimelineSource::Deploy => theme::success(),
    }
}

pub fn render_trail_lens(app: &crate::app::App, area: Rect, frame: &mut Frame) {
    let st = match &app.trail_in_pane {
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
            Constraint::Length(1), // source counts · ownership · notes
            Constraint::Min(0),    // body
            Constraint::Length(1), // footer
        ])
        .split(inner);

    // ── Header: badge · title · count · range · mode · filter · message ──
    let badge = Span::styled(
        " ⏱ TIMELINE ",
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
            format!("  {} rows", st.rows.len()),
            Style::default().fg(theme::text_dim()),
        ),
        Span::styled(
            format!("  · last {}", st.range.label()),
            Style::default().fg(theme::accent()),
        ),
        Span::styled(
            if st.include_reads {
                "  · all events"
            } else {
                "  · mutations only"
            },
            Style::default().fg(theme::text_dim()),
        ),
    ];
    if let Some(f) = st.source_filter {
        header.push(Span::styled(
            format!("  · {} only", f.label()),
            Style::default().fg(source_color(f)),
        ));
    }
    if st.is_loading() {
        header.push(Span::styled(
            format!("  {} looking up…", theme::spinner(app.tick_count)),
            Style::default().fg(theme::warning()),
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

    // ── Source-count row: per-source totals, ownership, degrade notes ──
    let mut counts: Vec<Span> = vec![Span::raw(" ")];
    for (source, list) in [
        (TimelineSource::Trail, &st.trail_rows),
        (TimelineSource::Alarm, &st.alarm_rows),
        (TimelineSource::Cfn, &st.cfn_rows),
        (TimelineSource::Deploy, &st.deploy_rows),
    ] {
        let in_flight = st.pending.contains(&source);
        if list.is_empty() && !in_flight {
            continue;
        }
        counts.push(Span::styled(
            format!("{} ", source.badge().trim()),
            Style::default().fg(source_color(source)),
        ));
        counts.push(Span::styled(
            if in_flight {
                "… ".to_string()
            } else {
                format!("{}  ", list.len())
            },
            Style::default().fg(theme::text_dim()),
        ));
    }
    if let Some(own) = &st.ownership {
        counts.push(Span::styled(
            format!(" ⛓ {}", own),
            Style::default().fg(theme::text_dim()),
        ));
    }
    for note in &st.aux_notes {
        counts.push(Span::styled(
            format!("  · {}", note),
            Style::default().fg(theme::text_dim()),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(counts)), chunks[1]);

    // ── Body: one row per timeline entry, selectable ──
    let viewport = chunks[2].height as usize;
    // Keep the selection on screen.
    let top = if st.selected < st.scroll {
        st.selected
    } else if viewport > 0 && st.selected >= st.scroll + viewport {
        st.selected + 1 - viewport
    } else {
        st.scroll
    };

    let mut body: Vec<Line> = Vec::with_capacity(viewport);
    if st.rows.is_empty() {
        let empty = if st.is_loading() {
            "  Looking up changes…"
        } else if st.error.is_some() {
            "  CloudTrail lookup failed (see header)"
        } else if st.source_filter.is_some() {
            "  No rows from this source in range — f cycles sources, ] widens"
        } else {
            "  No changes for this resource in range — widen (]) or toggle reads (a)"
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
            let what_style = if row.failed {
                base.fg(theme::error())
            } else {
                base
            };
            let mut spans = vec![
                Span::styled(marker, Style::default().fg(theme::accent())),
                Span::styled(
                    format!("{:16} ", trim(&row.time, 16)),
                    base.fg(theme::text_dim()),
                ),
                Span::styled(
                    format!("{} ", row.source.badge()),
                    Style::default().fg(source_color(row.source)),
                ),
                Span::styled(format!("{:28} ", trim(&row.what, 28)), what_style),
                Span::styled(
                    format!("{:20} ", trim(&row.who, 20)),
                    base.fg(theme::accent()),
                ),
                Span::styled(
                    format!("{} ", trim(&row.detail, 60)),
                    base.fg(theme::text_dim()),
                ),
            ];
            if !row.error.is_empty() {
                spans.push(Span::styled(
                    row.error.clone(),
                    Style::default().fg(theme::error()),
                ));
            }
            body.push(Line::from(spans));
        }
    }
    frame.render_widget(Paragraph::new(body), chunks[2]);

    // ── Footer: key hints ──
    let hint = |k: &'static str, d: &'static str| {
        vec![
            Span::styled(k, Style::default().fg(theme::warning())),
            Span::styled(d, Style::default().fg(theme::text_dim())),
        ]
    };
    let mut spans = vec![Span::raw("  ")];
    spans.extend(hint("j/k", " select  "));
    spans.extend(hint("⏎/v", " open  "));
    spans.extend(hint("f", " sources  "));
    spans.extend(hint("[/]", " window  "));
    spans.extend(hint(
        "a",
        if st.include_reads {
            " mutations  "
        } else {
            " +reads  "
        },
    ));
    spans.extend(hint("r", " refresh  "));
    spans.extend(hint("y", " copy  "));
    spans.extend(hint("Z", " width  "));
    spans.extend(hint("Esc", " close"));
    frame.render_widget(Paragraph::new(Line::from(spans)), chunks[3]);
}

/// Truncate to `width` display chars, adding `…` when clipped.
fn trim(s: &str, width: usize) -> String {
    if s.chars().count() <= width {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(width.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}
