use crate::app::{
    App, MetricsKind,
};
use crate::aws::services::s3::{S3MetricsData, S3MetricsState};
use crate::aws::services::cloudwatch::{
    CwAnnotation, CwDashboardMetricsData, CwDashboardMetricsState, CwDashboardWidget,
};
use crate::ui::theme;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Axis, Block, Borders, Chart, Dataset, GraphType, Paragraph};
use ratatui::Frame;

// ── Change markers (#245) ────────────────────────────────────────────────────
//
// The `W` timeline's events, drawn as vertical lines on every time-series
// chart: "latency jumped right after this UpdateService" without comparing
// two panes by eye. The per-kind renderers don't know about them. The
// markers ride a render-scoped context that `render_metrics_in_pane` sets
// and every time-series chart builder reads (`marker_lines`), so a new
// metrics kind gets them for free if it draws through those builders.

struct MarkerCtx {
    /// When the chart's fetch window ends: x is seconds from `end − x_max`.
    window_end: i64,
    /// One per timeline row, newest first.
    events: Vec<Change>,
    /// Indices into `events` some chart actually drew, for the legend.
    drawn: std::collections::BTreeSet<usize>,
    loading: bool,
    range: crate::aws::services::cloudtrail::TrailRange,
    /// A renderer drew the legend on its own chart ([`marker_caption`]), so
    /// the pane border stays plain.
    claimed: bool,
    /// The CloudTrail lookup stopped at its cap here: no event before it
    /// was read (`TrailLensState::trail_cut`).
    cut: Option<i64>,
    /// The earliest start of any chart drawn, to tell whether one reaches
    /// back past `cut`.
    earliest: Option<i64>,
}

/// A timeline row, as much of it as the markers use.
struct Change {
    ts: i64,
    failed: bool,
    what: String,
}

thread_local! {
    static MARKERS: std::cell::RefCell<Option<MarkerCtx>> = const { std::cell::RefCell::new(None) };
}

/// One marker: a vertical segment at the event's x, and whether it failed.
pub(super) type MarkerLine = ([(f64, f64); 2], bool);

/// The markers that fall inside a chart spanning `x_max` seconds, as
/// vertical segments from `y_lo` to `y_hi`. Empty outside a marker context.
pub(super) fn marker_lines(x_max: f64, y_lo: f64, y_hi: f64) -> Vec<MarkerLine> {
    MARKERS.with(|m| {
        let mut m = m.borrow_mut();
        let Some(ctx) = m.as_mut() else { return Vec::new() };
        let start = ctx.window_end as f64 - x_max;
        ctx.earliest = Some(ctx.earliest.map_or(start as i64, |e| e.min(start as i64)));
        let mut lines = Vec::new();
        for (i, c) in ctx.events.iter().enumerate() {
            let x = c.ts as f64 - start;
            if (0.0..=x_max).contains(&x) {
                ctx.drawn.insert(i);
                lines.push(([(x, y_lo), (x, y_hi)], c.failed));
            }
        }
        lines
    })
}

/// Unnamed datasets (so the legend box ignores them), meant to go
/// **before** the series so the data draws on top.
pub(super) fn marker_datasets(lines: &[MarkerLine]) -> Vec<Dataset<'_>> {
    lines
        .iter()
        .map(|(pts, failed)| {
            let color = if *failed { theme::error() } else { theme::text_dim() };
            Dataset::default()
                .data(pts)
                .marker(symbols::Marker::Braille)
                .graph_type(GraphType::Line)
                .style(Style::default().fg(color))
        })
        .collect()
}

/// Clears the context even if a renderer panics, so a later frame can't
/// inherit another resource's markers.
struct MarkerScope;

impl Drop for MarkerScope {
    fn drop(&mut self) {
        MARKERS.with(|m| m.borrow_mut().take());
    }
}

/// Render the metric charts for `kind` into `area` (the detail pane). The pane
/// frame is already drawn by the caller, so these no longer `Clear` full-screen.
/// With change markers on, the charts draw inside a marker context and the
/// legend goes on the bottom border.
pub fn render_metrics_in_pane(app: &App, kind: MetricsKind, area: Rect, frame: &mut Frame) {
    let timeline = app
        .chart_timeline
        .as_ref()
        .filter(|_| app.chart_markers && app.metrics_supports_markers());
    // A timeline that loaded empty leaves the charts exactly as they were.
    let Some(tl) = timeline.filter(|t| t.is_loading() || !t.rows.is_empty()) else {
        render_metrics_kind(app, kind, area, frame);
        return;
    };
    let _scope = MarkerScope;
    MARKERS.with(|m| {
        *m.borrow_mut() = Some(MarkerCtx {
            window_end: app.metrics_window_end,
            events: tl
                .rows
                .iter()
                .map(|r| Change { ts: r.ts_secs, failed: r.failed, what: r.what.clone() })
                .collect(),
            drawn: Default::default(),
            loading: tl.is_loading(),
            range: tl.range,
            claimed: false,
            cut: tl.trail_cut,
            earliest: None,
        })
    });
    render_metrics_kind(app, kind, area, frame);
    let Some(ctx) = MARKERS.with(|m| m.borrow_mut().take()) else { return };
    // A hand-built kind with several charts: on the pane's bottom border,
    // like the ownership ribbon (which the metrics view hides), with the
    // keys, since those kinds' own hints don't name them.
    if !ctx.claimed && area.width > 8 && area.height > 2 {
        let row = Rect { x: area.x + 2, y: area.bottom() - 1, width: area.width - 4, height: 1 };
        let mut legend = marker_legend(&ctx, row.width);
        legend.spans.insert(0, Span::styled(" ┊ ", Style::default().fg(theme::text_dim())));
        legend.spans.extend(marker_keys());
        let width = legend.width().min(row.width as usize) as u16;
        frame.render_widget(Paragraph::new(legend), Rect { width, ..row });
    }
}

/// The legend for a renderer to draw on its own chart, from what the
/// charts drawn so far placed (call it after drawing them). Claims the
/// legend, so the pane border stays plain. None outside a marker context.
pub(super) fn marker_caption(width: u16) -> Option<Line<'static>> {
    MARKERS.with(|m| {
        let mut m = m.borrow_mut();
        let ctx = m.as_mut()?;
        ctx.claimed = true;
        Some(marker_legend(ctx, width))
    })
}

fn marker_keys() -> Vec<Span<'static>> {
    let dim = Style::default().fg(theme::text_dim());
    let key = Style::default().fg(theme::warning());
    vec![
        Span::styled("   c", key),
        Span::styled(" hide · ", dim),
        Span::styled("W", key),
        Span::styled(" timeline", dim),
    ]
}

/// The legend: how many changes the charts drew, then one entry per kind
/// of change (newest first, `×N` when it repeats — a nightly scheduler's
/// StopInstances is one entry, not seven), as many as fit in `width`. The
/// lines say when; `W` lists every event.
fn marker_legend(ctx: &MarkerCtx, width: u16) -> Line<'static> {
    let dim = Style::default().fg(theme::text_dim());
    let mut spans = Vec::new();
    // The lookup stopped short of the chart's start: the drawn changes are
    // only the newest, and the stretch before `cut` is unknown, not quiet.
    let cut = ctx.cut.filter(|&c| ctx.earliest.is_some_and(|e| e < c));
    if ctx.drawn.is_empty() {
        let msg = if ctx.loading {
            "loading changes…".to_string()
        } else if let Some(c) = cut {
            let at = crate::aws::services::cloudwatch::fmt_epoch_secs(c);
            format!("no changes since {} (CloudTrail read no further)", &at[5..])
        } else {
            format!("no changes in this window ({} in the last {})", ctx.events.len(), ctx.range.label())
        };
        spans.push(Span::styled(msg, dim));
        return Line::from(spans);
    }
    // (what, count, any failed), in order of each kind's newest event.
    let mut kinds: Vec<(&str, usize, bool)> = Vec::new();
    for &i in &ctx.drawn {
        let c = &ctx.events[i];
        match kinds.iter_mut().find(|k| k.0 == c.what) {
            Some(k) => {
                k.1 += 1;
                k.2 |= c.failed;
            }
            None => kinds.push((&c.what, 1, c.failed)),
        }
    }
    let n = ctx.drawn.len();
    let plus = if cut.is_some() { "+" } else { "" };
    spans.push(Span::styled(format!("{n}{plus} change{}  ", if n == 1 && plus.is_empty() { "" } else { "s" }), dim));
    let more_w = 10;
    let budget = (width as usize).saturating_sub(spans.iter().map(|s| s.width()).sum::<usize>() + more_w);
    let mut used = 0;
    let mut shown = 0;
    for &(what, count, failed) in &kinds {
        let what: String = if what.chars().count() > 40 { what.chars().take(39).chain(['…']).collect() } else { what.to_string() };
        let times = if count > 1 { format!(" ×{count}") } else { String::new() };
        let w = what.chars().count() + times.chars().count() + 3;
        if shown > 0 && used + w > budget {
            break;
        }
        if shown > 0 {
            spans.push(Span::styled(" · ", dim));
        }
        let style = if failed { Style::default().fg(theme::error()) } else { Style::default().fg(theme::text_primary()) };
        spans.push(Span::styled(what, style));
        spans.push(Span::styled(times, dim));
        used += w;
        shown += 1;
    }
    if shown < kinds.len() {
        spans.push(Span::styled(format!("  +{} more", kinds.len() - shown), dim));
    }
    if ctx.loading {
        spans.push(Span::styled("  loading…", dim));
    }
    Line::from(spans)
}

