use crate::app::App;
use crate::aws::service::ServiceType;
use crate::ui::theme;
use ratatui::{
    layout::{Alignment, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{
        block::{Position, Title},
        Block, BorderType, Borders, Clear, Paragraph,
    },
    Frame,
};

pub fn render_search_bar(app: &App, area: Rect, frame: &mut Frame) {
    let (border_color, title_style) = if app.search_active {
        (
            theme::warning(),
            Style::default()
                .fg(theme::warning())
                .add_modifier(Modifier::BOLD),
        )
    } else {
        (theme::border_dim(), Style::default().fg(crate::ui::theme::text_muted()))
    };

    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border_color))
        .title(Span::styled(" Search ", title_style));

    if app.search_active {
        block = block.title(
            Title::from(Line::from(vec![
                Span::styled(
                    " ⏎ ",
                    Style::default()
                        .fg(theme::accent())
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled("confirm ", Style::default().fg(theme::text_dim())),
                Span::styled(
                    "Esc ",
                    Style::default()
                        .fg(theme::accent())
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled("cancel ", Style::default().fg(theme::text_dim())),
            ]))
            .position(Position::Top)
            .alignment(Alignment::Right),
        );
    } else if app.show_loading_indicator() {
        // While a service loads, show a spinner + live count (and a progress bar
        // when the total is known) in the search bar's top-right corner.
        block = block.title(
            Title::from(loading_indicator(app))
                .position(Position::Top)
                .alignment(Alignment::Right),
        );
    }

    let prompt = Span::styled(
        "❯ ",
        Style::default()
            .fg(theme::accent())
            .add_modifier(Modifier::BOLD),
    );

    let content = if app.search_query.is_empty() && !app.search_active {
        Line::from(vec![
            prompt,
            Span::styled(
                "Press / to search, @service to switch, @all everywhere, tag:key=value to filter",
                Style::default()
                    .fg(theme::text_dim())
                    .add_modifier(Modifier::ITALIC),
            ),
        ])
    } else {
        // Highlight a leading @service prefix so service switches stand out
        let query = app.search_query.as_str();
        let mut spans = vec![prompt];
        if let Some(rest) = query.strip_prefix('@') {
            let (prefix, remainder) = match rest.find(' ') {
                Some(pos) => (&rest[..pos], &rest[pos..]),
                None => (rest, ""),
            };
            spans.push(Span::styled(
                format!("@{}", prefix),
                Style::default()
                    .fg(theme::aws_orange())
                    .add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::styled(
                remainder.to_string(),
                Style::default().fg(crate::ui::theme::text_primary()),
            ));
        } else {
            spans.push(Span::styled(
                query.to_string(),
                Style::default().fg(crate::ui::theme::text_primary()),
            ));
        }
        Line::from(spans)
    };

    frame.render_widget(Paragraph::new(content).block(block), area);

    // Show a real terminal cursor at the end of the query while typing
    if app.search_active {
        let cursor_x = area.x + 1 + 2 + app.search_query.chars().count() as u16;
        let cursor_x = cursor_x.min(area.x + area.width.saturating_sub(2));
        frame.set_cursor(cursor_x, area.y + 1);
    }
}

/// Build the search-bar loading indicator: `⠹ loading EC2 12` (indeterminate)
/// or `⠹ loading EC2 [████░░░░] 30/50` when the total count is known.
fn loading_indicator(app: &App) -> Line<'static> {
    let service = app
        .current_service
        .map(|s| s.name().to_string())
        .unwrap_or_else(|| "resources".to_string());

    let mut spans = vec![
        Span::styled(
            format!(" {} ", theme::spinner(app.tick_count)),
            Style::default()
                .fg(theme::warning())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("loading ", Style::default().fg(theme::text_dim())),
        Span::styled(service, Style::default().fg(crate::ui::theme::text_primary())),
    ];

    if let Some(progress) = &app.loading_progress {
        match progress.total_count {
            Some(total) if total > 0 => {
                const W: usize = 12;
                let filled = ((progress.loaded_count.min(total) * W) / total).min(W);
                let bar: String = "█".repeat(filled) + &"░".repeat(W - filled);
                spans.push(Span::styled(
                    format!(" {} ", bar),
                    Style::default().fg(theme::aws_orange()),
                ));
                spans.push(Span::styled(
                    format!("{}/{} ", progress.loaded_count, total),
                    Style::default().fg(theme::text_dim()),
                ));
            }
            _ => {
                spans.push(Span::styled(
                    format!(" {} ", progress.loaded_count),
                    Style::default().fg(theme::text_dim()),
                ));
            }
        }
    } else {
        spans.push(Span::raw(" "));
    }

    Line::from(spans)
}

/// Floating one-row completion dropdown shown directly below the search bar
/// while the user is typing a `@prefix` service name (before any space).
pub fn render_service_completions(app: &App, search_area: Rect, frame: &mut Frame) {
    if !app.search_active {
        return;
    }

    let query = app.search_query.as_str();
    if !query.starts_with('@') {
        return;
    }

    let partial = &query[1..];
    // Once there's a space the service prefix is committed — hide completions
    if partial.contains(' ') {
        return;
    }

    let partial_lower = partial.to_lowercase();

    let matches: Vec<ServiceType> = ServiceType::all()
        .into_iter()
        .filter(|s| s.prefix()[1..].starts_with(partial_lower.as_str()))
        .collect();

    if matches.is_empty() {
        return;
    }

    let popup_y = search_area.y + search_area.height;
    if popup_y >= frame.size().height {
        return;
    }

    let mut spans: Vec<Span> = vec![Span::raw("  ")];

    for (i, service) in matches.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("  "));
        }
        let style = if i == 0 {
            // First (best) match: slightly brighter so it reads as the default completion
            Style::default().fg(crate::ui::theme::text_primary())
        } else {
            Style::default().fg(theme::text_dim())
        };
        spans.push(Span::styled(service.prefix().to_string(), style));
    }

    spans.push(Span::styled(
        "   ⇥ complete",
        Style::default().fg(theme::text_dim()),
    ));

    let popup_rect = Rect {
        x: search_area.x,
        y: popup_y,
        width: search_area.width,
        height: 1,
    };

    frame.render_widget(Clear, popup_rect);
    frame.render_widget(Paragraph::new(Line::from(spans)), popup_rect);
}
