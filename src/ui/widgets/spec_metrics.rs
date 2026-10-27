//! The spec-driven metrics view (`m` on a kind listed in `aws::metrics`),
//! rendered in the detail pane (`Z` full width). Ported from nebaz's
//! metrics view: one row per metric — label, sparkline, last / min / max /
//! avg in the metric's unit — and a braille chart of the selected one below,
//! drawn with its group's other metrics (in beside out) and the `W`
//! timeline's change markers.
//!
//! `[` / `]` or `1`–`4` pick the range and refetch, `s` cycles the selected
//! metric's statistic, `a` lists every metric CloudWatch has for the
//! resource, `d` returns to the curated set. A metric that fails is a
//! warning on its own row, never an error for the view.

use crate::app::App;
use crate::aws::metrics::{fmt_value, BreakdownData, MetricDefinition, MetricQuery, MetricSeries, MetricTarget, RANGES};
use crate::aws::services::ec2::MetricsTimeRange;
use crate::ui::theme;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Axis, Block, Borders, Chart, Dataset, GraphType, Paragraph};
use ratatui::Frame;

/// The `a` picker: the resource's metrics as `ListMetrics` reports them.
#[derive(Debug, Clone, Default)]
pub struct MetricPicker {
    pub defs: Vec<MetricDefinition>,
    pub loading: bool,
    pub error: Option<String>,
    pub selected: usize,
}

#[derive(Debug, Clone)]
pub struct SpecMetricsState {
    pub target: MetricTarget,
    pub range: MetricsTimeRange,
    /// What is charted: the curated set, or one picked metric.
    pub queries: Vec<MetricQuery>,
    /// `queries` is the target's curated set (`d` returns to it).
    pub curated: bool,
    pub series: Vec<MetricSeries>,
    pub loading: bool,
    pub selected: usize,
    /// The window's end, epoch seconds (the chart's `now`).
    pub end: i64,
    pub picker: Option<MetricPicker>,
    /// The target's breakdown tables for the range, once fetched.
    pub breakdown: Option<Result<BreakdownData, String>>,
    /// The range `breakdown` is for: the tables don't change with the
    /// charted set (`⏎`, `s`, `d`), so only a new range or `r` refetches.
    pub breakdown_for: Option<MetricsTimeRange>,
    /// `Tab` into the tables: the cursor, counting rows through every table.
    pub table_cursor: Option<usize>,
    /// The breakdown row charted in place of the set (`⏎` on it).
    pub drilled: Option<String>,
    /// Said on a row in place of "no data", by label (`TargetFix::notes`):
    /// why the metric can't exist.
    pub notes: Vec<(String, &'static str)>,
}

impl SpecMetricsState {
    pub fn open(target: MetricTarget, range: MetricsTimeRange) -> Self {
        let queries = target.curated_queries();
        Self {
            target,
            range,
            queries,
            curated: true,
            series: Vec::new(),
            loading: true,
            selected: 0,
            end: 0,
            picker: None,
            breakdown: None,
            breakdown_for: None,
            table_cursor: None,
            drilled: None,
            notes: Vec::new(),
        }
    }

    fn tables(&self) -> &[crate::aws::metrics::BreakdownTable] {
        match &self.breakdown {
            Some(Ok(d)) => &d.tables,
            _ => &[],
        }
    }

    /// Rows across every table, the cursor's range.
    pub fn table_rows(&self) -> usize {
        self.tables().iter().map(|t| t.rows.len()).sum()
    }

    /// The cursor as (table index, row within it).
    fn cursor_pos(&self) -> Option<(usize, usize)> {
        let mut i = self.table_cursor?;
        for (n, t) in self.tables().iter().enumerate() {
            if i < t.rows.len() {
                return Some((n, i));
            }
            i -= t.rows.len();
        }
        None
    }

    /// The cursor's table and row within it.
    pub fn cursor_row(&self) -> Option<(&crate::aws::metrics::BreakdownTable, usize)> {
        let (n, r) = self.cursor_pos()?;
        Some((&self.tables()[n], r))
    }

