//! Central visual language for the app: one palette, one set of block/border
//! conventions, shared selection and state styling. Every widget pulls from
//! here so the UI stays coherent.
//!
//! The palette is runtime-selected (config `theme = "dark" | "light"`, plus
//! optional per-color overrides in `[theme_colors]`) and installed once at
//! startup via [`init_palette`]; every color is read through an accessor
//! (`theme::accent()`, `theme::warning()`, …). Widgets must not hardcode
//! `Color::White`/`Color::Yellow`-style values for text — route them through
//! the palette so the light preset stays readable.

use crate::aws::resource::ResourceState;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders};
use std::sync::OnceLock;

/// Every color the UI uses, resolved once at startup.
#[derive(Debug, Clone)]
pub struct Palette {
    /// AWS orange — branding accents (banner, active tab marker).
    pub aws_orange: Color,
    /// Primary accent for keys, prompts, and interactive hints.
    pub accent: Color,
    /// Border of the focused pane (lazygit-style green).
    pub border_focused: Color,
    /// Border of unfocused panes.
    pub border_dim: Color,
    /// Primary (brightest) text — names, values.
    pub text_primary: Color,
    /// Secondary text — descriptions, labels.
    pub text_muted: Color,
    /// Muted text: placeholders, separators, hints.
    pub text_dim: Color,
    /// Group/section headings (the magenta bold labels).
    pub heading: Color,
    pub error: Color,
    pub success: Color,
    pub warning: Color,
    /// Selection bar background (focused pane).
    pub selection_bg: Color,
    /// Muted selection background for unfocused panes.
    pub selection_bg_dim: Color,
    /// Selection bar text — paired with the selection backgrounds.
    pub selection_fg: Color,
}

