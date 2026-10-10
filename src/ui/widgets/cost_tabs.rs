use crate::app::App;
use crate::aws::services::cost::{
    fmt_money, CostAnomaly, CostGroupBy, CostLineItem, CostPeriod, ANOMALY_LOOKBACK_DAYS,
};
use crate::ui::theme;
use crate::ui::widgets::subtab_bar::subtab_bar_spans;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

/// Sub-tab row for the Cost service, numbered in screen order like every
/// sub-tab strip: the groupings `1`–`4`, `5` Tag / `6` Category (whose chips
/// name the chosen key), the `7 Anomalies` view chip — every chip
/// individually clickable — and a right-aligned `Σ` total headline summed
/// from the loaded rows. The period isn't a tab: its chips sit on the Cost
/// detail pane's top border (`cost_period_title`, stepped with `[`/`]`). While Anomalies is active the
/// grouping chips show no active marker (they don't apply), and pressing one
/// returns to the spend view.
pub fn render_cost_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let g = app.cost_group_by;
    let spend = !app.cost_anomalies;
    let anomaly_tab = [('7', "Anomalies", app.cost_anomalies)];
    // Anomalies is always drawn whole; the grouping bar gets what's left.
    let tail_w = ANOMALY_GAP + bar_width(&anomaly_tab);
    let bar_area = Rect {
        x: area.x + 3,
        width: area.width.saturating_sub(3 + tail_w),
        ..area
    };

    // One bar for all six groupings, so `9` / `0` share its separators. Every
    // chip should be on screen, so a tight row shortens the labels (`Svc`,
    // `Acct`, `Usage`, `Cat`) before it resorts to scrolling around the
    // active grouping (`‹`/`›`, below ~80 columns).
    let groupings = [
        ('1', CostGroupBy::Service),
        ('2', CostGroupBy::LinkedAccount),
        ('3', CostGroupBy::Region),
        ('4', CostGroupBy::UsageType),
        ('5', CostGroupBy::Tag),
        ('6', CostGroupBy::CostCategory),
    ];
    let labels_for = |short: bool| -> Vec<String> {
        groupings
            .iter()
            .map(|(_, kind)| grouping_chip_label(*kind, app.cost_group_key_for(*kind).as_deref(), short))
            .collect()
    };
    let as_tabs = |labels: &[String]| -> Vec<(char, String, bool)> {
        groupings
            .iter()
            .zip(labels)
            .map(|((key, kind), label)| (*key, label.clone(), spend && g == *kind))
            .collect()
    };
    let mut labels = labels_for(false);
    if bar_width(&borrowed(&as_tabs(&labels))) > bar_area.width {
        labels = labels_for(true);
    }
    let owned = as_tabs(&labels);
    let tabs = borrowed(&owned);

    // A dim " By" prefix before the chips; the bar (and its click regions)
    // renders into the rect shifted past it.
    let mut spans: Vec<Span> = vec![Span::styled(" By", Style::default().fg(theme::text_dim()))];
    spans.extend(subtab_bar_spans(app, bar_area, &tabs));
    spans.push(Span::raw(" ".repeat(ANOMALY_GAP as usize)));

    // The Anomalies view chip (key 8) — its own bar so its click region
    // lands where it is drawn.
    let used = Line::from(spans.clone()).width() as u16;
    let anomaly_area = Rect {
        x: area.x + used,
        width: area.width.saturating_sub(used),
        ..area
    };
    spans.extend(subtab_bar_spans(app, anomaly_area, &anomaly_tab));

    // Σ total headline, right-aligned when it fits.
    let total = if app.cost_anomalies {
        anomaly_total_spans(app)
    } else {
        cost_total_spans(app)
    };
    if let Some(total) = total {
        let used = Line::from(spans.clone()).width() as u16;
        let total_w = Line::from(total.clone()).width() as u16;
        if used + total_w + 2 <= area.width {
            let pad = area.width - used - total_w - 1;
            spans.push(Span::raw(" ".repeat(pad as usize)));
            spans.extend(total);
        }
    }

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// The period chips for the right of the Cost detail pane's top border —
/// `[ MTD │ Last Mo │ 3 Mo ]`, the brackets naming the `[`/`]` keys that step
/// it. `pane` is that pane's outer rect, `title_w` the width its left-hand
/// title takes. Records each chip's click region where ratatui draws a
/// right-aligned title (flush against the top-right corner). None when the
/// pane is too narrow to fit them beside the title; `[`/`]` / `t` work
/// regardless. No active marker under Anomalies, which has no period.
pub fn cost_period_title(app: &App, pane: Rect, title_w: u16) -> Option<Line<'static>> {
    let spend = !app.cost_anomalies;
    let periods = [CostPeriod::Mtd, CostPeriod::LastMonth, CostPeriod::Last3Months];
    let key_style = Style::default().fg(theme::accent());
    let sep_style = Style::default().fg(theme::text_dim());

    // Build the spans first, noting each chip's offset, so the width (and
    // therefore the right-aligned start) is known before regions are recorded.
    let mut spans: Vec<Span<'static>> = vec![Span::raw(" "), Span::styled("[", key_style)];
    let mut chips: Vec<(u16, u16, CostPeriod)> = Vec::new();
    for (i, period) in periods.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("│", sep_style));
        }
        let text = format!(" {} ", period.label());
        let offset = Line::from(spans.clone()).width() as u16;
        chips.push((offset, text.chars().count() as u16, *period));
        let style = if spend && *period == app.cost_period {
            Style::default()
                .fg(Color::Black)
                .bg(theme::aws_orange())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme::text_muted())
        };
        spans.push(Span::styled(text, style));
    }
    spans.push(Span::styled("]", key_style));
    spans.push(Span::raw(" "));

    let w = Line::from(spans.clone()).width() as u16;
    // Two corners, the title, and a little rule between them.
    if pane.width < 2 + title_w + 4 + w {
        return None;
    }
    let x0 = pane.x + pane.width - 1 - w;
    for (offset, width, period) in chips {
        app.push_click_region(
            Rect { x: x0 + offset, y: pane.y, width, height: 1 },
            crate::app::ClickAction::CostPeriod(period),
        );
    }
    Some(Line::from(spans).right_aligned())
}

