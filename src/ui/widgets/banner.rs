use crate::ui::theme;
use ratatui::{
    layout::{Alignment, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

pub(crate) const BANNER_ART: [&str; 6] = [
    "███╗   ██╗███████╗██████╗  ██████╗ ████████╗ ██████╗ ",
    "████╗  ██║██╔════╝██╔══██╗██╔═══██╗╚══██╔══╝██╔═══██╗",
    "██╔██╗ ██║█████╗  ██████╔╝██║   ██║   ██║   ██║   ██║",
    "██║╚██╗██║██╔══╝  ██╔══██╗██║   ██║   ██║   ██║   ██║",
    "██║ ╚████║███████╗██████╔╝╚██████╔╝   ██║   ╚██████╔╝",
    "╚═╝  ╚═══╝╚══════╝╚═════╝  ╚═════╝    ╚═╝    ╚═════╝ ",
];

pub fn render_banner(area: Rect, frame: &mut Frame) {
    let mut lines: Vec<Line> = vec![Line::raw("")];
    lines.extend(
        BANNER_ART
            .iter()
            .map(|row| Line::from(Span::styled(*row, Style::default().fg(theme::aws_orange())))),
    );
    lines.push(Line::from(Span::styled(
        "AWS Terminal Interface",
        Style::default()
            .fg(theme::text_dim())
            .add_modifier(Modifier::ITALIC),
    )));

    let paragraph = Paragraph::new(lines).alignment(Alignment::Center);
    frame.render_widget(paragraph, area);
}