    /// What `x` samples: the cursor's rule, else the drilled one, else the
    /// default action (`None`). `GetSampledRequests` is per rule, so a label
    /// row is an error; labels are the only rows with a `:` (rule metric
    /// names can't hold one).
    pub fn sample_rule(&self) -> Result<Option<String>, &'static str> {
        let name = self.cursor_row().map(|(t, r)| t.rows[r].0.clone()).or_else(|| self.drilled.clone());
        match name {
            Some(n) if n.contains(':') => Err("Sampled requests are per rule: pick a rule row, not a label"),
            Some(n) if n == "Default_Action" => Ok(None),
            other => Ok(other),
        }
    }

    /// `⏎` on a table row: chart that row's own series. The cursor stays,
    /// so the next `j` + `⏎` charts the next one.
    pub fn drill(&mut self) -> bool {
        let Some((t, r)) = self.cursor_row() else { return false };
        let Some(queries) = t.drill.as_ref().and_then(|d| d.queries(r)) else { return false };
        self.drilled = Some(t.rows[r].0.clone());
        self.queries = queries;
        self.curated = false;
        self.series.clear();
        self.selected = 0;
        true
    }

    pub fn set_series(&mut self, end: i64, series: Vec<MetricSeries>) {
        self.loading = false;
        self.end = end;
        self.series = series;
        self.selected = self.selected.min(self.series.len().saturating_sub(1));
    }

    pub fn selected_series(&self) -> Option<&MetricSeries> {
        self.series.get(self.selected)
    }

    /// `j`/`k`: the picker's cursor while it is open, the tables' while
    /// they have focus, else the metric's.
    pub fn move_by(&mut self, delta: isize) {
        let rows = self.table_rows();
        let (cur, len) = match (self.picker.as_mut(), self.table_cursor.as_mut()) {
            (Some(p), _) => (&mut p.selected, p.defs.len()),
            (None, Some(c)) => (c, rows),
            (None, None) => (&mut self.selected, self.series.len()),
        };
        let max = len.saturating_sub(1) as isize;
        *cur = (*cur as isize + delta).clamp(0, max.max(0)) as usize;
    }

    /// Every row, as text (the `e` editor view).
    pub fn as_text(&self) -> String {
        let mut out = format!("Metrics — {} — last {}\n", self.target.title, self.range.label());
        let dims: Vec<String> = self.target.dimensions.iter().map(|(n, v)| format!("{n}={v}")).collect();
        out.push_str(&self.target.namespace);
        if !dims.is_empty() {
            out.push_str(&format!(" · {}", dims.join(", ")));
        }
        if let Some(home) = self.target.home {
            out.push_str(&format!(" · in {home}"));
        }
        out.push_str("\n\n");
        for s in &self.series {
            out.push_str(&s.summary());
            out.push('\n');
        }
        match &self.breakdown {
            Some(Ok(data)) => {
                if let Some(summary) = &data.summary {
                    out.push_str(&format!("\n{summary}\n"));
                }
                for t in &data.tables {
                    let (header, rows) = t.full_text();
                    out.push_str(&format!("\n{} ({})\n{header}\n", t.title, self.range.label()));
                    for r in rows {
                        out.push_str(&r);
                        out.push('\n');
                    }
                }
            }
            Some(Err(e)) => out.push_str(&format!("\nBreakdown: {e}\n")),
            None => {}
        }
        out
    }
}

/// One character per column, the column's mean on an eight-step scale.
fn spark_line(points: &[(i64, f64)], width: usize) -> String {
    const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let n = points.len();
    if n == 0 || width == 0 {
        return String::new();
    }
    let lo = points.iter().map(|(_, y)| *y).fold(f64::MAX, f64::min);
    let hi = points.iter().map(|(_, y)| *y).fold(f64::MIN, f64::max);
    let span = (hi - lo).max(f64::EPSILON);
    let width = width.min(n);
    (0..width)
        .map(|i| {
            let start = i * n / width;
            let end = (((i + 1) * n) / width).max(start + 1).min(n);
            let mean = points[start..end].iter().map(|(_, y)| *y).sum::<f64>() / (end - start) as f64;
            BARS[(((mean - lo) / span) * 7.0).round().clamp(0.0, 7.0) as usize]
        })
        .collect()
}