/// `0xrrggbb` → `Color::Rgb` — keeps the preset tables readable.
const fn rgb(hex: u32) -> Color {
    Color::Rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// Preset names accepted by `theme = "…"` (hyphens/underscores optional).
pub const PRESET_NAMES: &[&str] = &[
    "dark",
    "light",
    "solarized-dark",
    "solarized-light",
    "gruvbox-dark",
    "gruvbox-light",
    "dracula",
    "nord",
    "catppuccin-mocha",
    "catppuccin-latte",
];

impl Palette {
    /// The classic look — the exact colors the app shipped with.
    pub fn dark() -> Self {
        Self {
            aws_orange: Color::Rgb(255, 153, 0),
            accent: Color::Cyan,
            border_focused: Color::Green,
            border_dim: Color::DarkGray,
            text_primary: Color::White,
            text_muted: Color::Gray,
            text_dim: Color::DarkGray,
            heading: Color::Magenta,
            error: Color::Red,
            success: Color::Green,
            warning: Color::Yellow,
            // Dark navy via RGB so contrast with white text is guaranteed
            // regardless of the terminal's ANSI palette.
            selection_bg: Color::Rgb(38, 68, 110),
            selection_bg_dim: Color::Rgb(55, 55, 55),
            selection_fg: Color::White,
        }
    }

    /// Darker, saturated colors that stay readable on a light terminal
    /// background (ANSI cyan/yellow/white are the usual casualties there).
    pub fn light() -> Self {
        Self {
            aws_orange: Color::Rgb(180, 95, 6),
            accent: Color::Rgb(0, 95, 175),
            border_focused: Color::Rgb(0, 135, 0),
            border_dim: Color::Gray,
            text_primary: Color::Black,
            text_muted: Color::DarkGray,
            text_dim: Color::Gray,
            heading: Color::Rgb(135, 0, 135),
            error: Color::Rgb(175, 0, 0),
            success: Color::Rgb(0, 135, 0),
            warning: Color::Rgb(160, 110, 0),
            selection_bg: Color::Rgb(190, 210, 240),
            selection_bg_dim: Color::Rgb(222, 222, 222),
            selection_fg: Color::Black,
        }
    }

    /// Solarized dark (Ethan Schoonover's canonical values).
    pub fn solarized_dark() -> Self {
        Self {
            aws_orange: rgb(0xcb4b16),
            accent: rgb(0x268bd2),         // blue
            border_focused: rgb(0x859900), // green
            border_dim: rgb(0x586e75),     // base01
            text_primary: rgb(0x93a1a1),   // base1 (emphasized)
            text_muted: rgb(0x839496),     // base0 (body)
            text_dim: rgb(0x586e75),       // base01
            heading: rgb(0xd33682),        // magenta
            error: rgb(0xdc322f),
            success: rgb(0x859900),
            warning: rgb(0xb58900),
            selection_bg: rgb(0x073642), // base02 (canonical highlight)
            selection_bg_dim: rgb(0x05303a),
            selection_fg: rgb(0xfdf6e3), // base3
        }
    }

    /// Solarized light.
    pub fn solarized_light() -> Self {
        Self {
            aws_orange: rgb(0xcb4b16),
            accent: rgb(0x268bd2),
            border_focused: rgb(0x859900),
            border_dim: rgb(0x93a1a1), // base1
            text_primary: rgb(0x586e75), // base01 (emphasized)
            text_muted: rgb(0x657b83),   // base00 (body)
            text_dim: rgb(0x93a1a1),     // base1
            heading: rgb(0xd33682),
            error: rgb(0xdc322f),
            success: rgb(0x859900),
            warning: rgb(0xb58900),
            selection_bg: rgb(0xeee8d5), // base2
            selection_bg_dim: rgb(0xf5efdc),
            selection_fg: rgb(0x073642), // base02
        }
    }

    /// Gruvbox dark (medium contrast).
    pub fn gruvbox_dark() -> Self {
        Self {
            aws_orange: rgb(0xfe8019),
            accent: rgb(0x83a598),         // blue
            border_focused: rgb(0xb8bb26), // green
            border_dim: rgb(0x665c54),     // bg3
            text_primary: rgb(0xebdbb2),   // fg
            text_muted: rgb(0xbdae93),     // fg3
            text_dim: rgb(0x928374),       // gray
            heading: rgb(0xd3869b),        // purple
            error: rgb(0xfb4934),
            success: rgb(0xb8bb26),
            warning: rgb(0xfabd2f),
            selection_bg: rgb(0x504945), // bg2
            selection_bg_dim: rgb(0x3c3836),
            selection_fg: rgb(0xebdbb2),
        }
    }

    /// Gruvbox light.
    pub fn gruvbox_light() -> Self {
        Self {
            aws_orange: rgb(0xaf3a03),
            accent: rgb(0x076678),
            border_focused: rgb(0x79740e),
            border_dim: rgb(0xa89984),
            text_primary: rgb(0x3c3836),
            text_muted: rgb(0x504945),
            text_dim: rgb(0x928374),
            heading: rgb(0x8f3f71),
            error: rgb(0x9d0006),
            success: rgb(0x79740e),
            warning: rgb(0xb57614),
            selection_bg: rgb(0xebdbb2),
            selection_bg_dim: rgb(0xf2e5bc),
            selection_fg: rgb(0x3c3836),
        }
    }

    /// Dracula.
    pub fn dracula() -> Self {
        Self {
            aws_orange: rgb(0xffb86c),
            accent: rgb(0x8be9fd),         // cyan
            border_focused: rgb(0x50fa7b), // green
            border_dim: rgb(0x6272a4),     // comment
            text_primary: rgb(0xf8f8f2),   // foreground
            text_muted: rgb(0xbdc3dd),
            text_dim: rgb(0x6272a4), // comment
            heading: rgb(0xff79c6),  // pink
            error: rgb(0xff5555),
            success: rgb(0x50fa7b),
            warning: rgb(0xf1fa8c),
            selection_bg: rgb(0x44475a), // current line
            selection_bg_dim: rgb(0x343746),
            selection_fg: rgb(0xf8f8f2),
        }
    }

    /// Nord.
    pub fn nord() -> Self {
        Self {
            aws_orange: rgb(0xd08770),     // nord12
            accent: rgb(0x88c0d0),         // nord8
            border_focused: rgb(0xa3be8c), // nord14
            border_dim: rgb(0x4c566a),     // nord3
            text_primary: rgb(0xeceff4),   // nord6
            text_muted: rgb(0xd8dee9),     // nord4
            text_dim: rgb(0x616e88),       // the de-facto Nord comment color
            heading: rgb(0xb48ead),        // nord15
            error: rgb(0xbf616a),          // nord11
            success: rgb(0xa3be8c),
            warning: rgb(0xebcb8b), // nord13
            selection_bg: rgb(0x434c5e), // nord2
            selection_bg_dim: rgb(0x3b4252),
            selection_fg: rgb(0xeceff4),
        }
    }

    /// Catppuccin Mocha (dark).
    pub fn catppuccin_mocha() -> Self {
        Self {
            aws_orange: rgb(0xfab387),     // peach
            accent: rgb(0x89b4fa),         // blue
            border_focused: rgb(0xa6e3a1), // green
            border_dim: rgb(0x585b70),     // surface2
            text_primary: rgb(0xcdd6f4),   // text
            text_muted: rgb(0xa6adc8),     // subtext0
            text_dim: rgb(0x6c7086),       // overlay0
            heading: rgb(0xcba6f7),        // mauve
            error: rgb(0xf38ba8),          // red
            success: rgb(0xa6e3a1),
            warning: rgb(0xf9e2af), // yellow
            selection_bg: rgb(0x45475a), // surface1
            selection_bg_dim: rgb(0x313244),
            selection_fg: rgb(0xcdd6f4),
        }
    }

    /// Catppuccin Latte (light).
    pub fn catppuccin_latte() -> Self {
        Self {
            aws_orange: rgb(0xfe640b),
            accent: rgb(0x1e66f5),
            border_focused: rgb(0x40a02b),
            border_dim: rgb(0x9ca0b0),
            text_primary: rgb(0x4c4f69),
            text_muted: rgb(0x6c6f85),
            text_dim: rgb(0x8c8fa1),
            heading: rgb(0x8839ef),
            error: rgb(0xd20f39),
            success: rgb(0x40a02b),
            warning: rgb(0xdf8e1d),
            selection_bg: rgb(0xccd0da),
            selection_bg_dim: rgb(0xdce0e8),
            selection_fg: rgb(0x4c4f69),
        }
    }

    /// Preset lookup, forgiving about separators: `solarized-dark`,
    /// `solarized_dark`, and `SolarizedDark` all resolve. Catppuccin flavors
    /// also answer to their bare names (`mocha`, `latte`).
    pub fn by_name(name: &str) -> Option<Self> {
        let normalized: String = name
            .to_ascii_lowercase()
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect();
        match normalized.as_str() {
            "dark" => Some(Self::dark()),
            "light" => Some(Self::light()),
            "solarizeddark" | "solarized" => Some(Self::solarized_dark()),
            "solarizedlight" => Some(Self::solarized_light()),
            "gruvboxdark" | "gruvbox" => Some(Self::gruvbox_dark()),
            "gruvboxlight" => Some(Self::gruvbox_light()),
            "dracula" => Some(Self::dracula()),
            "nord" => Some(Self::nord()),
            "catppuccinmocha" | "mocha" | "catppuccin" => Some(Self::catppuccin_mocha()),
            "catppuccinlatte" | "latte" => Some(Self::catppuccin_latte()),
            _ => None,
        }
    }

    /// Apply one `[theme_colors]` override by field name. Returns false for
    /// an unknown key so the caller can warn instead of silently dropping it.
    pub fn set_by_name(&mut self, key: &str, color: Color) -> bool {
        match key {
            "aws_orange" | "orange" => self.aws_orange = color,
            "accent" => self.accent = color,
            "border_focused" => self.border_focused = color,
            "border_dim" => self.border_dim = color,
            "text_primary" => self.text_primary = color,
            "text_muted" => self.text_muted = color,
            "text_dim" => self.text_dim = color,
            "heading" => self.heading = color,
            "error" => self.error = color,
            "success" => self.success = color,
            "warning" => self.warning = color,
            "selection_bg" => self.selection_bg = color,
            "selection_bg_dim" => self.selection_bg_dim = color,
            "selection_fg" => self.selection_fg = color,
            _ => return false,
        }
        true
    }

    /// Build the palette from config: preset by name (default dark), then
    /// per-color overrides. Problems come back as warnings — a typo'd color
    /// must never take the whole config down.
    pub fn from_config(
        theme: Option<&str>,
        overrides: Option<&std::collections::HashMap<String, String>>,
    ) -> (Self, Vec<String>) {
        let mut warnings = Vec::new();
        let mut palette = match theme {
            Some(name) => Self::by_name(name).unwrap_or_else(|| {
                warnings.push(format!(
                    "unknown theme \"{}\" — using dark (presets: {})",
                    name,
                    PRESET_NAMES.join(", ")
                ));
                Self::dark()
            }),
            None => Self::dark(),
        };
        if let Some(map) = overrides {
            // Sorted so repeated startups warn in a stable order.
            let mut entries: Vec<_> = map.iter().collect();
            entries.sort();
            for (key, value) in entries {
                match parse_color(value) {
                    Some(color) => {
                        if !palette.set_by_name(key, color) {
                            warnings.push(format!("unknown theme_colors key \"{}\"", key));
                        }
                    }
                    None => warnings.push(format!(
                        "theme_colors.{}: can't parse \"{}\" (use #rrggbb or an ANSI name)",
                        key, value
                    )),
                }
            }
        }
        (palette, warnings)
    }
}

static PALETTE: OnceLock<Palette> = OnceLock::new();

/// Install the palette (once, at startup, before the first draw). Later calls
/// are no-ops — the accessors have already handed the colors out.
pub fn init_palette(palette: Palette) {
    let _ = PALETTE.set(palette);
}

fn palette() -> &'static Palette {
    PALETTE.get_or_init(Palette::dark)
}

