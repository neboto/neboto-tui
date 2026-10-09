use crate::app::{App, ClickAction};
use crate::aws::service::ServiceType;
use crate::ui::theme;
use crate::ui::widgets::banner::BANNER_ART;
use crossterm::event::KeyCode;
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

/// One-click starting points for a first session: the service strip lists
/// only visited services, so on the splash it's empty, and without these a
/// mouse user has nothing to click but `S`.
const POPULAR: [ServiceType; 8] = [
    ServiceType::EC2,
    ServiceType::S3,
    ServiceType::Lambda,
    ServiceType::IAM,
    ServiceType::VPC,
    ServiceType::RDS,
    ServiceType::ECS,
    ServiceType::CloudWatch,
];

/// The menu rows a click can press. `@ec2` and `/` need typing afterwards and
/// `q` would end the session on a stray click, so those stay keyboard-only.
fn menu_click_key(key: &str) -> Option<KeyCode> {
    match key {
        "S" | "R" | "P" | "?" => key.chars().next().map(KeyCode::Char),
        _ => None,
    }
}

/// Welcome / getting-started dashboard shown in the main content area when no
/// service is loaded yet. A centered logo over a left-aligned menu block —
/// loads nothing until the user picks a service (`@prefix`, `S`, or a click
/// on a popular service). The service chips and the `S`/`R`/`P`/`?` rows
/// record click regions as they draw.
pub fn render_splash(app: &App, area: Rect, frame: &mut Frame) {
    let logo_h = BANNER_ART.len() as u16; // 6
    let menu_h = MENU.len() as u16; // 7
    // logo + subtitle + gap + chips + gap + menu + gap + footer
    let total_h = logo_h + 1 + 1 + 1 + 1 + menu_h + 1 + 1;
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
            "Nothing loaded — pick a service to begin (click one, or press S)",
            Style::default()
                .fg(theme::text_dim())
                .add_modifier(Modifier::ITALIC),
        )))
        .alignment(Alignment::Center),
        Rect { x: area.x, y: top + logo_h, width: area.width, height: 1 },
    );

    // ── Popular services (centered chips, each a click target) ──
    let chips_y = top + logo_h + 2;
    let chip_w = |s: &ServiceType| s.prefix().chars().count() as u16;
    const GAP: u16 = 2;
    let chips_w: u16 =
        POPULAR.iter().map(chip_w).sum::<u16>() + GAP * (POPULAR.len() as u16 - 1);
    let mut x = area.x + area.width.saturating_sub(chips_w) / 2;
    let mut spans = Vec::new();
    for (i, service) in POPULAR.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw(" ".repeat(GAP as usize)));
            x += GAP;
        }
        let w = chip_w(service);
        if x + w <= area.x + area.width && chips_y < area.y + area.height {
            app.push_click_region(
                Rect { x, y: chips_y, width: w, height: 1 },
                ClickAction::Service(*service),
            );
        }
        spans.push(Span::styled(
            service.prefix().to_string(),
            Style::default()
                .fg(theme::aws_orange())
                .add_modifier(Modifier::BOLD),
        ));
        x += w;
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)),
        Rect {
            x: area.x + area.width.saturating_sub(chips_w) / 2,
            y: chips_y,
            width: chips_w.min(area.width),
            height: 1,
        }
        .intersection(area),
    );

    // ── Menu (centered block; label left, shortcut right) ──
    let menu_x = area.x + area.width.saturating_sub(MENU_W) / 2;
    let menu_y = chips_y + 2;
    for (i, (key, _)) in MENU.iter().enumerate() {
        let y = menu_y + i as u16;
        if let (Some(code), true) = (menu_click_key(key), y < area.y + area.height) {
            app.push_click_region(
                Rect { x: menu_x, y, width: MENU_W.min(area.width), height: 1 },
                ClickAction::Press(code),
            );
        }
    }
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
        Rect { x: menu_x, y: menu_y, width: MENU_W, height: menu_h }.intersection(area),
    );

    // ── Footer (centered): a newer release takes the tip's slot, and is a
    // click target like the service strip's `↑` chip ──
    let footer_y = menu_y + menu_h + 1;
    let (footer, footer_style) = match &app.update_available {
        Some(v) => (
            format!(
                "↑ neboto v{v} is out (you have v{}) — click for the upgrade command",
                crate::update_check::current_version()
            ),
            Style::default().fg(theme::accent()),
        ),
        None => (
            "Tip: set default_service in ~/.config/neboto/config.toml to skip this screen"
                .to_string(),
            Style::default()
                .fg(theme::text_dim())
                .add_modifier(Modifier::ITALIC),
        ),
    };
    if app.update_available.is_some() && footer_y < area.y + area.height {
        let w = (footer.chars().count() as u16).min(area.width);
        app.push_click_region(
            Rect { x: area.x + area.width.saturating_sub(w) / 2, y: footer_y, width: w, height: 1 },
            ClickAction::UpdateNotice,
        );
    }
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(footer, footer_style))).alignment(Alignment::Center),
        Rect { x: area.x, y: footer_y, width: area.width, height: 1 }.intersection(area),
    );
}