fn render_metrics_kind(app: &App, kind: MetricsKind, area: Rect, frame: &mut Frame) {
    match kind {
        MetricsKind::Spec => super::spec_metrics::render_spec_metrics(app, area, frame),
        MetricsKind::CwDashboard => render_cw_dashboard_metrics_overlay(app, area, frame),
        MetricsKind::Cost => render_cost_metrics_overlay(app, area, frame),
        MetricsKind::S3 => render_s3_metrics_overlay(app, area, frame),
    }
}

// ── ECS service metrics overlay ───────────────────────────────────────────────


// ── ECS per-task / per-container metrics overlay ─────────────────────────────

/// One named, coloured line series for a multi-series chart.
type ChartSeries<'a> = (&'a str, &'a [(f64, f64)], Color);

/// Palette cycled across per-container series so each container's line has a
/// stable, distinguishable colour (legend shows the container name). A
/// function (not a const) because the theme palette is runtime-selected.
fn container_colors() -> [Color; 8] {
    [
        crate::ui::theme::success(),
        crate::ui::theme::accent(),
        crate::ui::theme::warning(),
        crate::ui::theme::heading(),
        Color::Blue,
        Color::LightRed,
        Color::LightGreen,
        Color::LightMagenta,
    ]
}

// ── DynamoDB metrics overlay ──────────────────────────────────────────────────

// ── EFS metrics overlay ───────────────────────────────────────────────────────

// ── ElastiCache metrics overlay ───────────────────────────────────────────────

// ── Kinesis metrics overlay ───────────────────────────────────────────────────

// ── Firehose metrics overlay ──────────────────────────────────────────────────

// ── Redshift metrics overlays ─────────────────────────────────────────────────

// ── SQS metrics overlay ───────────────────────────────────────────────────────

// ── Bedrock model / inference-profile metrics overlay ─────────────────────────

// ── S3 storage metrics overlay ────────────────────────────────────────────────

pub fn render_s3_metrics_overlay(app: &App, area: Rect, frame: &mut Frame) {
    let (name, id) = app
        .get_selected_resource()
        .map(|r| (r.name().to_string(), r.id().to_string()))
        .unwrap_or_else(|| ("Unknown".to_string(), String::new()));

    // S3 storage metrics are daily — a fixed 30-day window, no range knob.
    let chunks = render_metrics_chrome("S3 Storage — ", &name, "30d (daily)", area, frame);

    match app.s3_metrics.get(&id) {
        None | Some(S3MetricsState::Loading) => {
            frame.render_widget(
                Paragraph::new(Line::styled(
                    "  Loading metrics…",
                    Style::default().fg(theme::text_dim()),
                )),
                chunks[1],
            );
        }
        Some(S3MetricsState::Loaded(data)) => render_s3_charts(data, chunks[1], frame),
    }
}

fn render_s3_charts(data: &S3MetricsData, area: Rect, frame: &mut Frame) {
    let x_max = data.x_max;
    let cells = grid_3x2(area);
    render_value_chart("Bucket Size (all storage classes, bytes)", &data.size_bytes, x_max, "30d ago", crate::ui::theme::accent(), cells[0], frame);
    render_value_chart("Object Count", &data.object_count, x_max, "30d ago", crate::ui::theme::success(), cells[1], frame);
}

// ── Route 53 health-check metrics overlay ─────────────────────────────────────

// ── Transfer Family metrics overlay ───────────────────────────────────────────

// ── SES metrics overlay (account-wide, no dimensions) ─────────────────────────



// ── MSK metrics overlay ───────────────────────────────────────────────────────



// ── SNS metrics overlay ───────────────────────────────────────────────────────

// ── Step Functions metrics overlay ──────────────────────────────────────────────

/// Shared overlay chrome (title block + time-range line + footer); returns the
/// `[time_line, body, footer]` chunk layout, with `chunks[1]` for the charts.
fn render_metrics_chrome<'a>(
    title_prefix: &'a str,
    name: &'a str,
    range_label: &'a str,
    area: Rect,
    frame: &mut Frame,
) -> std::rc::Rc<[Rect]> {
    let block = Block::default()
        .title(Line::from(vec![
            Span::styled(title_prefix.to_string(), Style::default().fg(theme::text_dim())),
            Span::styled(
                name.to_string(),
                Style::default()
                    .fg(theme::aws_orange())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" ", Style::default()),
        ]))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::border_dim()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0), Constraint::Length(1)])
        .split(inner);

    let time_line = Line::from(vec![
        Span::styled("  Range ", Style::default().fg(theme::text_dim())),
        Span::styled("[", Style::default().fg(crate::ui::theme::warning())),
        Span::styled(" prev ", Style::default().fg(theme::text_dim())),
        Span::styled(
            format!(" {} ", range_label),
            Style::default().fg(crate::ui::theme::text_primary()).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" next ", Style::default().fg(theme::text_dim())),
        Span::styled("]", Style::default().fg(crate::ui::theme::warning())),
    ]);
    frame.render_widget(Paragraph::new(time_line), chunks[0]);

    let footer = Line::from(vec![
        Span::styled("  Esc", Style::default().fg(crate::ui::theme::warning())),
        Span::styled("/", Style::default().fg(theme::text_dim())),
        Span::styled("m", Style::default().fg(crate::ui::theme::warning())),
        Span::styled("  close    ", Style::default().fg(theme::text_dim())),
        Span::styled("[", Style::default().fg(crate::ui::theme::warning())),
        Span::styled("/", Style::default().fg(theme::text_dim())),
        Span::styled("]", Style::default().fg(crate::ui::theme::warning())),
        Span::styled("  time range    ", Style::default().fg(theme::text_dim())),
        Span::styled("r", Style::default().fg(crate::ui::theme::warning())),
        Span::styled("  refresh    ", Style::default().fg(theme::text_dim())),
        Span::styled("Z", Style::default().fg(crate::ui::theme::warning())),
        Span::styled("  full-width", Style::default().fg(theme::text_dim())),
    ]);
    frame.render_widget(Paragraph::new(footer), chunks[2]);

    chunks
}

/// Lay `area` out as a 3-row × 2-column grid → 6 cells, row-major.
fn grid_3x2(area: Rect) -> Vec<Rect> {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Ratio(1, 3), Constraint::Ratio(1, 3), Constraint::Ratio(1, 3)])
        .split(area);
    let mut cells = Vec::with_capacity(6);
    for row in rows.iter() {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Ratio(1, 2), Constraint::Ratio(1, 2)])
            .split(*row);
        cells.push(cols[0]);
        cells.push(cols[1]);
    }
    cells
}

// ── CloudWatch dashboard grid (`m` on a dashboard) ────────────────────────────

/// Terminal rows per grid unit of dashboard height. The console's default metric
/// widget is 6 units tall; at 1.4 that is an 8-row box — just enough for a
/// braille chart with axes and a legend. Widths need no factor: the grid is 24
/// columns and the pane is divided into 24 proportional slices.
const DASH_ROWS_PER_UNIT: f32 = 1.4;

/// A bordered chart below this has no room left for axes, so the cell degrades
/// to a sparkline row instead of drawing an unreadable 2-row plot.
const DASH_MIN_CHART_W: u16 = 26;
const DASH_MIN_CHART_H: u16 = 7;

fn dash_row(units: u16) -> u16 {
    (units as f32 * DASH_ROWS_PER_UNIT).round() as u16
}

/// Full height of a dashboard's grid in terminal rows — `App` clamps `j`/`G`
/// against this so the scroll offset can't run past the last widget.
pub fn dashboard_grid_rows(widgets: &[CwDashboardWidget]) -> u16 {
    widgets
        .iter()
        .map(|w| dash_row(w.y.saturating_add(w.height)))
        .max()
        .unwrap_or(0)
}