pub fn aws_orange() -> Color {
    palette().aws_orange
}
pub fn accent() -> Color {
    palette().accent
}
pub fn border_focused() -> Color {
    palette().border_focused
}
pub fn border_dim() -> Color {
    palette().border_dim
}
pub fn text_primary() -> Color {
    palette().text_primary
}
pub fn text_muted() -> Color {
    palette().text_muted
}
pub fn text_dim() -> Color {
    palette().text_dim
}
pub fn heading() -> Color {
    palette().heading
}
pub fn error() -> Color {
    palette().error
}
pub fn success() -> Color {
    palette().success
}
pub fn warning() -> Color {
    palette().warning
}

/// `#rrggbb` hex or an ANSI color name (the ratatui `Color` names,
/// case-insensitive; `gray`/`grey` both accepted).
pub fn parse_color(s: &str) -> Option<Color> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix('#') {
        if hex.len() == 6 && hex.is_ascii() {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            return Some(Color::Rgb(r, g, b));
        }
        return None;
    }
    match s.to_ascii_lowercase().as_str() {
        "black" => Some(Color::Black),
        "red" => Some(Color::Red),
        "green" => Some(Color::Green),
        "yellow" => Some(Color::Yellow),
        "blue" => Some(Color::Blue),
        "magenta" => Some(Color::Magenta),
        "cyan" => Some(Color::Cyan),
        "gray" | "grey" => Some(Color::Gray),
        "darkgray" | "darkgrey" => Some(Color::DarkGray),
        "lightred" => Some(Color::LightRed),
        "lightgreen" => Some(Color::LightGreen),
        "lightyellow" => Some(Color::LightYellow),
        "lightblue" => Some(Color::LightBlue),
        "lightmagenta" => Some(Color::LightMagenta),
        "lightcyan" => Some(Color::LightCyan),
        "white" => Some(Color::White),
        _ => None,
    }
}

