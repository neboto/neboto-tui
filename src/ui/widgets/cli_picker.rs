use crate::app::App;
use crate::aws::cli_actions::{CliPickerState, CliTier};
use crate::ui::theme;
use ratatui::{
    layout::{Alignment, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{
        block::{Position, Title},
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
    let full = frame.size();
    let width = full.width.saturating_sub(4).clamp(20, 90).min(full.width);
    let inner_width = width.saturating_sub(4) as usize;

    let list = list_lines(picker);
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

    let block = theme::popup_block(&format!("Copy CLI command · {}", picker.subject)).title(
        Title::from(Span::styled(
            " ↑↓ move · ⏎ copy · 1-9 pick · Esc close ",
            Style::default().fg(theme::text_dim()),
        ))
        .position(Position::Bottom)
        .alignment(Alignment::Center),
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
    lines.extend(list);
    lines.push(Line::styled(
        "─".repeat(inner.width as usize),
        Style::default().fg(theme::text_dim()),
    ));
    lines.extend(preview);
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

/// Tier headings + numbered rows.
fn list_lines(picker: &CliPickerState) -> Vec<Line<'static>> {
    let mut out = Vec::new();
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
    }
    out
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
