use crate::ui::theme;
use crate::ui::widgets::banner::BANNER_ART;
use ratatui::{
    layout::{Alignment, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

/// Dashboard-style entries: (shortcut, label). Shortcut is shown right-aligned,
/// label left-aligned, the whole block horizontally centered (LazyVim style).
const MENU: [(&str, &str); 7] = [
    ("@ec2", "Load a service by prefix (try @s3, @vpc, @iam, @cw…)"),
    ("S", "Open the service picker"),
    ("R", "Switch AWS region"),
    ("P", "Switch AWS profile"),
    ("/", "Search within a service"),
    ("?", "Full keybinding help"),
    ("q", "Quit"),
];

const MENU_W: u16 = 60;

/// Welcome / getting-started dashboard shown in the main content area when no
/// service is loaded yet. A centered logo over a left-aligned menu block —
/// loads nothing until the user picks a service (`@prefix` or `S`).
pub fn render_splash(area: Rect, frame: &mut Frame) {
    let logo_h = BANNER_ART.len() as u16; // 6
    let menu_h = MENU.len() as u16; // 7
    // logo + subtitle + gap + menu + gap + footer
    let total_h = logo_h + 1 + 1 + menu_h + 1 + 1;
    let top = area.y + area.height.saturating_sub(total_h) / 2;

    // ── Logo (centered across the full width) ──
    let logo_lines: Vec<Line> = BANNER_ART
        .iter()
        .map(|row| Line::from(Span::styled(*row, Style::default().fg(theme::aws_orange()))))
        .collect();
    frame.render_widget(
        Paragraph::new(logo_lines).alignment(Alignment::Center),
        Rect { x: area.x, y: top, width: area.width, height: logo_h },
    );

    // ── Subtitle (centered) ──
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "Nothing loaded — pick a service to begin",
            Style::default()
                .fg(theme::text_dim())
                .add_modifier(Modifier::ITALIC),
        )))
        .alignment(Alignment::Center),
        Rect { x: area.x, y: top + logo_h, width: area.width, height: 1 },
    );

    // ── Menu (centered block; label left, shortcut right) ──
    let menu_x = area.x + area.width.saturating_sub(MENU_W) / 2;
    let menu_y = top + logo_h + 2;
    let menu_lines: Vec<Line> = MENU
        .iter()
        .map(|(key, label)| {
            let used = 2 + label.chars().count() + key.chars().count();
            let pad = (MENU_W as usize).saturating_sub(used);
            Line::from(vec![
                Span::styled(format!("  {}", label), Style::default().fg(crate::ui::theme::text_muted())),
                Span::raw(" ".repeat(pad)),
                Span::styled(
                    key.to_string(),
                    Style::default()
                        .fg(theme::aws_orange())
                        .add_modifier(Modifier::BOLD),
                ),
            ])
        })
        .collect();
    frame.render_widget(
        Paragraph::new(menu_lines).alignment(Alignment::Left),
        Rect { x: menu_x, y: menu_y, width: MENU_W, height: menu_h },
    );

    // ── Footer tip (centered) ──
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "Tip: set default_service in ~/.config/neboto/config.toml to skip this screen",
            Style::default()
                .fg(theme::text_dim())
                .add_modifier(Modifier::ITALIC),
        )))
        .alignment(Alignment::Center),
        Rect { x: area.x, y: menu_y + menu_h + 1, width: area.width, height: 1 },
    );
}