/// The series palette, cycled: theme accessors only.
fn series_color(i: usize) -> Color {
    [theme::success(), theme::accent(), theme::warning(), theme::heading(), theme::aws_orange(), theme::border_focused()]
        [i % 6]
}

/// A row's colour: its tone when the spec gives one (blocked is red), else
/// its place in the palette.
fn row_color(st: &SpecMetricsState, i: usize) -> Color {
    use crate::aws::metrics::Tone;
    match st.series.get(i).and_then(|s| s.query.tone) {
        Some(Tone::Good) => theme::success(),
        Some(Tone::Bad) => theme::error(),
        Some(Tone::Warn) => theme::warning(),
        None => series_color(i),
    }
}

pub fn render_spec_metrics(app: &App, area: Rect, frame: &mut Frame) {
    let Some(st) = &app.spec_metrics else { return };
    let block = Block::default()
        .title(Line::from(vec![
            Span::styled(" Metrics — ", Style::default().fg(theme::text_dim())),
            Span::styled(
                st.target.title.clone(),
                Style::default().fg(theme::aws_orange()).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
        ]))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::border_dim()));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    app.popup_hits.borrow_mut().record_area(inner);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0), Constraint::Length(1)])
        .split(inner);

    // ── Range: each chip presses its digit ──
    let dim = Style::default().fg(theme::text_dim());
    let mut range = vec![Span::styled("  Range ", dim), Span::styled("[ ", Style::default().fg(theme::warning()))];
    let row = chunks[0];
    let mut x = row.x + Line::from(range.clone()).width() as u16;
    for (i, r) in RANGES.iter().enumerate() {
        let chip = format!(" {} {} ", i + 1, r.label());
        let w = chip.chars().count() as u16;
        let right = (row.x + row.width).min(x + w);
        if right > x {
            app.push_click_region(
                Rect { x, y: row.y, width: right - x, height: 1 },
                crate::app::ClickAction::Press(crossterm::event::KeyCode::Char(char::from(b'1' + i as u8))),
            );
        }
        x += w;
        let style = if *r == st.range {
            Style::default().fg(theme::text_primary()).add_modifier(Modifier::BOLD | Modifier::REVERSED)
        } else {
            dim
        };
        range.push(Span::styled(chip, style));
    }
    range.push(Span::styled(" ]", Style::default().fg(theme::warning())));
    // Which set is charted needs no word: a picked metric's footer offers
    // `d default set`, and the `a` list is plainly a list.
    range.push(Span::styled(format!("   {}", st.target.namespace), dim));
    // A metric row's title is only the metric's name: its dimensions are
    // what say which instance or queue it is. A resource's title says so,
    // except where a dimension was looked up (WAF's `WebACL` metric name):
    // that one is shown, so an empty chart says what it asked for.
    // A drilled row names itself instead (below): the line has no room for both.
    if st.drilled.is_none()
        && (st.target.specs.is_empty() || st.target.breakdown.is_some())
        && !st.target.dimensions.is_empty()
    {
        let dims: Vec<String> = st.target.dimensions.iter().map(|(n, v)| format!("{n}={v}")).collect();
        range.push(Span::styled(format!(" · {}", dims.join(", ")), dim));
    }
    if let Some(home) = st.target.home {
        range.push(Span::styled(format!(" in {home}"), dim));
    }
    // A breakdown row charted in place of the set says which.
    if let Some(row) = &st.drilled {
        range.push(Span::styled(format!(" · ▸ {row}"), Style::default().fg(theme::accent()).add_modifier(Modifier::BOLD)));
    }
    let failed = st.series.iter().filter(|s| s.error.is_some()).count();
    if failed > 0 && !st.loading {
        range.push(Span::styled(format!("  · {failed} unread"), Style::default().fg(theme::warning())));
    }
    if st.loading || st.picker.as_ref().is_some_and(|p| p.loading) {
        range.push(Span::styled(
            format!("  {} reading CloudWatch…", theme::spinner(app.tick_count)),
            Style::default().fg(theme::warning()),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(range)), row);

    // ── Body ──
    match &st.picker {
        Some(p) => render_picker(app, p, chunks[1], frame),
        None => render_series(app, st, chunks[1], frame),
    }

    // ── Footer ──
    let hints: Vec<(&str, &str)> = if st.picker.is_some() {
        vec![("j/k", "select"), ("⏎", "chart it"), ("Esc", "back")]
    } else if st.table_cursor.is_some() {
        let back: &[(&str, &str)] = if st.curated { &[] } else { &[("d", "default set")] };
        [&[("j/k", "select"), ("⏎", "chart it"), ("x", "samples"), ("y", "copy name")][..], back, &[("Tab", "metrics"), ("Esc", "back")]]
            .concat()
    } else {
        // `d` only means something once a picked metric replaced the set.
        let back: &[(&str, &str)] = if st.curated { &[] } else { &[("d", "default set")] };
        let changes: &[(&str, &str)] =
            if app.metrics_supports_markers() { &[("c", "changes"), ("W", "timeline")] } else { &[] };
        let tables: &[(&str, &str)] = match (st.table_rows() > 0, st.target.breakdown.is_some()) {
            (true, _) => &[("Tab", "rules"), ("x", "samples")],
            (false, true) => &[("x", "samples")],
            _ => &[],
        };
        [&[("[/]", "range"), ("s", "stat"), ("a", "all")][..], tables, back, changes, &[("y", "copy"), ("e", "edit"), ("Esc", "close")]]
            .concat()
    };
    // A narrow pane drops hints from the end, but `Esc` stays the last.
    let mut hints = hints;
    while hints.len() > 2 && theme::hint_line(&hints).width() + 2 > chunks[2].width as usize {
        hints.remove(hints.len() - 2);
    }
    let mut footer = theme::hint_line(&hints);
    footer.spans.insert(0, Span::raw("  "));
    frame.render_widget(Paragraph::new(footer), chunks[2]);
}

