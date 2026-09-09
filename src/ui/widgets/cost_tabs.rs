use crate::app::App;
use crate::aws::services::cost::{fmt_money, CostGroupBy, CostLineItem, CostPeriod};
use crate::ui::theme;
use crate::ui::widgets::subtab_bar::subtab_bar_spans;
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

/// Sub-tab row for the Cost service: a GroupBy selector (keys 1–4) on the
/// left, a Period selector (keys 5–7 direct, `t` cycles) beside it — both
/// rendered with the shared chip bar so every chip is individually clickable
/// — and a right-aligned `Σ` total headline summed from the loaded rows.
pub fn render_cost_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let g = app.cost_group_by;
    let tabs = [
        ('1', CostGroupBy::Service.label(), g == CostGroupBy::Service),
        ('2', CostGroupBy::LinkedAccount.label(), g == CostGroupBy::LinkedAccount),
        ('3', CostGroupBy::Region.label(), g == CostGroupBy::Region),
        ('4', CostGroupBy::UsageType.label(), g == CostGroupBy::UsageType),
    ];

    // A dim " By" prefix before the chips; the bar (and its click regions)
    // renders into the rect shifted past it.
    let mut spans: Vec<Span> = vec![Span::styled(" By", Style::default().fg(theme::text_dim()))];
    let bar_area = Rect {
        x: area.x + 3,
        width: area.width.saturating_sub(3),
        ..area
    };
    spans.extend(subtab_bar_spans(app, bar_area, &tabs));

    // Period chips: same chip language and per-chip click regions as the
    // group-by tabs. `t` still cycles; clicking the label does too.
    spans.push(Span::raw("  "));
    let label_x = area.x + Line::from(spans.clone()).width() as u16;
    spans.push(Span::styled("t", Style::default().fg(theme::accent())));
    spans.push(Span::styled(" Period", Style::default().fg(theme::text_dim())));
    app.push_click_region(
        Rect { x: label_x, y: area.y, width: 8, height: 1 },
        crate::app::ClickAction::Key('t'),
    );

    let p = app.cost_period;
    let period_tabs = [
        ('5', CostPeriod::Mtd.label(), p == CostPeriod::Mtd),
        ('6', CostPeriod::LastMonth.label(), p == CostPeriod::LastMonth),
        ('7', CostPeriod::Last3Months.label(), p == CostPeriod::Last3Months),
    ];
    let used = Line::from(spans.clone()).width() as u16;
    let period_area = Rect {
        x: area.x + used,
        width: area.width.saturating_sub(used),
        ..area
    };
    spans.extend(subtab_bar_spans(app, period_area, &period_tabs));

    // Σ total headline, right-aligned when it fits.
    if let Some(total) = cost_total_spans(app) {
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