/// Braille spinner frames, advanced by the app tick (250ms).
const SPINNER_FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub fn spinner(tick: u64) -> &'static str {
    SPINNER_FRAMES[(tick % SPINNER_FRAMES.len() as u64) as usize]
}

/// Standard pane block: rounded borders, focus-aware border + title styling.
pub fn pane_block(title: &str, focused: bool) -> Block<'static> {
    let (border_style, title_style) = if focused {
        (
            Style::default().fg(border_focused()),
            Style::default()
                .fg(border_focused())
                .add_modifier(Modifier::BOLD),
        )
    } else {
        (
            Style::default().fg(border_dim()),
            Style::default().fg(text_muted()),
        )
    };

    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style)
        .title(Span::styled(format!(" {} ", title), title_style))
}

/// Block for modal popups (help, region selector, jump list).
pub fn popup_block(title: &str) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(accent()))
        .title(Span::styled(
            format!(" {} ", title),
            Style::default().fg(accent()).add_modifier(Modifier::BOLD),
        ))
        .title_alignment(ratatui::layout::Alignment::Center)
}

/// Selection bar style; bright in the focused pane, muted elsewhere.
/// Sets both bg AND fg so colored spans underneath can't clash with the bar.
pub fn selection_style(focused: bool) -> Style {
    let p = palette();
    if focused {
        Style::default()
            .bg(p.selection_bg)
            .fg(p.selection_fg)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().bg(p.selection_bg_dim).fg(p.selection_fg)
    }
}

