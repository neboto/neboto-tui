use crate::app::{App, PopupHits};
use crate::aws::cli_actions::{CliPickerState, CliTier};
use crate::ui::theme;
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{
        Clear, Paragraph, Wrap,
    },
    Frame,
};

/// `C` command picker: the commands for the selection grouped by tier
/// (Inspect / Connect / Change), and beneath them the **exact** text `⏎`
/// copies — region, profile and all — so a mutating command is read before
/// it's pasted. neboto never runs any of them; the header says so.
pub fn render_cli_picker(app: &App, frame: &mut Frame) {
    let Some(picker) = &app.cli_picker else {
        return;
    };
    let full = frame.area();
    let width = full.width.saturating_sub(4).clamp(20, 90).min(full.width);
    let inner_width = width.saturating_sub(4) as usize;

    let (list, list_rows) = list_lines(picker);
    let preview = preview_lines(picker);
    // Preview lines wrap; estimate their height so the popup fits them.
    let preview_rows: usize = preview
        .iter()
        .map(|l| l.width().max(1).div_ceil(inner_width.max(1)))
        .sum();
    let want = 2 /* header + blank */ + list.len() + 1 /* rule */ + preview_rows + 2 /* borders */;
    let height = (want as u16).min(full.height.saturating_sub(2)).max(8).min(full.height);
    let area = Rect {
        x: full.x + (full.width.saturating_sub(width)) / 2,
        y: full.y + (full.height.saturating_sub(height)) / 2,
        width,
        height,
    };
    frame.render_widget(Clear, area);

    let block = theme::popup_block(&format!("Copy CLI command · {}", picker.subject)).title_bottom(
        Line::from(Span::styled(
            " ↑↓ move · ⏎ copy · 1-9 pick · double-click copy · Esc close ",
            Style::default().fg(theme::text_dim()),
        ))
        .centered(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines = vec![
        Line::styled(
            " copies only — neboto never runs these",
            Style::default().fg(theme::text_dim()),
        ),
        Line::raw(""),
    ];
    let first_list_line = lines.len();
    lines.extend(list);
    record_hits(app, area, inner, &lines, first_list_line, &list_rows);
    lines.push(Line::styled(
        "─".repeat(inner.width as usize),
        Style::default().fg(theme::text_dim()),
    ));
    lines.extend(preview);
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

/// Record the popup and each command row's on-screen rect for the mouse
/// (`App::handle_popup_picker_mouse`). The body wraps, so a row's y is the sum
/// of the wrapped heights of the lines above it, the same arithmetic used to
/// size the popup. Rows past the bottom edge are clipped and get no target.
fn record_hits(
    app: &App,
    area: Rect,
    inner: Rect,
    lines: &[Line],
    first_list_line: usize,
    list_rows: &[Option<usize>],
) {
    let w = (inner.width as usize).max(1);
    let wrapped = |l: &Line| l.width().max(1).div_ceil(w) as u16;
    let mut y = inner.y;
    let mut rows = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let h = wrapped(line);
        let row = i
            .checked_sub(first_list_line)
            .and_then(|j| list_rows.get(j).copied().flatten());
        if let Some(idx) = row {
            if y + h <= inner.y + inner.height {
                rows.push((idx, Rect { x: inner.x, y, width: inner.width, height: h }));
            }
        }
        y = y.saturating_add(h);
    }
    *app.popup_hits.borrow_mut() = PopupHits { area: Some(area), rows };
}

/// Tier headings + numbered rows, and for each line the picker row it shows
/// (`None` for a tier heading).
fn list_lines(picker: &CliPickerState) -> (Vec<Line<'static>>, Vec<Option<usize>>) {
    let mut out = Vec::new();
    let mut rows = Vec::new();
    let mut tier: Option<CliTier> = None;
    for (i, row) in picker.rows.iter().enumerate() {
        if tier != Some(row.tier) {
            tier = Some(row.tier);
            let mut heading = vec![Span::styled(
                format!(" {}", row.tier.heading()),
                Style::default()
                    .fg(theme::heading())
                    .add_modifier(Modifier::BOLD),
            )];
            if row.tier == CliTier::Change {
                heading.push(Span::styled("  ⚠", Style::default().fg(theme::warning())));
            }
            out.push(Line::from(heading));
            rows.push(None);
        }
        let selected = i == picker.selected;
        let marker = if selected {
            Span::styled(" ▸ ", Style::default().fg(theme::aws_orange()))
        } else {
            Span::raw("   ")
        };
        let num = if i < 9 {
            format!("{}  ", i + 1)
        } else {
            "   ".to_string()
        };
        let label_style = if row.disabled.is_some() {
            Style::default()
                .fg(theme::text_dim())
                .add_modifier(Modifier::CROSSED_OUT)
        } else if row.tier == CliTier::Change {
            Style::default().fg(theme::warning())
        } else {
            Style::default().fg(theme::text_primary())
        };
        let label_style = if selected {
            label_style.add_modifier(Modifier::BOLD)
        } else {
            label_style
        };
        out.push(Line::from(vec![
            marker,
            Span::styled(num, Style::default().fg(theme::text_dim())),
            Span::styled(row.label.clone(), label_style),
        ]));
        rows.push(Some(i));
    }
    (out, rows)
}

/// The selected row's exact command, then its note / gate reason.
fn preview_lines(picker: &CliPickerState) -> Vec<Line<'static>> {
    let Some(row) = picker.selected_row() else {
        return Vec::new();
    };
    let cmd_style = if row.disabled.is_some() {
        Style::default().fg(theme::text_dim())
    } else {
        Style::default()
            .fg(theme::text_primary())
            .add_modifier(Modifier::BOLD)
    };
    let mut out = vec![Line::styled(format!(" {}", row.command), cmd_style)];
    if let Some(note) = &row.note {
        out.push(Line::styled(
            format!(" · {}", note),
            Style::default().fg(theme::text_dim()),
        ));
    }
    if let Some(reason) = &row.disabled {
        out.push(Line::styled(
            format!(" ✗ not copyable: {}", reason),
            Style::default().fg(theme::error()),
        ));
    }
    out
}