fn render_picker(app: &App, p: &MetricPicker, area: Rect, frame: &mut Frame) {
    let mut lines: Vec<Line> = Vec::new();
    let dim = Style::default().fg(theme::text_dim());
    if let Some(e) = &p.error {
        lines.push(Line::styled(format!("  ⚠ {e}"), Style::default().fg(theme::warning())));
    } else if p.defs.is_empty() {
        let msg = if p.loading { "  Reading ListMetrics…" } else { "  CloudWatch has no metrics with this resource's dimensions" };
        lines.push(Line::styled(msg, dim));
    }
    let room = (area.height as usize).saturating_sub(lines.len());
    let top = if room > 0 && p.selected >= room { p.selected + 1 - room } else { 0 };
    let skip = lines.len() as u16;
    app.popup_hits.borrow_mut().record_rows(
        Rect { y: area.y + skip, height: area.height.saturating_sub(skip), ..area },
        top,
        p.defs.len(),
    );
    for (i, d) in p.defs.iter().enumerate().skip(top).take(room) {
        let sel = i == p.selected;
        let name = Style::default().fg(theme::text_primary());
        let others: Vec<String> = d.dimensions.iter().map(|(n, v)| format!("{n}={v}")).collect();
        lines.push(Line::from(vec![
            Span::styled(if sel { "▸ " } else { "  " }, Style::default().fg(theme::accent())),
            Span::styled(d.metric.clone(), if sel { name.add_modifier(Modifier::BOLD) } else { name }),
            Span::styled(format!("  {}", d.namespace), dim),
            Span::styled(format!("  {}", others.join(", ")), dim.add_modifier(Modifier::ITALIC)),
        ]));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

fn render_series(app: &App, st: &SpecMetricsState, area: Rect, frame: &mut Frame) {
    if st.series.is_empty() {
        let msg = if st.loading { "  Reading…" } else { "  Nothing to show" };
        frame.render_widget(Paragraph::new(Line::styled(msg, Style::default().fg(theme::text_dim()))), area);
        return;
    }
    // The list on top; the selected metric's chart below when there is room
    // for it (the list and a blank line first).
    let list_h = st.series.len() as u16;
    let (list_area, chart_area) = if area.height >= list_h + 8 {
        let parts = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(list_h + 1), Constraint::Min(0)])
            .split(area);
        (parts[0], Some(parts[1]))
    } else {
        (area, None)
    };

    let label_w = st.series.iter().map(|s| s.query.label.chars().count()).max().unwrap_or(0).min(24);
    let width = list_area.width as usize;
    // The stats as columns, padded to the widest value so every sparkline
    // is the same width and the numbers line up. When the pane is narrow
    // the columns go (avg, then min) before the sparkline gets short.
    let cells: Vec<Option<[String; 4]>> = st
        .series
        .iter()
        .map(|s| {
            let f = |v| fmt_value(v, s.query.unit);
            s.stats().filter(|_| s.error.is_none()).map(|t| [f(t.last), f(t.min), f(t.max), f(t.avg)])
        })
        .collect();
    let col_w: [usize; 4] = std::array::from_fn(|c| {
        cells.iter().flatten().map(|row| row[c].chars().count()).max().unwrap_or(0)
    });
    const NAMES: [&str; 4] = ["last", "min", "max", "avg"];
    const SPARK_MIN: usize = 16;
    let used = 2 + label_w + 1;
    let tail_w = |cols: &[usize]| -> usize { cols.iter().map(|&c| NAMES[c].len() + 3 + col_w[c]).sum::<usize>() + 6 };
    let shown: &[usize] = [&[0, 1, 2, 3][..], &[0, 1, 2], &[0, 2]]
        .into_iter()
        .find(|cols| width.saturating_sub(used + tail_w(cols)) >= SPARK_MIN)
        .unwrap_or(&[0, 2]);
    let spark_w = width.saturating_sub(used + tail_w(shown)).min(48);

    let mut lines = Vec::new();
    for (i, (s, row)) in st.series.iter().zip(&cells).enumerate() {
        let sel = i == st.selected;
        let bold = |s: Style| if sel { s.add_modifier(Modifier::BOLD) } else { s };
        let mut spans = vec![
            Span::styled(if sel { "▸ " } else { "  " }, Style::default().fg(theme::accent())),
            Span::styled(
                format!("{:<w$} ", s.query.label.chars().take(label_w).collect::<String>(), w = label_w),
                bold(Style::default().fg(theme::text_primary())),
            ),
        ];
        match (&s.error, row) {
            (Some(e), _) => spans.push(Span::styled(format!("⚠ {e}"), Style::default().fg(theme::warning()))),
            (None, None) => {
                let note = st.notes.iter().find(|(l, _)| *l == s.query.label).map_or("no data in the window", |(_, n)| *n);
                spans.push(Span::styled(note, Style::default().fg(theme::text_dim())));
            }
            (None, Some(row)) => {
                let spark = spark_line(&s.points, spark_w);
                let pad = spark_w.saturating_sub(spark.chars().count());
                spans.push(Span::styled(spark, Style::default().fg(row_color(st, i))));
                spans.push(Span::raw(" ".repeat(pad + 1)));
                for &c in shown {
                    spans.push(Span::styled(format!("{} ", NAMES[c]), Style::default().fg(theme::text_dim())));
                    spans.push(Span::styled(
                        format!("{:>w$}  ", row[c], w = col_w[c]),
                        bold(Style::default().fg(theme::text_muted())),
                    ));
                }
                let how = if s.query.expression.is_some() { "math" } else { s.query.stat.short() };
                spans.push(Span::styled(format!("({how})"), Style::default().fg(theme::text_dim())));
            }
        }
        lines.push(Line::from(spans));
    }
    // Nothing at all came back: say what has to be on for it to exist.
    let all_empty = st.series.iter().all(|s| s.error.is_none() && s.points.is_empty());
    if all_empty && !st.loading {
        // A stopped instance or an idle queue publishes nothing, so a short
        // window can be empty while last week's data is still there.
        let longest = RANGES[RANGES.len() - 1];
        let hint = match st.target.hint {
            Some(h) => h.to_string(),
            None if st.range != longest => {
                format!("Nothing published in the last {} — {} for the last {}", st.range.label(), RANGES.len(), longest.label())
            }
            None => format!("Nothing published in the last {}", longest.label()),
        };
        lines.push(Line::styled(format!("  ℹ {hint}"), Style::default().fg(theme::text_dim())));
    }
    frame.render_widget(Paragraph::new(lines), list_area);
    app.popup_hits.borrow_mut().record_rows(list_area, 0, st.series.len());

    let Some(chart_area) = chart_area else { return };
    // A target with a breakdown shares the chart's space with its tables:
    // beside the chart when the pane is wide, under it when it is tall.
    let (chart_area, table_area) = match &st.target.breakdown {
        None => (chart_area, None),
        Some(_) if chart_area.width >= 110 => {
            let parts = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
                .split(chart_area);
            (parts[0], Some(parts[1]))
        }
        Some(_) if chart_area.height >= 18 => {
            let parts = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(9), Constraint::Percentage(45)])
                .split(chart_area);
            (parts[0], Some(parts[1]))
        }
        Some(_) => (chart_area, None),
    };
    if let Some(sel) = st.selected_series() {
        // The selected metric and its group's others, each in its row colour.
        let drawn: Vec<(usize, &MetricSeries)> = match &sel.query.group {
            Some(g) => st.series.iter().enumerate().filter(|(_, s)| s.query.group.as_ref() == Some(g)).collect(),
            None => vec![(st.selected, sel)],
        };
        render_chart(st, &drawn, chart_area, frame);
    }
    if let Some(area) = table_area {
        render_breakdown(st, area, frame);
    }
}