/// Indicator glyph + color for a resource state.
pub fn state_indicator(state: &ResourceState) -> (&'static str, Color) {
    match state {
        ResourceState::Running | ResourceState::Available => ("●", success()),
        ResourceState::Stopped | ResourceState::Unavailable => ("●", error()),
        ResourceState::Pending | ResourceState::Creating => ("◐", warning()),
        ResourceState::Deleting => ("◑", error()),
        ResourceState::Terminated => ("✗", text_dim()),
        ResourceState::Unknown(_) => ("○", text_dim()),
    }
}

/// A `key desc` pair for hint lines, e.g. `j/k navigate`.
pub fn hint(key: &str, desc: &str) -> Vec<Span<'static>> {
    vec![
        Span::styled(
            key.to_string(),
            Style::default().fg(accent()).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!(" {}", desc), Style::default().fg(text_muted())),
    ]
}

/// Join several hints into one line with dim separators.
pub fn hint_line(hints: &[(&str, &str)]) -> Line<'static> {
    let mut spans = Vec::new();
    for (i, (key, desc)) in hints.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("  ·  ", Style::default().fg(text_dim())));
        }
        spans.extend(hint(key, desc));
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn by_name_resolves_every_advertised_preset_and_forgives_separators() {
        for name in PRESET_NAMES {
            assert!(Palette::by_name(name).is_some(), "preset {} missing", name);
        }
        assert!(Palette::by_name("Solarized_Dark").is_some());
        assert!(Palette::by_name("CATPPUCCIN-MOCHA").is_some());
        assert!(Palette::by_name("mocha").is_some());
        assert!(Palette::by_name("latte").is_some());
        assert!(Palette::by_name("tokyonight").is_none());
    }

    #[test]
    fn parse_color_hex_and_names() {
        assert_eq!(parse_color("#ff9900"), Some(Color::Rgb(255, 153, 0)));
        assert_eq!(parse_color("  #FF9900 "), Some(Color::Rgb(255, 153, 0)));
        assert_eq!(parse_color("cyan"), Some(Color::Cyan));
        assert_eq!(parse_color("Grey"), Some(Color::Gray));
        assert_eq!(parse_color("#ff99"), None);
        assert_eq!(parse_color("#ff990g"), None);
        assert_eq!(parse_color("mauve"), None);
    }

    #[test]
    fn from_config_applies_preset_and_overrides_with_warnings() {
        let mut overrides = std::collections::HashMap::new();
        overrides.insert("accent".to_string(), "#00afff".to_string());
        overrides.insert("bogus_key".to_string(), "red".to_string());
        overrides.insert("warning".to_string(), "not-a-color".to_string());
        let (p, warnings) = Palette::from_config(Some("light"), Some(&overrides));
        assert_eq!(p.accent, Color::Rgb(0, 175, 255));
        assert_eq!(p.text_primary, Color::Black); // light preset survived
        assert_eq!(p.warning, Palette::light().warning); // bad value ignored
        assert_eq!(warnings.len(), 2);

        let (p, warnings) = Palette::from_config(Some("tokyonight"), None);
        assert_eq!(p.accent, Palette::dark().accent);
        assert_eq!(warnings.len(), 1);
    }
}