pub fn render_cw_dashboard_metrics_overlay(app: &App, area: Rect, frame: &mut Frame) {
    use crate::aws::services::cloudwatch::CwDashboard;

    let name = app
        .get_selected_resource()
        .and_then(|r| r.as_any().downcast_ref::<CwDashboard>())
        .map(|d| d.name.clone())
        .unwrap_or_default();

    let block = Block::default()
        .title(Line::from(vec![
            Span::styled(" Dashboard — ", Style::default().fg(theme::text_dim())),
            Span::styled(
                name.clone(),
                Style::default().fg(theme::aws_orange()).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
        ]))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::border_dim()));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0), Constraint::Length(1)])
        .split(inner);

    let state = app.cw_dashboard_metrics.get(&name);
    // Drop last frame's click targets up front: on a Loading or Error frame the
    // grid never runs, and stale rects would accept clicks for widgets that are
    // no longer drawn.
    app.cw_dashboard_regions.borrow_mut().clear();

    let mut top = vec![
        Span::styled("  Range ", Style::default().fg(theme::text_dim())),
        Span::styled("[", Style::default().fg(theme::warning())),
        Span::styled(" prev ", Style::default().fg(theme::text_dim())),
        Span::styled(
            format!(" {} ", app.cw_dashboard_metrics_time_range.label()),
            Style::default().fg(theme::text_primary()).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" next ", Style::default().fg(theme::text_dim())),
        Span::styled("]", Style::default().fg(theme::warning())),
    ];
    if let Some(CwDashboardMetricsState::Loaded(d)) = state {
        let charted = d.widgets.iter().filter(|w| w.kind == "metric").count();
        // Series, not queries: the cap counts queries, but one SEARCH() can
        // match hundreds of metrics and GetMetricData bills for every one.
        let series: usize = d.series.values().map(|v| v.len()).sum();
        top.push(Span::styled(
            format!(
                "    {} of {} widgets · {} series",
                charted,
                d.widgets.len(),
                series
            ),
            Style::default().fg(theme::text_dim()),
        ));
        if d.capped {
            top.push(Span::styled(
                format!("    ⚠ capped at {} series", crate::aws::services::cloudwatch::MAX_DASHBOARD_QUERIES),
                Style::default().fg(theme::warning()),
            ));
        }
        if d.math_error.is_some() {
            top.push(Span::styled(
                "    ⚠ metric math dropped",
                Style::default().fg(theme::warning()),
            ));
        }
        if !d.skipped_regions.is_empty() {
            top.push(Span::styled(
                format!("    ⚠ {}", d.skipped_regions.join("; ")),
                Style::default().fg(theme::warning()),
            ));
        }
    }
    frame.render_widget(Paragraph::new(Line::from(top)), chunks[0]);

    let key = |k: &'static str| Span::styled(k, Style::default().fg(theme::warning()));
    let dim = |t: String| Span::styled(t, Style::default().fg(theme::text_dim()));
    let footer = if app.cw_dashboard_zoomed {
        Line::from(vec![
            key("  Esc"),
            dim("  back to grid    ".into()),
            key("⇥"),
            dim("  next widget    ".into()),
            key("["),
            dim("/".into()),
            key("]"),
            dim("  time range    ".into()),
            key("r"),
            dim("  refresh".into()),
        ])
    } else {
        Line::from(vec![
            key("  Esc"),
            dim("/".into()),
            key("m"),
            dim("  close    ".into()),
            key("⇥"),
            dim("  widget    ".into()),
            key("⏎"),
            dim("  zoom    ".into()),
            key("["),
            dim("/".into()),
            key("]"),
            dim("  time range    ".into()),
            key("j"),
            dim("/".into()),
            key("k"),
            dim("  scroll    ".into()),
            key("r"),
            dim("  refresh    ".into()),
            key("w"),
            dim("  watch    ".into()),
            key("Z"),
            dim("  full-width".into()),
        ])
    };
    frame.render_widget(Paragraph::new(footer), chunks[2]);

    match state {
        None | Some(CwDashboardMetricsState::Loading) => {
            let loading = Paragraph::new(Line::from(vec![Span::styled(
                format!("  {} Loading dashboard…", theme::spinner(app.tick_count)),
                Style::default().fg(theme::warning()),
            )]));
            frame.render_widget(loading, chunks[1]);
        }
        Some(CwDashboardMetricsState::Error(e)) => {
            let err = Paragraph::new(Line::from(vec![Span::styled(
                format!("  ⚠ {}", e),
                Style::default().fg(theme::error()),
            )]));
            frame.render_widget(err, chunks[1]);
        }
        Some(CwDashboardMetricsState::Loaded(d)) => {
            render_dashboard_grid(app, d, chunks[1], frame)
        }
    }
}

/// Lay the widgets out on the console's 24-column grid, scaled to the pane, and
/// draw each one. Widgets scrolled fully above the viewport are skipped; a
/// partially-visible one is clipped to what's left rather than dropped.
fn render_dashboard_grid(
    app: &App,
    data: &CwDashboardMetricsData,
    area: Rect,
    frame: &mut Frame,
) {
    if data.widgets.is_empty() || area.height == 0 || area.width == 0 {
        let empty = Paragraph::new(Line::from(vec![Span::styled(
            "  This dashboard has no widgets.",
            Style::default().fg(theme::text_dim()),
        )]));
        frame.render_widget(empty, area);
        return;
    }

    let scroll = app
        .cw_dashboard_scroll
        .min(dashboard_grid_rows(&data.widgets).saturating_sub(area.height));

    // Click targets are recorded as the widgets are drawn, so what the mouse
    // hits is exactly what's on screen — scrolled, clipped and all.
    let mut regions = app.cw_dashboard_regions.borrow_mut();
    regions.clear();

    // Zoom: one widget takes the whole body. The grid underneath isn't drawn —
    // a chart is unreadable at grid scale in a split pane, which is the reason
    // zoom exists.
    if app.cw_dashboard_zoomed {
        if let Some(i) = app.cw_dashboard_cursor {
            if let Some(w) = data.widgets.get(i) {
                regions.push((i, area));
                render_dashboard_widget(i, w, data, true, area, frame);
                return;
            }
        }
    }

    for (i, w) in data.widgets.iter().enumerate() {
        if let Some(rect) = dashboard_widget_rect(w, area, scroll) {
            // Text panels aren't selectable by `Tab`, so they aren't clickable
            // either — clicking one would move the cursor somewhere `Tab` can
            // never return to.
            if w.kind != "text" {
                regions.push((i, rect));
            }
            render_dashboard_widget(i, w, data, app.cw_dashboard_cursor == Some(i), rect, frame);
        }
    }
}

/// Where one widget lands in the pane, or `None` if it is scrolled out of view
/// or too small to draw anything legible.
///
/// Column edges come from the *cumulative* grid position rather than a per-widget
/// width, so side-by-side widgets share an edge instead of rounding into a
/// one-column gap.
fn dashboard_widget_rect(w: &CwDashboardWidget, area: Rect, scroll: u16) -> Option<Rect> {
    let (grid_top, grid_bot) = (dash_row(w.y), dash_row(w.y.saturating_add(w.height)));
    if grid_bot <= scroll {
        return None; // scrolled off the top
    }
    let y0 = area.y + grid_top.saturating_sub(scroll);
    if y0 >= area.y + area.height {
        return None; // below the fold
    }
    let y1 = (area.y + grid_bot.saturating_sub(scroll)).min(area.y + area.height);

    let col = |units: u16| (units.min(24) as u32 * area.width as u32 / 24) as u16;
    let (x0, x1) = (col(w.x), col(w.x.saturating_add(w.width)));

    let rect = Rect {
        x: area.x + x0,
        y: y0,
        width: x1.saturating_sub(x0),
        height: y1.saturating_sub(y0),
    };
    (rect.width >= 4 && rect.height >= 2).then_some(rect)
}

fn render_dashboard_widget(
    idx: usize,
    w: &CwDashboardWidget,
    data: &CwDashboardMetricsData,
    selected: bool,
    rect: Rect,
    frame: &mut Frame,
) {
    let title = if w.title.is_empty() {
        w.kind.clone()
    } else {
        w.title.clone()
    };

    if w.kind == "text" {
        render_dashboard_text(&w.markdown, rect, frame);
        return;
    }
    if w.kind == "alarm" {
        render_dashboard_alarms(&title, w, data, selected, rect, frame);
        return;
    }
    if w.kind != "metric" {
        let note = if w.kind == "log" {
            "Logs Insights query".to_string()
        } else {
            format!("{} widget", w.kind)
        };
        render_dashboard_placeholder(&title, &note, selected, rect, frame);
        return;
    }

    // Labels stay full length here: each renderer elides to its own space (a
    // legend gets half the cell, a tile column gets its column), and eliding
    // twice reads as corruption — `pay-jo…gesSent`.
    let drawn = dashboard_widget_series(idx, w, data);

    if drawn.is_empty() {
        render_dashboard_placeholder(&title, &dashboard_empty_note(w, data), selected, rect, frame);
        return;
    }

    let colors = container_colors();
    match w.view.as_str() {
        "singleValue" => render_dashboard_single_value(&title, w, &drawn, selected, rect, frame),
        "gauge" => render_dashboard_gauge(&title, w, &drawn, selected, rect, frame),
        "bar" | "pie" => render_dashboard_bars(&title, w, &drawn, selected, rect, frame),
        _ => {
            // A widget's legend is what tells two near-identical series apart,
            // so labels are trimmed to fit rather than letting ratatui drop it.
            let label_w = ((rect.width as usize / 2).saturating_sub(4)).max(8);
            let (plotted, right_scale) = dashboard_plot_points(w, &drawn);
            let labels: Vec<String> = drawn
                .iter()
                .map(|s| {
                    // A right-axis series is drawn against a scale the y-labels
                    // don't describe, so the legend has to say which ones.
                    let mark = if s.right_axis { " ›" } else { "" };
                    format!("{}{}", elide_mid(&s.label, label_w), mark)
                })
                .collect();
            let series: Vec<ChartSeries> = plotted
                .iter()
                .zip(labels.iter())
                .enumerate()
                .map(|(i, (points, label))| {
                    (label.as_str(), points.as_slice(), colors[i % colors.len()])
                })
                .collect();
            if rect.width < DASH_MIN_CHART_W || rect.height < DASH_MIN_CHART_H {
                render_dashboard_sparkline(&title, &series, selected, rect, frame);
            } else {
                render_dashboard_chart(
                    &title,
                    &series,
                    &w.annotations,
                    right_scale,
                    data.x_max,
                    data.time_range.start_label(),
                    selected,
                    rect,
                    frame,
                );
            }
        }
    }
}

/// One drawable series: the label to show and the points behind it.
struct DrawnSeries<'a> {
    label: String,
    points: &'a [(f64, f64)],
    /// How the widget aggregates this series to a single number. `Sum` widgets
    /// total the window (a bar chart of request counts); everything else reads
    /// the latest datapoint, which is what the console's big numbers show.
    summed: bool,
    /// Plotted against the widget's second scale (`"yAxis": "right"`).
    right_axis: bool,
}

/// The points to draw, plus the right-hand scale their `›`-marked members were
/// rescaled from (`None` when the widget has only one scale).
type PlottedSeries = (Vec<Vec<(f64, f64)>>, Option<(f64, f64)>);