/// The target's breakdown tables (WAF's requests by rule and by label) for
/// the view's range. What doesn't fit is left to `e`, which lists them all.
fn render_breakdown(st: &SpecMetricsState, area: Rect, frame: &mut Frame) {
    let dim = Style::default().fg(theme::text_dim());
    let focused = st.table_cursor.is_some();
    let border = if focused { theme::border_focused() } else { theme::border_dim() };
    let block = Block::default()
        .title(Span::styled(format!(" Totals · last {} ", st.range.label()), dim))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let height = inner.height as usize;
    let mut lines: Vec<Line> = Vec::new();
    match &st.breakdown {
        None => lines.push(Line::styled(if st.loading { " Reading…" } else { " Nothing to show" }, dim)),
        Some(Err(e)) => lines.push(Line::styled(format!(" ⚠ {e}"), Style::default().fg(theme::warning()))),
        Some(Ok(data)) => {
            if let Some(summary) = &data.summary {
                lines.push(Line::styled(
                    format!(" {summary}"),
                    Style::default().fg(theme::text_primary()).add_modifier(Modifier::BOLD),
                ));
                lines.push(Line::raw(""));
            }
            let tables = &data.tables;
            let cursor = st.cursor_pos();
            for (i, t) in tables.iter().enumerate() {
                if i > 0 {
                    lines.push(Line::raw(""));
                }
                lines.push(Line::styled(
                    format!(" {}", t.title),
                    Style::default().fg(theme::heading()).add_modifier(Modifier::BOLD),
                ));
                if t.rows.is_empty() {
                    lines.push(Line::styled(" None in the range", dim));
                    continue;
                }
                // Leave the next table its title, header and a row.
                let reserve = if i + 1 < tables.len() { 4 } else { 0 };
                let room = height.saturating_sub(lines.len() + 1 + reserve).max(1);
                let shown = if t.rows.len() > room { room.saturating_sub(1).max(1) } else { room };
                // Scrolled so the cursor's row is on screen.
                let here = cursor.filter(|(n, _)| *n == i).map(|(_, r)| r);
                let skip = here.map_or(0, |r| (r + 1).saturating_sub(shown));
                let (header, rows) = t.text_window(inner.width as usize, skip, shown);
                lines.push(Line::styled(header, dim));
                for (k, row) in rows.into_iter().enumerate() {
                    let r = skip + k;
                    let name = &t.rows[r].0;
                    let line = if here == Some(r) {
                        // The cursor: `▸` in the leading space, bold.
                        Line::styled(
                            format!("▸{}", &row[1..]),
                            Style::default().fg(theme::accent()).add_modifier(Modifier::BOLD),
                        )
                    } else if st.drilled.as_ref() == Some(name) {
                        Line::styled(row, Style::default().fg(theme::accent()))
                    } else {
                        Line::styled(row, Style::default().fg(theme::text_primary()))
                    };
                    lines.push(line);
                }
                let below = t.rows.len().saturating_sub(skip + shown);
                if below > 0 {
                    lines.push(Line::styled(format!(" · {below} more — e lists them all"), dim));
                }
            }
        }
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

/// The selected metric (with its group) as braille lines over the window.
fn render_chart(st: &SpecMetricsState, drawn: &[(usize, &MetricSeries)], area: Rect, frame: &mut Frame) {
    let x_max = st.range.duration_secs() as f64;
    let start = st.end - st.range.duration_secs();
    let data: Vec<Vec<(f64, f64)>> = drawn
        .iter()
        .map(|(_, s)| s.points.iter().map(|&(t, v)| ((t - start) as f64, v)).collect())
        .collect();
    let all = data.iter().flatten();
    let lo = all.clone().map(|(_, y)| *y).fold(0.0_f64, f64::min);
    let peak = all.map(|(_, y)| *y).fold(f64::MIN, f64::max);
    let hi = if peak > lo { peak + (peak - lo) * 0.1 } else { lo + 1.0 };
    let unit = drawn[0].1.query.unit;
    let label = |v: f64| Span::styled(fmt_value(v, unit), Style::default().fg(theme::text_dim()));

    let marks = super::metrics_overlay::marker_lines(x_max, lo, hi);
    let mut datasets = super::metrics_overlay::marker_datasets(&marks);
    for ((i, s), pts) in drawn.iter().zip(&data) {
        let mut ds = Dataset::default()
            .data(pts)
            .marker(symbols::Marker::Braille)
            .graph_type(GraphType::Line)
            .style(Style::default().fg(row_color(st, *i)));
        // A legend only when there is more than one line to tell apart.
        if drawn.len() > 1 {
            ds = ds.name(s.query.label.clone());
        }
        datasets.push(ds);
    }
    let sel = drawn.iter().find(|(i, _)| *i == st.selected).map(|(_, s)| *s).unwrap_or(drawn[0].1);
    let title = match &sel.query.expression {
        Some(_) => format!(" {} · expression ", sel.query.label),
        None => format!(" {} · {} ({}) ", sel.query.label, sel.query.metric, sel.query.stat.short()),
    };
    let mut block = Block::default()
        .title(Span::styled(title, Style::default().fg(row_color(st, st.selected)).add_modifier(Modifier::BOLD)))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::text_dim()));
    // The change markers' legend on the chart they're drawn on.
    if let Some(mut caption) = super::metrics_overlay::marker_caption(area.width.saturating_sub(4)) {
        caption.spans.insert(0, Span::raw(" "));
        caption.spans.push(Span::raw(" "));
        block = block.title_bottom(caption);
    }
    let chart = Chart::new(datasets)
        .block(block)
        .x_axis(Axis::default().bounds([0.0, x_max]).labels(vec![
            Span::styled(st.range.start_label(), Style::default().fg(theme::text_dim())),
            Span::styled("now", Style::default().fg(theme::text_dim())),
        ]))
        .y_axis(Axis::default().bounds([lo, hi]).labels(vec![label(lo), label((lo + hi) / 2.0), label(hi)]));
    frame.render_widget(chart, area);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spark_line_buckets_and_scales() {
        let pts: Vec<(i64, f64)> = (0..8).map(|i| (i, i as f64)).collect();
        assert_eq!(spark_line(&pts, 8), "▁▂▃▄▅▆▇█");
        assert_eq!(spark_line(&pts, 4).chars().count(), 4);
        assert_eq!(spark_line(&pts[..2], 10).chars().count(), 2, "never wider than the points");
        assert_eq!(spark_line(&[], 10), "");
    }
}