/// Gap between the grouping bar and the Anomalies chip.
const ANOMALY_GAP: u16 = 2;

/// Drawn width of a `subtab_bar_spans` bar that fits whole: a leading space,
/// then `key label` chips (`label + 3`) joined by `" │ "`.
fn bar_width(tabs: &[(char, &str, bool)]) -> u16 {
    1 + tabs.iter().map(|(_, l, _)| l.chars().count() as u16 + 3).sum::<u16>()
        + 3 * (tabs.len() as u16).saturating_sub(1)
}

/// A grouping chip's label: `Service`, `Usage Type`… or, short, `Svc`,
/// `Usage`…. The keyed groupings add the chosen key (`Tag:team`), ellipsised
/// so one long key can't crowd out the other chips.
fn grouping_chip_label(kind: CostGroupBy, key: Option<&str>, short: bool) -> String {
    let name = match (kind, short) {
        (_, false) => kind.label(),
        (CostGroupBy::Service, true) => "Svc",
        (CostGroupBy::LinkedAccount, true) => "Acct",
        (CostGroupBy::UsageType, true) => "Usage",
        (CostGroupBy::CostCategory, true) => "Cat",
        (_, true) => kind.label(),
    };
    match key {
        None => name.to_string(),
        Some(k) => {
            let max = if short { 8 } else { 16 };
            let k: String = if k.chars().count() > max {
                format!("{}…", k.chars().take(max - 1).collect::<String>())
            } else {
                k.to_string()
            };
            format!("{}:{}", name, k)
        }
    }
}

/// `(key, &label, active)` views of owned chip tuples, for `subtab_bar_spans`.
fn borrowed(tabs: &[(char, String, bool)]) -> Vec<(char, &str, bool)> {
    tabs.iter().map(|(k, l, a)| (*k, l.as_str(), *a)).collect()
}

/// `Σ $412.80 over · 3 ongoing` — the anomalies' summed impact over the
/// lookback window, plus how many are still open.
fn anomaly_total_spans(app: &App) -> Option<Vec<Span<'static>>> {
    if app.all_search_mode {
        return None;
    }
    let mut total = 0.0_f64;
    let mut ongoing = 0usize;
    let mut any = false;
    for r in &app.resources {
        if let Some(a) = r.as_any().downcast_ref::<CostAnomaly>() {
            any = true;
            total += a.total_impact;
            if a.is_ongoing() {
                ongoing += 1;
            }
        }
    }
    if !any {
        return None;
    }
    let mut spans = vec![
        Span::styled("Σ ", Style::default().fg(theme::text_dim())),
        Span::styled(
            format!("${}", fmt_money(total)),
            Style::default()
                .fg(theme::text_primary())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" over · {}d", ANOMALY_LOOKBACK_DAYS),
            Style::default().fg(theme::text_dim()),
        ),
    ];
    if ongoing > 0 {
        spans.push(Span::styled(
            format!("  {} ongoing", ongoing),
            Style::default().fg(theme::warning()),
        ));
    }
    Some(spans)
}

/// `Σ $1,234.56 MTD ↑8%` — grand total over every loaded row (the full
/// window, regardless of any fuzzy filter) with the aggregate trend vs the
/// comparable prior window. None while another service's rows are grafted in
/// (`@all` mode) or nothing has loaded.
fn cost_total_spans(app: &App) -> Option<Vec<Span<'static>>> {
    if app.all_search_mode {
        return None;
    }
    let mut current = 0.0_f64;
    let mut prior = 0.0_f64;
    let mut has_prior = false;
    let mut any = false;
    for r in &app.resources {
        if let Some(i) = r.as_any().downcast_ref::<CostLineItem>() {
            any = true;
            current += i.current;
            if i.has_prior {
                prior += i.prior;
                has_prior = true;
            }
        }
    }
    if !any {
        return None;
    }
    let mut spans = vec![
        Span::styled("Σ ", Style::default().fg(theme::text_dim())),
        Span::styled(
            format!("${}", fmt_money(current)),
            Style::default()
                .fg(theme::text_primary())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" {}", app.cost_period.label()),
            Style::default().fg(theme::text_dim()),
        ),
    ];
    if has_prior && prior > 0.0 {
        let pct = (current - prior) / prior * 100.0;
        if pct.abs() >= 0.5 {
            // Spend up reads red (attention), down green — the row-dot rule.
            let (arrow, color) = if pct >= 0.0 {
                ("↑", theme::error())
            } else {
                ("↓", theme::success())
            };
            spans.push(Span::styled(
                format!("  {}{:.0}%", arrow, pct.abs()),
                Style::default().fg(color),
            ));
        }
    }
    Some(spans)
}
