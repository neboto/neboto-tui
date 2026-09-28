//! Network-access lens (`N`): the **effective** security-group rule set of
//! a resource, rendered **in the detail pane** (rich detail-pane view + `Z`
//! full-width, like the timeline). An instance / RDS / Lambda / ECS service
//! with three groups otherwise means three jumps and a mental merge; here
//! every group's rules land in one table, identical `(direction, protocol,
//! ports, source)` rules collapsed with the contributing groups listed.
//!
//! Groups come from the warm EC2 / VPC caches when present, else one
//! `DescribeSecurityGroups` for the missing ids (`Event::AccessLensLoaded`,
//! generation-guarded, off the LazyStore like the other lenses — `r`
//! refetches). Which groups a resource carries is
//! `Resource::security_group_ids()`.
//!
//! `t` cycles inbound → outbound → both; open-to-world sources
//! (`0.0.0.0/0`, `::/0`) render in the warning colour; `⏎` jumps to the
//! row's group (a referenced `sg-…` source wins over the contributing
//! group); `e` opens the whole table in `$EDITOR`.
//!
//! A source that is a load balancer's own security group is labelled
//! `⇠ ALB my-alb` (from the warm ELB cache — zero API): on an instance that
//! separates "traffic the LB forwards" from ports opened some other way. A
//! cold ELB cache says so in the header rather than guessing.

use crate::aws::services::ec2::SecurityGroup;
use crate::theme;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AccessDirection {
    #[default]
    Inbound,
    Outbound,
    Both,
}

impl AccessDirection {
    pub fn next(self) -> Self {
        match self {
            Self::Inbound => Self::Outbound,
            Self::Outbound => Self::Both,
            Self::Both => Self::Inbound,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Inbound => "inbound",
            Self::Outbound => "outbound",
            Self::Both => "inbound + outbound",
        }
    }
}

/// One merged rule: identical `(inbound, protocol, ports, source)` rules
/// from several groups collapse into one row listing all of them.
#[derive(Debug, Clone, PartialEq)]
pub struct AccessRow {
    pub inbound: bool,
    pub protocol: String,
    pub ports: String,
    pub source: String,
    pub source_label: Option<String>,
    /// Contributing groups, `(id, name)`, in attachment order.
    pub from: Vec<(String, String)>,
    pub descriptions: Vec<String>,
    /// Open to the world (`0.0.0.0/0` / `::/0`).
    pub world: bool,
    /// The source group belongs to these load balancers (`"ALB my-alb"`).
    pub lb_sources: Vec<String>,
}

impl AccessRow {
    pub fn source_display(&self) -> String {
        let mut s = match &self.source_label {
            Some(n) if !n.is_empty() => format!("{} ({})", self.source, n),
            _ => self.source.clone(),
        };
        if !self.lb_sources.is_empty() {
            s.push_str(" ⇠ ");
            s.push_str(&self.lb_sources.join(", "));
        }
        s
    }