/// Transform a widget's series into what actually gets plotted, returning the
/// points and the right-hand scale (if any) for the title marker.
///
/// Two transforms, both of which need owned data:
/// - **stacked** widgets are cumulative — each line is itself plus everything
///   below it, so the top line reads as the total. Requires aligned x values;
///   series of differing length fall back to unstacked rather than inventing an
///   alignment.
/// - **right-axis** series are rescaled onto the left axis's range. ratatui
///   charts have one y-axis, and a latency-vs-requests widget otherwise draws
///   the latency flat along the bottom — shape preserved but unreadable. The
///   numbers those lines belong to go in the title, the way you'd read them off
///   a right-hand axis.
fn dashboard_plot_points(w: &CwDashboardWidget, drawn: &[DrawnSeries]) -> PlottedSeries {
    let mut plotted: Vec<Vec<(f64, f64)>> =
        drawn.iter().map(|s| s.points.to_vec()).collect();

    // Right-axis rescaling first: stacking a mix of two scales is meaningless,
    // and after rescaling everything shares one.
    let right: Vec<usize> = drawn
        .iter()
        .enumerate()
        .filter(|(_, s)| s.right_axis)
        .map(|(i, _)| i)
        .collect();

    let mut right_scale = None;
    if !right.is_empty() && right.len() < drawn.len() {
        let (r_lo, r_hi) = match (w.y_right_min, w.y_right_max) {
            (Some(lo), Some(hi)) => (lo, hi),
            _ => {
                // No declared bounds: use the right series' own extent.
                let ys = || right.iter().flat_map(|i| plotted[*i].iter().map(|(_, y)| *y));
                (
                    ys().fold(f64::MAX, f64::min).min(0.0),
                    ys().fold(f64::MIN, f64::max),
                )
            }
        };
        let left_hi = drawn
            .iter()
            .enumerate()
            .filter(|(i, _)| !right.contains(i))
            .flat_map(|(i, _)| plotted[i].iter().map(|(_, y)| *y))
            .fold(0.0_f64, f64::max);
        let r_span = (r_hi - r_lo).abs();
        if r_span > f64::EPSILON && left_hi > 0.0 {
            for i in &right {
                for (_, y) in plotted[*i].iter_mut() {
                    *y = (*y - r_lo) / r_span * left_hi;
                }
            }
            right_scale = Some((r_lo, r_hi));
        }
    }

    if w.stacked && plotted.len() > 1 {
        let len = plotted[0].len();
        if plotted.iter().all(|p| p.len() == len) {
            for i in 1..plotted.len() {
                let (below, rest) = plotted.split_at_mut(i);
                for (point, under) in rest[0].iter_mut().zip(below[i - 1].iter()) {
                    point.1 += under.1;
                }
            }
        }
    }

    (plotted, right_scale)
}

impl DrawnSeries<'_> {
    /// The single number a tile, gauge or bar shows for this series.
    fn value(&self) -> f64 {
        if self.summed {
            self.points.iter().map(|(_, y)| *y).sum()
        } else {
            self.points.last().map(|(_, y)| *y).unwrap_or(0.0)
        }
    }
}

/// Collect a metric widget's visible series in body order, flattening the
/// several series a `SEARCH()` expression returns under one query id. Hidden
/// lines (`"visible": false`) exist only to feed metric math and never draw.
fn dashboard_widget_series<'a>(
    idx: usize,
    w: &CwDashboardWidget,
    data: &'a CwDashboardMetricsData,
) -> Vec<DrawnSeries<'a>> {
    let mut out = Vec::new();
    for (mi, m) in w.metrics.iter().enumerate() {
        if !m.visible {
            continue;
        }
        let id = crate::aws::services::cloudwatch::dashboard_query_id(idx, mi, m.id.as_deref());
        let Some(bucket) = data.series.get(&id) else {
            continue;
        };
        let summed = m
            .stat
            .as_deref()
            .or(w.stat.as_deref())
            .map(|s| s == "Sum")
            .unwrap_or(false);
        for s in bucket.iter().filter(|s| !s.points.is_empty()) {
            // Prefer the label CloudWatch returned: it is the only thing
            // separating a SEARCH()'s several series, and it comes back with
            // any dynamic-label syntax already substituted. Falling back to the
            // widget's own label is what surfaced raw `${PROP('Dim.…')}` on
            // single-series widgets.
            let label = if !s.label.is_empty() {
                s.label.clone()
            } else if !m.label.is_empty() {
                m.label.clone()
            } else {
                m.metric_name.clone()
            };
            out.push(DrawnSeries {
                // Belt and braces: substitute locally too, so an unresolved
                // token can never reach the screen.
                label: crate::aws::services::cloudwatch::resolve_dynamic_label(
                    &label, m, w, &s.points,
                ),
                points: &s.points,
                summed,
                right_axis: m.right_axis,
            });
        }
    }
    out
}

/// Why a metric widget drew nothing — the distinction matters, since "no data"
/// and "we never asked" look identical in an empty box.
fn dashboard_empty_note(w: &CwDashboardWidget, data: &CwDashboardMetricsData) -> String {
    if w.metrics.is_empty() {
        return "no metrics".to_string();
    }
    if let Some(region) = &w.region {
        if data.skipped_regions.iter().any(|s| s.starts_with(region.as_str())) {
            return format!("{} — not fetched", region);
        }
    }
    if data.math_error.is_some() && w.metrics.iter().any(|m| m.expression.is_some()) {
        return "metric math unavailable".to_string();
    }
    "no datapoints in range".to_string()
}

/// A widget we can't chart: its box, its title, and one dim line saying why —
/// so the grid keeps the dashboard's shape instead of leaving a hole.
fn render_dashboard_placeholder(
    title: &str,
    note: &str,
    selected: bool,
    rect: Rect,
    frame: &mut Frame,
) {
    let block = Block::default()
        .title(Span::styled(
            dashboard_title(title, selected),
            Style::default().fg(theme::text_dim()),
        ))
        .borders(Borders::ALL)
        .border_style(dashboard_border(selected));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    if inner.height == 0 {
        return;
    }
    frame.render_widget(
        Paragraph::new(Line::from(vec![Span::styled(
            format!(" · {}", note),
            Style::default().fg(theme::text_dim()),
        )])),
        inner,
    );
}

/// Like `render_multi_chart`, but the legend is never hidden: a dashboard cell
/// usually holds two or three sibling series whose names differ only at the end,
/// and ratatui's default constraints drop the legend at exactly those widths.
#[allow(clippy::too_many_arguments)]
fn render_dashboard_chart(
    title: &str,
    series: &[ChartSeries],
    annotations: &[CwAnnotation],
    right_scale: Option<(f64, f64)>,
    x_max: f64,
    start_label: &'static str,
    selected: bool,
    area: Rect,
    frame: &mut Frame,
) {
    let peak = series
        .iter()
        .flat_map(|(_, d, _)| d.iter().map(|(_, y)| *y))
        .fold(0.0_f64, f64::max);
    let floor = series
        .iter()
        .flat_map(|(_, d, _)| d.iter().map(|(_, y)| *y))
        .fold(0.0_f64, f64::min);
    // A reference line the data never reaches is the interesting case (nothing
    // has hit the limit yet), so the axis has to make room for it.
    let peak = annotations.iter().fold(peak, |acc, a| acc.max(a.value));
    let floor = annotations.iter().fold(floor, |acc, a| acc.min(a.value));
    let y_max = if peak > 0.0 { peak * 1.15 } else { 1.0 };
    let y_min = if floor < 0.0 { floor * 1.15 } else { 0.0 };

    // Flat two-point lines, built before the datasets so they outlive them.
    let bands: Vec<[(f64, f64); 2]> = annotations
        .iter()
        .map(|a| [(0.0, a.value), (x_max, a.value)])
        .collect();

    // A one-series widget is already named by its title; a legend there would
    // just be a box of duplicate text drawn over the data. Only named datasets
    // reach the legend, so leaving the name off is how it stays away.
    let show_legend = series.iter().filter(|(_, d, _)| !d.is_empty()).count() > 1;
    let mut datasets: Vec<Dataset> = series
        .iter()
        .filter(|(_, d, _)| !d.is_empty())
        .map(|(name, d, color)| {
            let ds = Dataset::default()
                .data(d)
                .marker(symbols::Marker::Braille)
                .graph_type(GraphType::Line)
                .style(Style::default().fg(*color));
            if show_legend {
                ds.name(*name)
            } else {
                ds
            }
        })
        .collect();

    // Reference lines go on last so they draw over the data, and stay **out** of
    // the legend: ratatui's legend box costs two rows of chrome plus one per
    // entry, and a widget-sized plot has about four rows to give — an extra
    // entry there hides the legend entirely, losing the series names that
    // actually disambiguate. The labels ride in the title instead, where they
    // cost no plot space and can't be pushed out.
    for band in &bands {
        datasets.push(
            Dataset::default()
                .data(band)
                .marker(symbols::Marker::Braille)
                .graph_type(GraphType::Line)
                .style(Style::default().fg(theme::warning())),
        );
    }

    let mut heading = vec![Span::styled(
        dashboard_title(title, selected),
        Style::default().fg(theme::text_primary()).add_modifier(Modifier::BOLD),
    )];
    if !annotations.is_empty() {
        let marks: Vec<String> = annotations
            .iter()
            .map(|a| {
                if a.label.is_empty() {
                    fmt_dash_value(a.value)
                } else {
                    format!("{} {}", a.label, fmt_dash_value(a.value))
                }
            })
            .collect();
        heading.push(Span::styled(
            format!("╌ {} ", marks.join(" · ")),
            Style::default().fg(theme::warning()),
        ));
    }
    if let Some((lo, hi)) = right_scale {
        // The `›`-marked series are drawn on this scale, not the y-axis's.
        heading.push(Span::styled(
            format!("› {}–{} ", fmt_dash_value(lo), fmt_dash_value(hi)),
            Style::default().fg(theme::text_dim()),
        ));
    }

    let chart = Chart::new(datasets)
        .hidden_legend_constraints((Constraint::Percentage(100), Constraint::Percentage(100)))
        .block(
            Block::default()
                .title(Line::from(heading))
                .borders(Borders::ALL)
                .border_style(dashboard_border(selected)),
        )
        .x_axis(
            Axis::default().bounds([0.0, x_max]).labels(vec![
                Span::styled(start_label, Style::default().fg(theme::text_dim())),
                Span::styled("now", Style::default().fg(theme::text_dim())),
            ]),
        )
        .y_axis(
            Axis::default().bounds([y_min, y_max]).labels(vec![
                Span::styled(fmt_dash_value(y_min), Style::default().fg(theme::text_dim())),
                Span::styled(
                    fmt_dash_value((y_min + y_max) / 2.0),
                    Style::default().fg(theme::text_dim()),
                ),
                Span::styled(fmt_dash_value(y_max), Style::default().fg(theme::text_dim())),
            ]),
        );

    frame.render_widget(chart, area);
}

/// A widget's framed box, returning the usable interior (empty height means
/// there is nothing left to draw into).
/// A `▸` on the cursor's title. Colour alone carries it poorly: the accent
/// varies by theme, and on the light presets a dim and an accent border are
/// close enough to miss at a glance.
fn dashboard_title(title: &str, selected: bool) -> String {
    if selected {
        format!(" ▸ {} ", title)
    } else {
        format!(" {} ", title)
    }
}

/// The cursor is also the widget border going accent — every widget already has
/// one, so nothing shifts when it moves.
fn dashboard_border(selected: bool) -> Style {
    if selected {
        Style::default().fg(theme::accent()).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme::text_dim())
    }
}

fn dashboard_box(title: &str, selected: bool, rect: Rect, frame: &mut Frame) -> Rect {
    let block = Block::default()
        .title(Span::styled(
            dashboard_title(title, selected),
            Style::default().fg(theme::text_primary()).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(dashboard_border(selected));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    inner
}

/// Vertically centre `lines` rows of content in a widget's interior. The console
/// centres its numbers and gauges, and a 3-line tile pinned to the top of an
/// 8-row box reads as a rendering bug rather than a layout choice.
fn center_v(inner: Rect, lines: u16) -> Rect {
    let pad = inner.height.saturating_sub(lines) / 2;
    Rect {
        y: inner.y + pad,
        height: inner.height.saturating_sub(pad),
        ..inner
    }
}

/// `view: singleValue` — the console's big-number tile. Series are laid out
/// across the width, each showing its value over its label, with an optional
/// sparkline row when the author asked for one.
fn render_dashboard_single_value(
    title: &str,
    w: &CwDashboardWidget,
    series: &[DrawnSeries],
    selected: bool,
    rect: Rect,
    frame: &mut Frame,
) {
    let inner = dashboard_box(title, selected, rect, frame);
    if inner.height == 0 || inner.width == 0 || series.is_empty() {
        return;
    }

    let rows = 1 + u16::from(inner.height > 1) + u16::from(w.sparkline && inner.height > 2);
    let inner = center_v(inner, rows);

    // One column per series, but never so narrow that the number is unreadable.
    let max_cols = ((inner.width / 12).max(1) as usize).min(series.len());
    let cols: Vec<Rect> = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(vec![Constraint::Ratio(1, max_cols as u32); max_cols])
        .split(inner)
        .to_vec();

    let colors = container_colors();
    for (i, (s, cell)) in series.iter().zip(cols.iter()).enumerate() {
        let mut lines = vec![Line::from(Span::styled(
            fmt_dash_value(s.value()),
            Style::default()
                .fg(colors[i % colors.len()])
                .add_modifier(Modifier::BOLD),
        ))];
        if cell.height > 1 {
            // One column short of the cell, so neighbouring tiles don't collide.
            lines.push(Line::from(Span::styled(
                elide_mid(&s.label, cell.width.saturating_sub(1) as usize),
                Style::default().fg(theme::text_dim()),
            )));
        }
        if w.sparkline && cell.height > 2 {
            lines.push(Line::from(Span::styled(
                spark_line(s.points, cell.width as usize),
                Style::default().fg(colors[i % colors.len()]),
            )));
        }
        frame.render_widget(Paragraph::new(lines), *cell);
    }
}

/// `view: gauge` — a filled bar against the author's `yAxis.left` bounds. A
/// gauge without bounds is meaningless (the console requires them), so a body
/// missing them falls back to 0–100.
fn render_dashboard_gauge(
    title: &str,
    w: &CwDashboardWidget,
    series: &[DrawnSeries],
    selected: bool,
    rect: Rect,
    frame: &mut Frame,
) {
    let inner = dashboard_box(title, selected, rect, frame);
    if inner.height == 0 || inner.width < 8 {
        return;
    }
    let (lo, hi) = (w.y_min.unwrap_or(0.0), w.y_max.unwrap_or(100.0));
    let span = (hi - lo).abs().max(f64::EPSILON);
    let inner = center_v(inner, series.len().min(inner.height as usize) as u16);

    let lines: Vec<Line> = series
        .iter()
        .take(inner.height as usize)
        .map(|s| {
            let value = s.value();
            let frac = ((value - lo) / span).clamp(0.0, 1.0);
            let label = fmt_dash_value(value);
            let bar_w = inner.width.saturating_sub(label.chars().count() as u16 + 1) as usize;
            let filled = (frac * bar_w as f64).round() as usize;
            // Colour by how full it is — a gauge is drawn to be read at a glance.
            let color = if frac >= 0.9 {
                theme::error()
            } else if frac >= 0.7 {
                theme::warning()
            } else {
                theme::success()
            };
            Line::from(vec![
                Span::styled("█".repeat(filled), Style::default().fg(color)),
                Span::styled(
                    "░".repeat(bar_w.saturating_sub(filled)),
                    Style::default().fg(theme::text_dim()),
                ),
                Span::raw(" "),
                Span::styled(label, Style::default().fg(theme::text_primary())),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

/// `view: bar` / `view: pie` — both become horizontal bars. A pie chart has no
/// honest terminal rendering, and the thing it conveys (relative share) reads
/// fine as bars scaled to the largest slice.
fn render_dashboard_bars(
    title: &str,
    _w: &CwDashboardWidget,
    series: &[DrawnSeries],
    selected: bool,
    rect: Rect,
    frame: &mut Frame,
) {
    let inner = dashboard_box(title, selected, rect, frame);
    if inner.height == 0 || inner.width < 12 {
        return;
    }

    let values: Vec<f64> = series.iter().map(|s| s.value()).collect();
    let peak = values.iter().cloned().fold(0.0_f64, f64::max).max(f64::EPSILON);
    let label_w = (inner.width as usize / 3).clamp(6, 24);
    let colors = container_colors();

    let lines: Vec<Line> = series
        .iter()
        .zip(values.iter())
        .take(inner.height as usize)
        .enumerate()
        .map(|(i, (s, value))| {
            let label = elide_mid(&s.label, label_w);
            let shown = fmt_dash_value(*value);
            let bar_w = (inner.width as usize)
                .saturating_sub(label_w + shown.chars().count() + 2);
            let filled = ((value / peak) * bar_w as f64).round().max(0.0) as usize;
            Line::from(vec![
                Span::styled(
                    format!("{:<w$} ", label, w = label_w),
                    Style::default().fg(theme::text_dim()),
                ),
                Span::styled(
                    "▇".repeat(filled.min(bar_w)),
                    Style::default().fg(colors[i % colors.len()]),
                ),
                Span::raw(" "),
                Span::styled(shown, Style::default().fg(theme::text_primary())),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

/// An alarm widget: one coloured chip per referenced alarm. Unresolved alarms
/// (another account, a denied `DescribeAlarms`) still list by name — the widget
/// says what the dashboard points at even when the state is unknown.
fn render_dashboard_alarms(
    title: &str,
    w: &CwDashboardWidget,
    data: &CwDashboardMetricsData,
    selected: bool,
    rect: Rect,
    frame: &mut Frame,
) {
    let inner = dashboard_box(title, selected, rect, frame);
    if inner.height == 0 {
        return;
    }
    if w.alarm_arns.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                " · no alarms",
                Style::default().fg(theme::text_dim()),
            ))),
            inner,
        );
        return;
    }

    let lines: Vec<Line> = w
        .alarm_arns
        .iter()
        .take(inner.height as usize)
        .map(|arn| {
            let name = arn.split_once(":alarm:").map(|(_, n)| n).unwrap_or(arn);
            let (marker, state, style) = match data.alarms.get(name) {
                Some(chip) => match chip.state.as_str() {
                    "ALARM" => ("●", "ALARM", Style::default().fg(theme::error())),
                    "OK" => ("●", "OK", Style::default().fg(theme::success())),
                    "INSUFFICIENT_DATA" => {
                        ("○", "NO DATA", Style::default().fg(theme::text_dim()))
                    }
                    _ => ("○", "UNKNOWN", Style::default().fg(theme::text_dim())),
                },
                None => ("·", "", Style::default().fg(theme::text_dim())),
            };
            let width = inner.width as usize;
            Line::from(vec![
                Span::styled(format!(" {} ", marker), style),
                Span::styled(format!("{:<8}", state), style.add_modifier(Modifier::BOLD)),
                Span::styled(
                    elide_mid(name, width.saturating_sub(13)),
                    Style::default().fg(theme::text_primary()),
                ),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

/// Text widgets draw **unframed**, like the console does: they are usually two
/// or three grid units tall, and a border would spend two of the three terminal
/// rows on chrome and drop the content.
fn render_dashboard_text(markdown: &str, rect: Rect, frame: &mut Frame) {
    let inner = Rect {
        x: rect.x + 1,
        y: rect.y,
        width: rect.width.saturating_sub(2),
        height: rect.height,
    };
    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let lines: Vec<Line> = markdown
        .lines()
        .map(|raw| {
            let trimmed = raw.trim_end();
            let heading = trimmed.trim_start().starts_with('#');
            let text = flatten_md(trimmed);
            if heading {
                Line::from(Span::styled(
                    text,
                    Style::default().fg(theme::accent()).add_modifier(Modifier::BOLD),
                ))
            } else {
                Line::from(Span::styled(text, Style::default().fg(theme::text_primary())))
            }
        })
        .collect();

    frame.render_widget(
        Paragraph::new(lines).wrap(ratatui::widgets::Wrap { trim: false }),
        inner,
    );
}

/// Strip the markdown a text widget is written in down to plain text — heading
/// hashes and bold/italic runs, which would otherwise read as literal `**`.
fn flatten_md(line: &str) -> String {
    let s = line.trim_start_matches('#').trim_start();
    flatten_md_links(s).replace("**", "").replace("__", "")
}

/// Turn `[text](url)` into `text ↗`, and CloudWatch's own button extension —
/// `[button:Label](url)` / `[button:primary:Label](url)` — into `[ Label ↗ ]`.
///
/// The URL is dropped on purpose: nothing in this pane can follow a link, and a
/// full console URL is several times wider than the widget that holds it. The
/// dashboard's **Raw** detail section (and `e`) still has the untouched body.
fn flatten_md_links(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;

    while i < chars.len() {
        if chars[i] == '[' {
            // `[text](url)` — both brackets and both parens, or it's literal.
            if let Some(close) = chars[i + 1..].iter().position(|c| *c == ']') {
                let close = i + 1 + close;
                if chars.get(close + 1) == Some(&'(') {
                    if let Some(end) = chars[close + 2..].iter().position(|c| *c == ')') {
                        let text: String = chars[i + 1..close].iter().collect();
                        out.push_str(&render_md_link(&text));
                        i = close + 2 + end + 1;
                        continue;
                    }
                }
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn render_md_link(text: &str) -> String {
    match text.strip_prefix("button:") {
        Some(rest) => format!("[ {} ↗ ]", rest.strip_prefix("primary:").unwrap_or(rest)),
        None => format!("{} ↗", text),
    }
}

/// Shorten a series label to `max` columns, keeping both ends — sibling metrics
/// on one widget usually share a prefix (`pay-api Latency` /
/// `pay-api IntegrationLatency`), so truncating from the right would make every
/// legend entry read the same.
fn elide_mid(s: &str, max: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max {
        return s.to_string();
    }
    if max < 4 {
        return chars.into_iter().take(max).collect();
    }
    let head = (max - 1) / 2;
    let tail = max - 1 - head;
    let mut out: String = chars[..head].iter().collect();
    out.push('…');
    out.extend(&chars[chars.len() - tail..]);
    out
}

/// Block-character sparkline for a cell too small to hold a real chart.
fn spark_line(points: &[(f64, f64)], width: usize) -> String {
    const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let n = points.len();
    if n == 0 || width == 0 {
        return String::new();
    }
    let lo = points.iter().map(|(_, y)| *y).fold(f64::MAX, f64::min);
    let hi = points.iter().map(|(_, y)| *y).fold(f64::MIN, f64::max);
    let span = (hi - lo).max(f64::EPSILON);

    (0..width)
        .map(|i| {
            let start = i * n / width;
            let end = (((i + 1) * n) / width).max(start + 1).min(n);
            let mean =
                points[start..end].iter().map(|(_, y)| *y).sum::<f64>() / (end - start) as f64;
            let step = (((mean - lo) / span) * 7.0).round().clamp(0.0, 7.0) as usize;
            BARS[step]
        })
        .collect()
}

fn fmt_dash_value(v: f64) -> String {
    if v == 0.0 {
        "0".to_string()
    } else if v.abs() >= 1_000_000.0 {
        format!("{:.1}M", v / 1_000_000.0)
    } else if v.abs() >= 1_000.0 {
        format!("{:.1}k", v / 1_000.0)
    } else if v.abs() >= 10.0 {
        format!("{:.0}", v)
    } else {
        format!("{:.2}", v)
    }
}

/// One sparkline row per series, latest value on the right. Used for widgets the
/// dashboard author made small (a 3-unit-tall strip of KPIs is a common layout).
fn render_dashboard_sparkline(
    title: &str,
    series: &[ChartSeries],
    selected: bool,
    rect: Rect,
    frame: &mut Frame,
) {
    // Under 3 rows the borders would consume the whole cell — drop them and use
    // the single row for the data.
    let (inner, bordered) = if rect.height >= 3 {
        let block = Block::default()
            .title(Span::styled(
                dashboard_title(title, selected),
                Style::default().fg(theme::text_dim()),
            ))
            .borders(Borders::ALL)
            .border_style(dashboard_border(selected));
        let inner = block.inner(rect);
        frame.render_widget(block, rect);
        (inner, true)
    } else {
        (rect, false)
    };
    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let mut lines: Vec<Line> = Vec::new();
    if !bordered {
        lines.push(Line::from(Span::styled(
            title.to_string(),
            Style::default().fg(theme::text_dim()),
        )));
    }
    let rows = (inner.height as usize).saturating_sub(lines.len());
    for (_, points, color) in series.iter().take(rows) {
        let last = points.last().map(|(_, y)| *y).unwrap_or(0.0);
        let value = fmt_dash_value(last);
        let spark_w = inner.width.saturating_sub(value.chars().count() as u16 + 1) as usize;
        lines.push(Line::from(vec![
            Span::styled(spark_line(points, spark_w), Style::default().fg(*color)),
            Span::raw(" "),
            Span::styled(value, Style::default().fg(theme::text_primary())),
        ]));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

// ── EBS volume metrics overlay ────────────────────────────────────────────────

// ── Auto Scaling group metrics overlay ────────────────────────────────────────

/// Daily-spend chart for the selected cost row. The series is already loaded
/// with the row (no fetch), so this just charts `item.daily`.
pub fn render_cost_metrics_overlay(app: &App, area: Rect, frame: &mut Frame) {
    use crate::aws::services::cost::{fmt_money, CostLineItem};


    let item = app
        .get_selected_resource()
        .and_then(|r| r.as_any().downcast_ref::<CostLineItem>());
    let (name, points, first_label) = match item {
        Some(i) => (i.key.clone(), i.daily_points(), i.first_date_label()),
        None => ("Unknown".to_string(), vec![], String::new()),
    };

    let block = Block::default()
        .title(Line::from(vec![
            Span::styled(" Daily Spend — ", Style::default().fg(theme::text_dim())),
            Span::styled(
                name,
                Style::default()
                    .fg(theme::aws_orange())
                    .add_modifier(Modifier::BOLD),
            ),
        ]))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::border_dim()));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)])
        .split(inner);

    let footer = Line::from(vec![
        Span::styled("  Esc", Style::default().fg(crate::ui::theme::warning())),
        Span::styled("/", Style::default().fg(theme::text_dim())),
        Span::styled("m", Style::default().fg(crate::ui::theme::warning())),
        Span::styled("  close", Style::default().fg(theme::text_dim())),
    ]);
    frame.render_widget(Paragraph::new(footer), chunks[1]);

    if points.is_empty() {
        let msg = Paragraph::new(Line::from(vec![Span::styled(
            "  No daily data for this row.",
            Style::default().fg(theme::text_dim()),
        )]));
        frame.render_widget(msg, chunks[0]);
        return;
    }

    let x_max = (points.len().saturating_sub(1)).max(1) as f64;
    let y_peak = points.iter().map(|(_, y)| *y).fold(0.0_f64, f64::max).max(1.0);
    let y_max = y_peak * 1.2;

    let dataset = Dataset::default()
        .name("daily")
        .data(&points)
        .marker(symbols::Marker::Braille)
        .graph_type(GraphType::Line)
        .style(Style::default().fg(crate::ui::theme::success()));

    let chart = Chart::new(vec![dataset])
        .block(
            Block::default()
                .title(Span::styled(
                    " Daily unblended cost ",
                    Style::default()
                        .fg(crate::ui::theme::success())
                        .add_modifier(Modifier::BOLD),
                ))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(crate::ui::theme::text_dim())),
        )
        .x_axis(
            Axis::default().bounds([0.0, x_max]).labels(vec![
                Span::styled(first_label, Style::default().fg(theme::text_dim())),
                Span::styled("now", Style::default().fg(theme::text_dim())),
            ]),
        )
        .y_axis(
            Axis::default().bounds([0.0, y_max]).labels(vec![
                Span::styled("$0", Style::default().fg(theme::text_dim())),
                Span::styled(
                    format!("${}", fmt_money(y_max / 2.0)),
                    Style::default().fg(theme::text_dim()),
                ),
                Span::styled(
                    format!("${}", fmt_money(y_max)),
                    Style::default().fg(theme::text_dim()),
                ),
            ]),
        );

    frame.render_widget(chart, chunks[0]);
}

/// Line chart with an auto-scaled y-axis and adaptive numeric labels — for
/// rate/count metrics whose magnitude isn't known ahead of time (IOPS, MB/s,
/// queue length, instance counts). Unlike `render_count_chart` it doesn't round
/// labels to ints.
fn render_value_chart(
    title: &str,
    data: &[(f64, f64)],
    x_max: f64,
    start_label: &'static str,
    color: Color,
    area: Rect,
    frame: &mut Frame,
) {
    let y_peak = data.iter().map(|(_, y)| *y).fold(0.0_f64, f64::max).max(1.0);
    let y_max = y_peak * 1.2;

    let fmt = |v: f64| {
        if v >= 10.0 {
            format!("{:.0}", v)
        } else if v >= 1.0 {
            format!("{:.1}", v)
        } else {
            format!("{:.2}", v)
        }
    };
    let mid_label = fmt(y_max / 2.0);
    let top_label = fmt(y_max);

    let dataset = Dataset::default()
        .name(title)
        .data(data)
        .marker(symbols::Marker::Braille)
        .graph_type(GraphType::Line)
        .style(Style::default().fg(color));

    let marks = marker_lines(x_max, 0.0, y_max);
    let mut datasets = marker_datasets(&marks);
    datasets.push(dataset);
    let chart = Chart::new(datasets)
        .block(
            Block::default()
                .title(Span::styled(
                    format!(" {} ", title),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(crate::ui::theme::text_dim())),
        )
        .x_axis(
            Axis::default()
                .bounds([0.0, x_max])
                .labels(vec![
                    Span::styled(start_label, Style::default().fg(theme::text_dim())),
                    Span::styled("now", Style::default().fg(theme::text_dim())),
                ]),
        )
        .y_axis(
            Axis::default()
                .bounds([0.0, y_max])
                .labels(vec![
                    Span::styled("0", Style::default().fg(theme::text_dim())),
                    Span::styled(mid_label, Style::default().fg(theme::text_dim())),
                    Span::styled(top_label, Style::default().fg(theme::text_dim())),
                ]),
        );

    frame.render_widget(chart, area);
}


// ── U14 metrics-gap overlays (NAT GW / VPN / DX / ECR / GA / WorkSpaces / Cognito)





// ── Second metrics-gap batch (VPC endpoint / TGW / log group / EB rule /
//    R53 zone / resolver endpoint / CodeBuild) ─────────────────────────────────




#[cfg(test)]
mod tests {
    use super::*;
    use crate::aws::services::cloudwatch::{parse_dashboard_body, CwSeries};
    use std::collections::HashMap;

    fn widgets(body: &str) -> Vec<CwDashboardWidget> {
        parse_dashboard_body(body).1
    }

    /// Draw one value chart over a 1h window ending at `END`, with `events`
    /// as the marker context (None = no context at all).
    fn chart_buffer(events: Option<Vec<(i64, bool)>>) -> (ratatui::buffer::Buffer, Vec<usize>) {
        const END: i64 = 1_800_000_000;
        let data: Vec<(f64, f64)> = (0..=60).map(|m| ((m * 60) as f64, 10.0)).collect();
        let _scope = MarkerScope;
        if let Some(events) = events {
            MARKERS.with(|m| {
                *m.borrow_mut() = Some(MarkerCtx {
                    window_end: END,
                    events: events.into_iter().map(|(ts, failed)| Change { ts, failed, what: String::new() }).collect(),
                    drawn: Default::default(),
                    loading: false,
                    range: Default::default(),
                    claimed: false,
                    cut: None,
                    earliest: None,
                })
            });
        }
        let backend = ratatui::backend::TestBackend::new(80, 16);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal
            .draw(|f| render_value_chart("Requests", &data, 3600.0, "1h ago", Color::Green, f.area(), f))
            .unwrap();
        let drawn = MARKERS.with(|m| m.borrow().as_ref().map(|c| c.drawn.iter().copied().collect()).unwrap_or_default());
        (terminal.backend().buffer().clone(), drawn)
    }

    #[test]
    fn markers_draw_inside_the_window_and_leave_charts_alone_outside_it() {
        const END: i64 = 1_800_000_000;
        let (plain, _) = chart_buffer(None);
        // 25 minutes ago, a failed one 10 minutes ago, and one from yesterday.
        let (marked, drawn) = chart_buffer(Some(vec![(END - 600, true), (END - 1500, false), (END - 86_400, false)]));
        assert_eq!(drawn, vec![0, 1], "yesterday's event is outside a 1h chart");
        assert_ne!(plain, marked, "in-window markers draw");
        let error = theme::error();
        assert!(marked.content().iter().any(|c| c.fg == error), "a failed change is error-coloured");

        let (outside, drawn) = chart_buffer(Some(vec![(END - 86_400, false)]));
        assert!(drawn.is_empty());
        assert_eq!(plain, outside, "no events in the window: the chart is unchanged");
    }

    #[test]
    fn the_legend_counts_each_kind_of_change_once() {
        let change = |what: &str, failed: bool| Change { ts: 0, failed, what: what.to_string() };
        let ctx = |drawn: &[usize]| MarkerCtx {
            window_end: 0,
            events: vec![
                change("StopInstances", false),
                change("StopInstances", false),
                change("AttachRolePolicy", true),
                change("StopInstances", false),
                change("PutRolePolicy", false),
            ],
            drawn: drawn.iter().copied().collect(),
            loading: false,
            range: Default::default(),
            claimed: false,
            cut: None,
            earliest: None,
        };
        let text = |l: Line| l.spans.iter().map(|s| s.content.to_string()).collect::<String>();

        let wide = text(marker_legend(&ctx(&[0, 1, 2, 3]), 200));
        assert_eq!(wide, "4 changes  StopInstances ×3 · AttachRolePolicy");

        let narrow = text(marker_legend(&ctx(&[0, 1, 2, 3]), 40));
        assert!(narrow.contains("StopInstances ×3") && narrow.contains("+1 more"), "{narrow}");

        let none = text(marker_legend(&ctx(&[]), 120));
        assert_eq!(none, "no changes in this window (5 in the last 7d)");

        // The lookup stopped at its cap: a chart reaching back past that
        // point counts with a `+`, or says how far back it knows nothing.
        let capped = |drawn: &[usize], earliest: i64| MarkerCtx { cut: Some(1_000), earliest: Some(earliest), ..ctx(drawn) };
        assert!(text(marker_legend(&capped(&[0, 1, 2, 3], 500), 200)).starts_with("4+ changes  "));
        assert!(text(marker_legend(&capped(&[0, 1, 2, 3], 1_500), 200)).starts_with("4 changes  "), "window inside what was read");
        let quiet = text(marker_legend(&capped(&[], 500), 200));
        assert!(quiet.starts_with("no changes since ") && quiet.ends_with("(CloudTrail read no further)"), "{quiet}");
    }

    /// Side-by-side widgets must share a column edge. Computing each rect from
    /// its own width instead of the cumulative grid position leaves a ragged
    /// one-column gutter between halves at most pane widths.
    #[test]
    fn dashboard_columns_tile_without_gaps() {
        let w = widgets(
            r#"{"widgets": [
                {"type":"metric","x":0,"y":0,"width":12,"height":6},
                {"type":"metric","x":12,"y":0,"width":12,"height":6}
            ]}"#,
        );
        let area = Rect { x: 0, y: 0, width: 100, height: 40 };
        let left = dashboard_widget_rect(&w[0], area, 0).unwrap();
        let right = dashboard_widget_rect(&w[1], area, 0).unwrap();

        assert_eq!(left.x + left.width, right.x);
        assert_eq!(right.x + right.width, area.width);
        // 6 grid units of height → one chart-sized box, both the same.
        assert_eq!(left.height, right.height);
    }

    /// A dashboard is taller than any pane, so the grid scrolls. Widgets fully
    /// above the fold drop out; a partially-visible one is clipped, not dropped.
    #[test]
    fn dashboard_scroll_drops_only_what_is_fully_above() {
        let w = widgets(
            r#"{"widgets": [
                {"type":"metric","x":0,"y":0,"width":24,"height":6},
                {"type":"metric","x":0,"y":6,"width":24,"height":6},
                {"type":"metric","x":0,"y":12,"width":24,"height":6}
            ]}"#,
        );
        let area = Rect { x: 0, y: 0, width: 100, height: 12 };
        let first_bottom = dashboard_widget_rect(&w[0], area, 0).unwrap().height;

        // Scrolled exactly past the first widget: it's gone, the second is at
        // the top, the third is clipped by the viewport's bottom edge.
        assert!(dashboard_widget_rect(&w[0], area, first_bottom).is_none());
        let second = dashboard_widget_rect(&w[1], area, first_bottom).unwrap();
        assert_eq!(second.y, area.y);
        let third = dashboard_widget_rect(&w[2], area, first_bottom).unwrap();
        assert_eq!(third.y + third.height, area.y + area.height);
    }

    /// The scroll ceiling `App` clamps `j`/`G` against.
    #[test]
    fn grid_rows_span_the_lowest_widget() {
        let w = widgets(
            r#"{"widgets": [
                {"type":"metric","x":0,"y":0,"width":12,"height":6},
                {"type":"metric","x":12,"y":0,"width":12,"height":24}
            ]}"#,
        );
        assert_eq!(dashboard_grid_rows(&w), dash_row(24));
        assert_eq!(dashboard_grid_rows(&[]), 0);
    }

    fn data(widgets: Vec<CwDashboardWidget>, series: HashMap<String, Vec<CwSeries>>)
        -> CwDashboardMetricsData
    {
        CwDashboardMetricsData {
            time_range: crate::aws::services::ec2::MetricsTimeRange::SixHours,
            x_max: 21_600.0,
            widgets,
            series,
            alarms: HashMap::new(),
            skipped_regions: Vec::new(),
            math_error: None,
            capped: false,
        }
    }

    fn series(label: &str, ys: &[f64]) -> CwSeries {
        CwSeries {
            label: label.to_string(),
            points: ys.iter().enumerate().map(|(i, y)| (i as f64 * 60.0, *y)).collect(),
        }
    }

    /// A tile, gauge or bar shows one number per series, and which number
    /// depends on the stat: a `Sum` widget means "how many over the window"
    /// (totalled), anything else means "where is it now" (latest). Reading the
    /// latest datapoint of a Sum series reports one bucket as the whole day.
    #[test]
    fn single_values_sum_or_take_the_latest_by_stat() {
        let w = widgets(
            r#"{"widgets": [
                {"type":"metric","x":0,"y":0,"width":6,"height":3,
                 "properties":{"view":"singleValue","stat":"Sum",
                   "metrics":[["AWS/SQS","NumberOfMessagesSent","QueueName","q"]]}},
                {"type":"metric","x":6,"y":0,"width":6,"height":3,
                 "properties":{"view":"singleValue","stat":"Average",
                   "metrics":[["AWS/EC2","CPUUtilization","InstanceId","i-1"]]}}
            ]}"#,
        );
        let mut s = HashMap::new();
        s.insert("w0m0".to_string(), vec![series("sent", &[1.0, 2.0, 3.0, 4.0])]);
        s.insert("w1m0".to_string(), vec![series("cpu", &[10.0, 20.0, 90.0])]);
        let d = data(w, s);

        let sums = dashboard_widget_series(0, &d.widgets[0], &d);
        assert_eq!(sums[0].value(), 10.0);

        let latest = dashboard_widget_series(1, &d.widgets[1], &d);
        assert_eq!(latest[0].value(), 90.0);
    }

    /// Two things the series collector has to get right: a hidden line exists
    /// only to feed metric math and must never be drawn, and a SEARCH()
    /// expression returns many series under one query id — flattening them is
    /// the whole reason series are stored as a list.
    #[test]
    fn hidden_lines_are_skipped_and_search_results_flattened() {
        let w = widgets(
            r#"{"widgets": [
                {"type":"metric","x":0,"y":0,"width":12,"height":6,
                 "properties":{"metrics":[
                   ["AWS/Lambda","Errors","FunctionName","f",{"id":"m1","visible":false}],
                   [{"expression":"SEARCH('{AWS/Lambda,FunctionName} MetricName=\"Errors\"','Sum')","id":"e1"}]
                 ]}}
            ]}"#,
        );
        let mut s = HashMap::new();
        s.insert("w0_m1".to_string(), vec![series("hidden", &[1.0])]);
        s.insert(
            "w0_e1".to_string(),
            vec![series("f-one", &[1.0, 2.0]), series("f-two", &[3.0, 4.0])],
        );
        let d = data(w, s);

        let drawn = dashboard_widget_series(0, &d.widgets[0], &d);
        let labels: Vec<&str> = drawn.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(labels, vec!["f-one", "f-two"]);
    }

    /// A stacked widget's lines are cumulative — the top one is the total, not
    /// one series among several. Drawing them independently is a different
    /// (wrong) reading of the same numbers.
    #[test]
    fn stacked_series_accumulate_and_bail_out_when_unaligned() {
        let w = widgets(
            r#"{"widgets":[{"type":"metric","x":0,"y":0,"width":12,"height":6,
                 "properties":{"stacked":true,"metrics":[["A","one"],["A","two"]]}}]}"#,
        );
        let a = series("a", &[1.0, 2.0]);
        let b = series("b", &[10.0, 20.0]);
        let drawn = vec![
            DrawnSeries { label: "a".into(), points: &a.points, summed: false, right_axis: false },
            DrawnSeries { label: "b".into(), points: &b.points, summed: false, right_axis: false },
        ];
        let (plotted, _) = dashboard_plot_points(&w[0], &drawn);
        assert_eq!(plotted[0], a.points);
        assert_eq!(plotted[1], vec![(0.0, 11.0), (60.0, 22.0)]);

        // Differing lengths have no honest alignment — leave them unstacked
        // rather than pairing points that don't share a timestamp.
        let short = series("b", &[10.0]);
        let ragged = vec![
            DrawnSeries { label: "a".into(), points: &a.points, summed: false, right_axis: false },
            DrawnSeries { label: "b".into(), points: &short.points, summed: false, right_axis: false },
        ];
        let (plotted, _) = dashboard_plot_points(&w[0], &ragged);
        assert_eq!(plotted[1], short.points);
    }

    /// ratatui charts have one y-axis. A right-axis series left on the left
    /// scale draws flat along the bottom whenever the two differ by orders of
    /// magnitude — shape preserved, but invisible.
    #[test]
    fn right_axis_series_rescale_onto_the_left_range() {
        let w = widgets(
            r#"{"widgets":[{"type":"metric","x":0,"y":0,"width":12,"height":6,
                 "properties":{"yAxis":{"right":{"min":0,"max":1}},
                   "metrics":[["A","requests"],["A","latency",{"yAxis":"right"}]]}}]}"#,
        );
        let left = series("requests", &[0.0, 1000.0]);
        let right = series("latency", &[0.0, 0.5]);
        let drawn = vec![
            DrawnSeries { label: "r".into(), points: &left.points, summed: false, right_axis: false },
            DrawnSeries { label: "l".into(), points: &right.points, summed: false, right_axis: true },
        ];
        let (plotted, scale) = dashboard_plot_points(&w[0], &drawn);

        // Left series untouched; the right one is mapped 0-1 onto 0-1000, so
        // half its declared range lands at half the left axis's height.
        assert_eq!(plotted[0], left.points);
        assert_eq!(plotted[1][1].1, 500.0);
        // The scale the marked lines belong to is reported for the title.
        assert_eq!(scale, Some((0.0, 1.0)));
    }

    /// A widget with *only* right-axis series has nothing to rescale against —
    /// its own scale is the only one, so it must be left alone.
    #[test]
    fn an_all_right_axis_widget_is_not_rescaled() {
        let w = widgets(
            r#"{"widgets":[{"type":"metric","x":0,"y":0,"width":12,"height":6,
                 "properties":{"yAxis":{"right":{"min":0,"max":1}},
                   "metrics":[["A","latency",{"yAxis":"right"}]]}}]}"#,
        );
        let only = series("latency", &[0.0, 0.5]);
        let drawn = vec![DrawnSeries {
            label: "l".into(),
            points: &only.points,
            summed: false,
            right_axis: true,
        }];
        let (plotted, scale) = dashboard_plot_points(&w[0], &drawn);
        assert_eq!(plotted[0], only.points);
        assert_eq!(scale, None);
    }

    /// Text widgets are written in CloudWatch's markdown, which has a button
    /// extension on top of ordinary links. Unhandled, a widget renders its raw
    /// `[button:primary:Firewall Console](https://…)` source.
    #[test]
    fn markdown_links_and_buttons_flatten_to_readable_text() {
        assert_eq!(
            flatten_md("[button:primary:Firewall Console](https://console.aws.amazon.com/x)"),
            "[ Firewall Console ↗ ]"
        );
        assert_eq!(flatten_md("[button:Runbook](https://go/rb)"), "[ Runbook ↗ ]");
        assert_eq!(flatten_md("see [the docs](https://d) for more"), "see the docs ↗ for more");
        // Headings and bold still flatten, and links inside them too.
        assert_eq!(flatten_md("## **Payments** [x](y)"), "Payments x ↗");
        // Two links on one line both convert.
        assert_eq!(flatten_md("[a](1) and [b](2)"), "a ↗ and b ↗");

        // Anything that isn't a complete link is left exactly as written —
        // brackets are ordinary text in a dashboard's markdown.
        assert_eq!(flatten_md("an [unclosed link"), "an [unclosed link");
        assert_eq!(flatten_md("array[0] indexing"), "array[0] indexing");
        assert_eq!(flatten_md("[label] then (paren)"), "[label] then (paren)");
    }

    /// Sibling metrics on one widget usually share a prefix, so a label trimmed
    /// from the right makes every legend entry read the same.
    #[test]
    fn elision_keeps_both_ends_of_a_label() {
        assert_eq!(elide_mid("short", 10), "short");
        let e = elide_mid("pay-api IntegrationLatency", 12);
        assert_eq!(e.chars().count(), 12);
        assert!(e.starts_with("pay-a"));
        assert!(e.ends_with("atency"));
        // Degenerate widths truncate rather than panic on a slice boundary.
        assert_eq!(elide_mid("abcdef", 3).chars().count(), 3);
        assert_eq!(elide_mid("héllo wörld", 6).chars().count(), 6);
    }

    /// Sparklines resample to the cell width in both directions — a 7-day series
    /// squeezed into 10 columns, and a 5-point series stretched across 20.
    #[test]
    fn sparkline_fills_the_requested_width() {
        let rising: Vec<(f64, f64)> = (0..100).map(|i| (i as f64, i as f64)).collect();
        assert_eq!(spark_line(&rising, 10).chars().count(), 10);
        assert!(spark_line(&rising, 10).starts_with('▁'));
        assert!(spark_line(&rising, 10).ends_with('█'));

        let sparse = vec![(0.0, 1.0), (1.0, 2.0), (2.0, 3.0), (3.0, 4.0), (4.0, 5.0)];
        assert_eq!(spark_line(&sparse, 20).chars().count(), 20);

        // Degenerate inputs must not panic or divide by zero.
        assert_eq!(spark_line(&[], 10), "");
        assert_eq!(spark_line(&rising, 0), "");
        let flat = vec![(0.0, 4.0), (1.0, 4.0)];
        assert_eq!(spark_line(&flat, 2).chars().count(), 2);
    }
}