    pub fn groups_display(&self) -> String {
        self.from
            .iter()
            .map(|(id, name)| {
                if name.is_empty() || name == id {
                    id.clone()
                } else {
                    format!("{} ({})", id, name)
                }
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Plain-text line for copy / export.
    pub fn text(&self) -> String {
        let mut s = format!(
            "{}  {:<5}  {:<11}  {}  ← {}",
            if self.inbound { "IN " } else { "OUT" },
            self.protocol,
            self.ports,
            self.source_display(),
            self.groups_display()
        );
        if !self.descriptions.is_empty() {
            s.push_str("  # ");
            s.push_str(&self.descriptions.join("; "));
        }
        s
    }

    /// The group `⏎` should jump to: a referenced group as the source, else
    /// the first contributing group.
    pub fn jump_group(&self) -> Option<String> {
        if self.source.starts_with("sg-") {
            return Some(self.source.clone());
        }
        self.from.first().map(|(id, _)| id.clone())
    }
}

/// Merge every group's rules into one deduplicated, sorted table.
pub fn merge_rules(groups: &[SecurityGroup]) -> Vec<AccessRow> {
    let mut rows: Vec<AccessRow> = Vec::new();
    for g in groups {
        for (inbound, rules) in [(true, &g.inbound_rules), (false, &g.outbound_rules)] {
            for r in rules {
                let existing = rows.iter_mut().find(|row| {
                    row.inbound == inbound
                        && row.protocol == r.protocol
                        && row.ports == r.ports
                        && row.source == r.source
                });
                match existing {
                    Some(row) => {
                        if !row.from.iter().any(|(id, _)| *id == g.group_id) {
                            row.from.push((g.group_id.clone(), g.group_name.clone()));
                        }
                        if let Some(d) = &r.description {
                            if !row.descriptions.contains(d) {
                                row.descriptions.push(d.clone());
                            }
                        }
                        if row.source_label.is_none() {
                            row.source_label = r.source_label.clone();
                        }
                    }
                    None => rows.push(AccessRow {
                        inbound,
                        protocol: r.protocol.clone(),
                        ports: r.ports.clone(),
                        source: r.source.clone(),
                        source_label: r.source_label.clone(),
                        from: vec![(g.group_id.clone(), g.group_name.clone())],
                        descriptions: r.description.iter().cloned().collect(),
                        world: r.source == "0.0.0.0/0" || r.source == "::/0",
                        lb_sources: Vec::new(),
                    }),
                }
            }
        }
    }
    // Inbound first; then by port (All → first), protocol, source — so the
    // table reads as a port map rather than as per-group blocks.
    rows.sort_by(|a, b| {
        b.inbound
            .cmp(&a.inbound)
            .then_with(|| port_key(&a.ports).cmp(&port_key(&b.ports)))
            .then_with(|| a.protocol.cmp(&b.protocol))
            .then_with(|| a.source.cmp(&b.source))
    });
    rows
}

/// Sort key for a port string: `All` first, then the range start.
fn port_key(ports: &str) -> (u8, u32) {
    if ports.eq_ignore_ascii_case("all") {
        return (0, 0);
    }
    let start = ports
        .split('-')
        .next()
        .and_then(|p| p.trim().parse::<u32>().ok())
        .unwrap_or(u32::MAX);
    (1, start)
}

#[derive(Debug, Clone, Default)]
pub struct AccessLensState {
    pub title: String,
    /// Groups the resource carries, deduped, attachment order.
    pub sg_ids: Vec<String>,
    /// Resolved groups (cache hits + fetched).
    pub groups: Vec<SecurityGroup>,
    /// All merged rows (both directions).
    pub all_rows: Vec<AccessRow>,
    /// `all_rows` narrowed by `direction`.
    pub rows: Vec<AccessRow>,
    pub direction: AccessDirection,
    pub loading: bool,
    pub error: Option<String>,
    pub selected: usize,
    pub scroll: usize,
    pub message: Option<String>,
    /// Security group id → the load balancers using it (`"ALB my-alb"`),
    /// from the warm ELB cache at open time.
    pub lb_by_sg: std::collections::HashMap<String, Vec<String>>,
    /// Whether the ELB cache was warm when the lens opened — a cold cache
    /// means an unlabelled `sg-` source may still be a load balancer's.
    pub elb_loaded: bool,
}

impl AccessLensState {
    pub fn open(title: String, sg_ids: Vec<String>, direction: AccessDirection) -> Self {
        Self {
            title,
            sg_ids,
            direction,
            ..Default::default()
        }
    }

    /// Ids not yet resolved to a group.
    pub fn missing(&self) -> Vec<String> {
        self.sg_ids
            .iter()
            .filter(|id| !self.groups.iter().any(|g| g.group_id == **id))
            .cloned()
            .collect()
    }

    /// Add resolved groups (ignoring ones already present) and rebuild,
    /// keeping `groups` in `sg_ids` order so `from` lists read consistently.
    pub fn add_groups(&mut self, incoming: Vec<SecurityGroup>) {
        for g in incoming {
            if !self.groups.iter().any(|x| x.group_id == g.group_id) {
                self.groups.push(g);
            }
        }
        let order = self.sg_ids.clone();
        self.groups.sort_by_key(|g| {
            order
                .iter()
                .position(|id| *id == g.group_id)
                .unwrap_or(usize::MAX)
        });
        self.all_rows = merge_rules(&self.groups);
        for row in &mut self.all_rows {
            let sg = row.source.split(['/', ' ']).next().unwrap_or("");
            if let Some(lbs) = self.lb_by_sg.get(sg) {
                row.lb_sources = lbs.clone();
            }
        }
        self.rebuild();
    }

    pub fn rebuild(&mut self) {
        self.rows = self
            .all_rows
            .iter()
            .filter(|r| match self.direction {
                AccessDirection::Inbound => r.inbound,
                AccessDirection::Outbound => !r.inbound,
                AccessDirection::Both => true,
            })
            .cloned()
            .collect();
        if self.rows.is_empty() {
            self.selected = 0;
            self.scroll = 0;
        } else if self.selected >= self.rows.len() {
            self.selected = self.rows.len() - 1;
        }
    }

    pub fn cycle_direction(&mut self) {
        self.direction = self.direction.next();
        self.rebuild();
    }

    pub fn selected_row(&self) -> Option<&AccessRow> {
        self.rows.get(self.selected)
    }

    /// Whole table as text (the `e` editor view / bulk copy).
    pub fn as_text(&self) -> String {
        let mut out = format!("Effective network access — {}\n", self.title);
        out.push_str("Groups: ");
        out.push_str(
            &self
                .groups
                .iter()
                .map(|g| format!("{} ({})", g.group_id, g.group_name))
                .collect::<Vec<_>>()
                .join(", "),
        );
        let missing = self.missing();
        if !missing.is_empty() {
            out.push_str(&format!("  [unresolved: {}]", missing.join(", ")));
        }
        out.push_str("\n\n");
        for r in &self.all_rows {
            out.push_str(&r.text());
            out.push('\n');
        }
        out
    }

    pub fn scroll_up(&mut self, n: usize) {
        self.selected = self.selected.saturating_sub(n);
    }

    pub fn scroll_down(&mut self, n: usize) {
        if !self.rows.is_empty() {
            self.selected = (self.selected + n).min(self.rows.len() - 1);
        }
    }

    pub fn goto_top(&mut self) {
        self.selected = 0;
    }

    pub fn goto_bottom(&mut self) {
        self.selected = self.rows.len().saturating_sub(1);
    }
}

pub fn render_access_lens(app: &crate::app::App, area: Rect, frame: &mut Frame) {
    let st = match &app.access_in_pane {
        Some(s) => s,
        None => return,
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::border_dim()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // header
            Constraint::Length(1), // group chips
            Constraint::Min(0),    // body
            Constraint::Length(1), // footer
        ])
        .split(inner);

    // ── Header ──
    let badge = Span::styled(
        " ⇄ ACCESS ",
        Style::default()
            .fg(Color::Black)
            .bg(theme::accent())
            .add_modifier(Modifier::BOLD),
    );
    let world = st.rows.iter().filter(|r| r.world).count();
    let mut header = vec![
        badge,
        Span::raw(" "),
        Span::styled(
            st.title.clone(),
            Style::default()
                .fg(theme::aws_orange())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(
                "  {} rule{}",
                st.rows.len(),
                if st.rows.len() == 1 { "" } else { "s" }
            ),
            Style::default().fg(theme::text_dim()),
        ),
        Span::styled(
            format!("  · {}", st.direction.label()),
            Style::default().fg(theme::accent()),
        ),
    ];
    if world > 0 {
        header.push(Span::styled(
            format!("  · {} open to world", world),
            Style::default().fg(theme::warning()),
        ));
    }
    let from_lb = st.rows.iter().filter(|r| !r.lb_sources.is_empty()).count();
    if from_lb > 0 {
        header.push(Span::styled(
            format!("  · {} from load balancer{}", from_lb, if from_lb == 1 { "" } else { "s" }),
            Style::default().fg(theme::success()),
        ));
    } else if !st.elb_loaded && st.rows.iter().any(|r| r.source.starts_with("sg-")) {
        header.push(Span::styled(
            "  · ELB not loaded — LB sources unlabelled",
            Style::default().fg(theme::text_dim()),
        ));
    }
    if st.loading {
        header.push(Span::styled(
            format!("  {} describing groups…", theme::spinner(app.tick_count)),
            Style::default().fg(theme::warning()),
        ));
    }
    if let Some(m) = &st.message {
        header.push(Span::styled(
            format!("   {}", m),
            Style::default().fg(theme::success()),
        ));
    }
    if let Some(e) = &st.error {
        header.push(Span::styled(
            format!("   ⚠ {}", e),
            Style::default().fg(theme::error()),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(header)), chunks[0]);

    // ── Group chips: id (name) in↓ out↑, unresolved ones flagged ──
    let mut chips: Vec<Span> = vec![Span::raw(" ")];
    for id in &st.sg_ids {
        match st.groups.iter().find(|g| g.group_id == *id) {
            Some(g) => {
                chips.push(Span::styled(
                    id.clone(),
                    Style::default().fg(theme::accent()),
                ));
                if !g.group_name.is_empty() && g.group_name != *id {
                    chips.push(Span::styled(
                        format!(" {}", g.group_name),
                        Style::default().fg(theme::text_primary()),
                    ));
                }
                chips.push(Span::styled(
                    format!(
                        " {}↓ {}↑   ",
                        g.inbound_rules.len(),
                        g.outbound_rules.len()
                    ),
                    Style::default().fg(theme::text_dim()),
                ));
            }
            None => {
                let style = if st.loading {
                    Style::default().fg(theme::text_dim())
                } else {
                    Style::default().fg(theme::error())
                };
                chips.push(Span::styled(
                    format!("{} {}   ", id, if st.loading { "…" } else { "?" }),
                    style,
                ));
            }
        }
    }
    frame.render_widget(Paragraph::new(Line::from(chips)), chunks[1]);

    // ── Body ──
    let viewport = chunks[2].height as usize;
    let top = if st.selected < st.scroll {
        st.selected
    } else if viewport > 0 && st.selected >= st.scroll + viewport {
        st.selected + 1 - viewport
    } else {
        st.scroll
    };

    let mut body: Vec<Line> = Vec::with_capacity(viewport);
    if st.rows.is_empty() {
        let empty = if st.loading {
            "  Describing security groups…"
        } else if st.groups.is_empty() {
            "  No groups resolved (see header)"
        } else {
            "  No rules in this direction — t cycles inbound / outbound / both"
        };
        body.push(Line::styled(empty, Style::default().fg(theme::text_dim())));
    } else {
        for (i, row) in st.rows.iter().enumerate().skip(top).take(viewport) {
            let selected = i == st.selected;
            let marker = if selected { "▸ " } else { "  " };
            let base = if selected {
                Style::default()
                    .fg(theme::text_primary())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme::text_primary())
            };
            let source_style = if row.world {
                base.fg(theme::warning())
            } else if !row.lb_sources.is_empty() {
                base.fg(theme::success())
            } else if row.source.starts_with("sg-") {
                base.fg(theme::accent())
            } else {
                base
            };
            let mut spans = vec![
                Span::styled(marker, Style::default().fg(theme::accent())),
                Span::styled(
                    if row.inbound { "↓ " } else { "↑ " },
                    Style::default().fg(if row.inbound {
                        theme::success()
                    } else {
                        theme::text_dim()
                    }),
                ),
                Span::styled(format!("{:<5} ", row.protocol), base),
                Span::styled(format!("{:<11} ", row.ports), base),
                Span::styled(
                    format!("{:<36} ", trim(&row.source_display(), 36)),
                    source_style,
                ),
                Span::styled(
                    format!("← {:<30} ", trim(&row.groups_display(), 30)),
                    base.fg(theme::text_dim()),
                ),
            ];
            if !row.descriptions.is_empty() {
                spans.push(Span::styled(
                    trim(&row.descriptions.join("; "), 40),
                    base.fg(theme::text_dim()).add_modifier(Modifier::ITALIC),
                ));
            }
            body.push(Line::from(spans));
        }
    }
    frame.render_widget(Paragraph::new(body), chunks[2]);

    // ── Footer ──
    let hint = |k: &'static str, d: &'static str| {
        vec![
            Span::styled(k, Style::default().fg(theme::warning())),
            Span::styled(d, Style::default().fg(theme::text_dim())),
        ]
    };
    let mut spans = vec![Span::raw("  ")];
    spans.extend(hint("j/k", " select  "));
    spans.extend(hint("⏎", " go to group  "));
    spans.extend(hint("t", " direction  "));
    spans.extend(hint("e", " open all  "));
    spans.extend(hint("y", " copy  "));
    spans.extend(hint("r", " refetch  "));
    spans.extend(hint("Z", " width  "));
    spans.extend(hint("Esc", " close"));
    frame.render_widget(Paragraph::new(Line::from(spans)), chunks[3]);
}

fn trim(s: &str, width: usize) -> String {
    if s.chars().count() <= width {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(width.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aws::services::ec2::SgRule;
    use std::collections::HashMap;

    fn rule(protocol: &str, ports: &str, source: &str, desc: Option<&str>) -> SgRule {
        SgRule {
            protocol: protocol.to_string(),
            ports: ports.to_string(),
            source: source.to_string(),
            source_label: None,
            description: desc.map(|d| d.to_string()),
        }
    }

    fn group(id: &str, name: &str, inbound: Vec<SgRule>, outbound: Vec<SgRule>) -> SecurityGroup {
        SecurityGroup {
            group_id: id.to_string(),
            group_name: name.to_string(),
            description: String::new(),
            vpc_id: None,
            inbound_rules: inbound,
            outbound_rules: outbound,
            tags: HashMap::new(),
        }
    }

    #[test]
    fn identical_rules_collapse_and_list_every_group() {
        let a = group(
            "sg-a",
            "web",
            vec![rule("TCP", "443", "0.0.0.0/0", Some("public https"))],
            vec![],
        );
        let b = group(
            "sg-b",
            "shared",
            vec![
                rule("TCP", "443", "0.0.0.0/0", None),
                rule("TCP", "22", "10.0.0.0/8", Some("bastion")),
            ],
            vec![rule("All", "All", "0.0.0.0/0", None)],
        );
        let rows = merge_rules(&[a, b]);
        // Inbound first, by port: 22 then 443; then the outbound row.
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].ports, "22");
        assert_eq!(rows[1].ports, "443");
        assert_eq!(rows[1].from.len(), 2);
        assert_eq!(rows[1].descriptions, vec!["public https".to_string()]);
        assert!(rows[1].world);
        assert!(!rows[0].world);
        assert!(!rows[2].inbound);
        assert_eq!(rows[2].ports, "All");
    }

    #[test]
    fn direction_filter_and_missing_ids() {
        let mut st = AccessLensState::open(
            "i-1".to_string(),
            vec!["sg-a".to_string(), "sg-b".to_string()],
            AccessDirection::Inbound,
        );
        assert_eq!(st.missing(), vec!["sg-a".to_string(), "sg-b".to_string()]);
        st.add_groups(vec![group(
            "sg-b",
            "x",
            vec![rule("TCP", "80", "10.0.0.0/16", None)],
            vec![rule("All", "All", "0.0.0.0/0", None)],
        )]);
        assert_eq!(st.missing(), vec!["sg-a".to_string()]);
        assert_eq!(st.rows.len(), 1);
        st.cycle_direction();
        assert_eq!(st.direction, AccessDirection::Outbound);
        assert_eq!(st.rows.len(), 1);
        assert!(!st.rows[0].inbound);
        st.cycle_direction();
        assert_eq!(st.rows.len(), 2);
    }

    #[test]
    fn jump_prefers_referenced_group_over_contributor() {
        let g = group(
            "sg-a",
            "app",
            vec![rule("TCP", "5432", "sg-db", None), rule("TCP", "80", "10.0.0.0/8", None)],
            vec![],
        );
        let rows = merge_rules(&[g]);
        assert_eq!(rows[0].jump_group().as_deref(), Some("sg-a"));
        assert_eq!(rows[1].jump_group().as_deref(), Some("sg-db"));
    }

    #[test]
    fn load_balancer_group_sources_are_labelled() {
        let mut st = AccessLensState::open(
            "i-1".to_string(),
            vec!["sg-a".to_string()],
            AccessDirection::Inbound,
        );
        st.lb_by_sg
            .insert("sg-alb".to_string(), vec!["ALB web".to_string()]);
        st.add_groups(vec![group(
            "sg-a",
            "app",
            vec![
                rule("TCP", "80", "sg-alb", None),
                rule("TCP", "22", "0.0.0.0/0", None),
            ],
            vec![],
        )]);
        let lb_row = st.rows.iter().find(|r| r.ports == "80").unwrap();
        assert_eq!(lb_row.lb_sources, vec!["ALB web".to_string()]);
        assert!(lb_row.source_display().ends_with("⇠ ALB web"));
        let ssh = st.rows.iter().find(|r| r.ports == "22").unwrap();
        assert!(ssh.lb_sources.is_empty());
        assert!(ssh.world);
    }
}
