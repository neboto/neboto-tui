//! Network-access lens (`N`): what can reach a resource and what it can
//! reach, rendered **in the detail pane** (rich detail-pane view + `Z`
//! full-width, like the timeline), as the layers AWS checks in turn.
//!
//! ```text
//! ↓ Inbound · NACL, then security groups: both must allow
//!   ① NACL acl-… · subnet-… · first match wins     (rule order, as AWS reads it)
//!   ② Security groups · any rule allows            (every group merged)
//! ↑ Outbound · security groups, then NACL
//! ```
//!
//! **Security groups** only allow, and a resource's groups add up, so their
//! rules are one table: identical `(direction, protocol, ports, source)`
//! rules collapse with the contributing groups listed, sorted by port.
//! Groups come from the warm EC2 / VPC caches when present, else one
//! `DescribeSecurityGroups` for the missing ids (`Event::AccessLensLoaded`,
//! generation-guarded, off the LazyStore like the other lenses — `r`
//! refetches). Which groups a resource carries is
//! `Resource::security_group_ids()`.
//!
//! **Subnet NACLs** (#246) come first inbound and last outbound. The
//! resource's subnets come from `Resource::references()` (`Subnet` rows),
//! their ACLs from the warm VPC cache or one `DescribeNetworkAcls` filtered
//! by `association.subnet-id`. NACLs are first-match, so they keep their
//! rule order (not the port sort), an entry an earlier one already covers
//! is marked shadowed, and an ACL whose first rule each way allows
//! everything is one `allow-all` line.
//!
//! **Each security group rule is run through the NACL** (`nacl_verdict`):
//! a rule the NACL stops reads `⊘ NACL #90 denies`, one it stops for part
//! of the traffic `◐ NACL #* blocks some`. NACLs keep no state, so a rule's
//! **replies** (ephemeral ports 1024-65535 back to the peer) are checked
//! too. A rule the NACL lets through says nothing. A source that is a
//! group or prefix list has no address here, so only a `/0` entry decides
//! it. Computed from the rules, not AWS's own reachability analysis: route
//! tables and Network Firewall aren't evaluated.
//!
//! `t` cycles inbound → outbound → both; open-to-world sources
//! (`0.0.0.0/0`, `::/0`) render in the warning colour; `⏎` jumps to the
//! row's group (a referenced `sg-…` source wins over the contributing
//! group) or ACL; `e` opens the whole view in `$EDITOR`.
//!
//! A source that is a load balancer's own security group is labelled
//! `⇠ ALB my-alb` (from the warm ELB cache — zero API): on an instance that
//! separates "traffic the LB forwards" from ports opened some other way. A
//! cold ELB cache says so in the header rather than guessing.

use crate::aws::services::ec2::SecurityGroup;
use crate::aws::services::vpc::{AclEntry, NetworkAcl};
use crate::theme;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
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

    fn includes(self, inbound: bool) -> bool {
        match self {
            Self::Inbound => inbound,
            Self::Outbound => !inbound,
            Self::Both => true,
        }
    }
}

/// How a NACL treats one flow, reading its entries in rule order up to the
/// first that covers the flow whole.
#[derive(Debug, Clone, PartialEq)]
pub enum NaclVerdict {
    Allowed,
    /// All of it meets a deny first — `rule` is that deny.
    Blocked { acl: String, rule: String },
    /// Some of it does — `rule` is the first deny on the way.
    Partly { acl: String, rule: String },
}

impl NaclVerdict {
    fn is_allowed(&self) -> bool {
        matches!(self, Self::Allowed)
    }

    /// `⊘ NACL #90 denies` / `◐ NACL #* blocks some`; the ACL is named only
    /// when the resource sits behind more than one.
    fn mark(&self, name_acl: bool) -> Option<String> {
        let (acl, rule, what, sign) = match self {
            Self::Allowed => return None,
            Self::Blocked { acl, rule } => (acl, rule, "denies", "⊘"),
            Self::Partly { acl, rule } => (acl, rule, "blocks some", "◐"),
        };
        Some(if name_acl {
            format!("{sign} NACL {acl} #{rule} {what}")
        } else {
            format!("{sign} NACL #{rule} {what}")
        })
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
    /// The subnet NACL's verdict on this rule's traffic (`None` until the
    /// ACLs are resolved).
    pub nacl: Option<NaclVerdict>,
    /// …and on its replies, which a stateless NACL must allow separately.
    pub reply: Option<NaclVerdict>,
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

    /// The one NACL fact worth the row's room: the rule's own traffic if a
    /// NACL stops any of it, else its replies'. Nothing when both get
    /// through. The bool says whether all of it is stopped.
    pub fn nacl_mark(&self, name_acl: bool) -> Option<(String, bool)> {
        if let Some(v) = self.nacl.as_ref().filter(|v| !v.is_allowed()) {
            return v.mark(name_acl).map(|m| (m, matches!(v, NaclVerdict::Blocked { .. })));
        }
        let v = self.reply.as_ref().filter(|v| !v.is_allowed())?;
        let mark = v.mark(name_acl)?;
        // "⊘ NACL #* denies" → "⊘ replies: NACL #* denies"
        let (sign, rest) = mark.split_once(' ')?;
        Some((format!("{sign} replies: {rest}"), matches!(v, NaclVerdict::Blocked { .. })))
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
        if let Some((mark, _)) = self.nacl_mark(true) {
            s.push_str("  ");
            s.push_str(&mark);
        }
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

/// One subnet-NACL entry. Rows keep the ACL's rule order: NACLs are
/// evaluated first match wins, so a port sort would misstate them.
#[derive(Debug, Clone, PartialEq)]
pub struct NaclRow {
    pub inbound: bool,
    pub acl_id: String,
    /// `100`, or `*` for the catch-all deny AWS adds to every ACL.
    pub rule: String,
    pub allow: bool,
    pub protocol: String,
    pub ports: String,
    pub cidr: String,
    /// The resource's subnets this ACL covers.
    pub subnets: Vec<String>,
    /// An earlier entry covers everything this one matches, so it never
    /// decides anything (`#100`).
    pub shadowed_by: Option<String>,
}

/// The rule numbers AWS gives the implicit catch-all entries (IPv4, IPv6).
const NACL_CATCH_ALL: i32 = 32767;

fn rule_label(n: i32) -> String {
    if n >= NACL_CATCH_ALL {
        "*".to_string()
    } else {
        n.to_string()
    }
}

impl NaclRow {
    pub fn acl_display(&self) -> String {
        format!("{} #{} {}", self.acl_id, self.rule, if self.allow { "allow" } else { "deny" })
    }

    pub fn text(&self) -> String {
        let mut s = format!(
            "{}  {:<5}  {:<11}  {}  ← NACL {}  [{}]",
            if self.inbound { "IN " } else { "OUT" },
            self.protocol,
            self.ports,
            self.cidr,
            self.acl_display(),
            self.subnets.join(", ")
        );
        if let Some(by) = &self.shadowed_by {
            s.push_str(&format!("  shadowed by #{by}"));
        }
        s
    }
}

/// Every entry of every ACL, inbound first, in rule order per ACL.
pub fn nacl_rows(nacls: &[NetworkAcl], subnet_ids: &[String]) -> Vec<NaclRow> {
    let mut rows = Vec::new();
    for inbound in [true, false] {
        for acl in nacls {
            let subnets: Vec<String> = subnet_ids
                .iter()
                .filter(|s| acl.associations.iter().any(|a| a.subnet_id == **s))
                .cloned()
                .collect();
            let entries = if inbound { &acl.inbound_entries } else { &acl.outbound_entries };
            for (i, e) in entries.iter().enumerate() {
                // The catch-all is shadowed whenever an allow-all precedes
                // it; saying so on every such ACL is noise.
                let shadowed_by = (e.rule_number < NACL_CATCH_ALL)
                    .then(|| {
                        entries[..i]
                            .iter()
                            .find(|p| entry_cover(p, &e.protocol, &e.port_range, &e.cidr) == Cover::All)
                            .map(|p| rule_label(p.rule_number))
                    })
                    .flatten();
                rows.push(NaclRow {
                    inbound,
                    acl_id: acl.acl_id.clone(),
                    rule: rule_label(e.rule_number),
                    allow: e.action.eq_ignore_ascii_case("allow"),
                    protocol: e.protocol.clone(),
                    ports: e.port_range.clone(),
                    cidr: e.cidr.clone(),
                    subnets: subnets.clone(),
                    shadowed_by,
                });
            }
        }
    }
    rows
}

/// The default shape: the first rule each way allows everything. Such an
/// ACL never blocks anything, which is worth saying instead of making the
/// reader parse two rows to find out.
pub fn nacl_allows_all(acl: &NetworkAcl) -> bool {
    let opens = |entries: &[AclEntry]| {
        entries.first().is_some_and(|e| {
            e.action.eq_ignore_ascii_case("allow")
                && e.protocol == "All"
                && matches!(e.cidr.as_str(), "0.0.0.0/0" | "::/0")
        })
    };
    opens(&acl.inbound_entries) && opens(&acl.outbound_entries)
}

/// How much of a flow one NACL entry matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Cover {
    None,
    Some,
    All,
}

fn is_ported(protocol: &str) -> bool {
    matches!(protocol.to_ascii_uppercase().as_str(), "TCP" | "UDP")
}

fn port_span(ports: &str) -> (u32, u32) {
    let parse = |p: &str| p.trim().parse::<u32>().ok();
    match ports.split_once('-') {
        Some((a, b)) => match (parse(a), parse(b)) {
            (Some(a), Some(b)) => (a, b),
            _ => (0, 65535),
        },
        None => parse(ports).map(|p| (p, p)).unwrap_or((0, 65535)),
    }
}

/// `(ipv6, address left-aligned in 128 bits, prefix length)`.
fn parse_cidr(s: &str) -> Option<(bool, u128, u8)> {
    let (addr, len) = s.split_once('/')?;
    let len: u8 = len.parse().ok()?;
    match addr.parse::<std::net::IpAddr>().ok()? {
        std::net::IpAddr::V4(a) if len <= 32 => Some((false, (u32::from(a) as u128) << 96, len)),
        std::net::IpAddr::V6(a) if len <= 128 => Some((true, u128::from(a), len)),
        _ => None,
    }
}

fn cidr_contains(outer: (bool, u128, u8), inner: (bool, u128, u8)) -> bool {
    let mask = |x: u128, len: u8| if len == 0 { 0 } else { x & (!0u128 << (128 - len as u32)) };
    outer.0 == inner.0 && outer.2 <= inner.2 && mask(outer.1, outer.2) == mask(inner.1, outer.2)
}

/// How much of the flow `(protocol, ports, peer)` the entry matches. A peer
/// that is a group or prefix list has no address here, so only a `/0`
/// entry matches all of it.
fn entry_cover(e: &AclEntry, protocol: &str, ports: &str, peer: &str) -> Cover {
    let proto = if e.protocol.eq_ignore_ascii_case("all") {
        Cover::All
    } else if protocol.eq_ignore_ascii_case("all") {
        Cover::Some
    } else if e.protocol.eq_ignore_ascii_case(protocol) {
        Cover::All
    } else {
        Cover::None
    };
    let ports = if is_ported(&e.protocol) && (is_ported(protocol) || protocol.eq_ignore_ascii_case("all")) {
        let (ea, eb) = port_span(&e.port_range);
        let (fa, fb) = if is_ported(protocol) { port_span(ports) } else { (0, 65535) };
        if ea <= fa && fb <= eb {
            Cover::All
        } else if ea <= fb && fa <= eb {
            Cover::Some
        } else {
            Cover::None
        }
    } else {
        Cover::All
    };
    let addr = match (parse_cidr(&e.cidr), parse_cidr(peer)) {
        (Some(ent), Some(flow)) if cidr_contains(ent, flow) => Cover::All,
        (Some(ent), Some(flow)) if cidr_contains(flow, ent) => Cover::Some,
        (Some(_), Some(_)) => Cover::None,
        (Some(ent), None) if ent.2 == 0 => Cover::All,
        _ => Cover::Some,
    };
    proto.min(ports).min(addr)
}

/// Run the flow through one ACL's entries one way, first match wins: every
/// entry that matches part of the flow decides that part, until one
/// matches the rest. Past the last entry AWS denies.
pub fn nacl_verdict(acl: &NetworkAcl, inbound: bool, protocol: &str, ports: &str, peer: &str) -> NaclVerdict {
    let entries = if inbound { &acl.inbound_entries } else { &acl.outbound_entries };
    let (mut allowed, mut deny, mut decided) = (false, None, false);
    for e in entries {
        let cover = entry_cover(e, protocol, ports, peer);
        if cover == Cover::None {
            continue;
        }
        if e.action.eq_ignore_ascii_case("allow") {
            allowed = true;
        } else if deny.is_none() {
            deny = Some(rule_label(e.rule_number));
        }
        if cover == Cover::All {
            decided = true;
            break;
        }
    }
    if !decided && deny.is_none() {
        deny = Some("*".to_string());
    }
    let acl = acl.acl_id.clone();
    match (allowed, deny) {
        (_, None) => NaclVerdict::Allowed,
        (false, Some(rule)) => NaclVerdict::Blocked { acl, rule },
        (true, Some(rule)) => NaclVerdict::Partly { acl, rule },
    }
}

/// One verdict across every ACL the resource sits behind (one per subnet):
/// allowed or blocked only when they all agree, else partly, naming the
/// first ACL that stops anything.
fn across_acls(acls: &[NetworkAcl], verdict: impl Fn(&NetworkAcl) -> NaclVerdict) -> NaclVerdict {
    let all: Vec<NaclVerdict> = acls.iter().map(verdict).collect();
    let Some(first_stop) = all.iter().find(|v| !v.is_allowed()).cloned() else {
        return NaclVerdict::Allowed;
    };
    if all.iter().all(|v| matches!(v, NaclVerdict::Blocked { .. })) {
        return first_stop;
    }
    match first_stop {
        NaclVerdict::Blocked { acl, rule } | NaclVerdict::Partly { acl, rule } => NaclVerdict::Partly { acl, rule },
        NaclVerdict::Allowed => NaclVerdict::Allowed,
    }
}

/// Replies to a connection land on the client's ephemeral port; AWS's
/// guidance for NACLs is to open 1024-65535 for them.
const EPHEMERAL: &str = "1024-65535";

/// The rule's own traffic and its replies through the ACLs. A rule's
/// traffic goes the rule's way (inbound rules through the inbound
/// entries); its replies go back the other way, to the peer's ephemeral
/// ports. Only TCP / UDP have replies to check.
fn verdicts(acls: &[NetworkAcl], row: &AccessRow) -> (Option<NaclVerdict>, Option<NaclVerdict>) {
    if acls.is_empty() {
        return (None, None);
    }
    let own = across_acls(acls, |a| nacl_verdict(a, row.inbound, &row.protocol, &row.ports, &row.source));
    let reply = (is_ported(&row.protocol) && !matches!(own, NaclVerdict::Blocked { .. })).then(|| {
        across_acls(acls, |a| nacl_verdict(a, !row.inbound, &row.protocol, EPHEMERAL, &row.source))
    });
    (Some(own), reply)
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
                        world: r.is_world(),
                        lb_sources: Vec::new(),
                        nacl: None,
                        reply: None,
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

/// One line of the lens body, in the order it's drawn.
#[derive(Debug, Clone, PartialEq)]
pub enum LensLine {
    /// A direction's heading (`true` = inbound).
    Direction(bool),
    /// A layer's heading. `acl` makes it selectable (`⏎` goes to the ACL);
    /// `warn` draws it in the warning colour (an unread ACL).
    Layer { text: String, acl: Option<String>, warn: bool },
    /// `rows[i]`.
    Sg(usize),
    /// `nacl_rows[i]`.
    Nacl(usize),
    Note(String),
    Blank,
}

impl LensLine {
    fn selectable(&self) -> bool {
        matches!(self, Self::Sg(_) | Self::Nacl(_) | Self::Layer { acl: Some(_), .. })
    }
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
    /// The cursor: an index into `lines`, always on a selectable one.
    pub selected: usize,
    pub scroll: usize,
    pub message: Option<String>,
    /// Security group id → the load balancers using it (`"ALB my-alb"`),
    /// from the warm ELB cache at open time.
    pub lb_by_sg: std::collections::HashMap<String, Vec<String>>,
    /// Whether the ELB cache was warm when the lens opened — a cold cache
    /// means an unlabelled `sg-` source may still be a load balancer's.
    pub elb_loaded: bool,
    /// The resource's subnets, deduped, in reference order.
    pub subnet_ids: Vec<String>,
    /// Their resolved ACLs (cache hits + fetched).
    pub nacls: Vec<NetworkAcl>,
    pub all_nacl_rows: Vec<NaclRow>,
    /// `all_nacl_rows` narrowed by `direction`.
    pub nacl_rows: Vec<NaclRow>,
    pub nacl_error: Option<String>,
    /// The body, built by `rebuild` from the above.
    pub lines: Vec<LensLine>,
    /// A row to put the cursor on once it appears (`line_key`) — going
    /// back to the lens restores the row you jumped from.
    pub want_row: Option<String>,
}

impl AccessLensState {
    pub fn open(
        title: String,
        sg_ids: Vec<String>,
        subnet_ids: Vec<String>,
        direction: AccessDirection,
    ) -> Self {
        Self {
            title,
            sg_ids,
            subnet_ids,
            direction,
            ..Default::default()
        }
    }

    /// Subnets no resolved ACL covers yet.
    pub fn missing_subnets(&self) -> Vec<String> {
        self.subnet_ids
            .iter()
            .filter(|s| !self.nacls.iter().any(|n| n.associations.iter().any(|a| a.subnet_id == **s)))
            .cloned()
            .collect()
    }

    /// Add resolved ACLs (once each, ordered by first subnet) and rebuild.
    pub fn add_nacls(&mut self, incoming: Vec<NetworkAcl>) {
        for n in incoming {
            // Only ACLs on this resource's subnets (a cache walk or a
            // loose response may carry others).
            let ours = n.associations.iter().any(|a| self.subnet_ids.contains(&a.subnet_id));
            if ours && !self.nacls.iter().any(|x| x.acl_id == n.acl_id) {
                self.nacls.push(n);
            }
        }
        let order = self.subnet_ids.clone();
        self.nacls.sort_by_key(|n| {
            order
                .iter()
                .position(|s| n.associations.iter().any(|a| a.subnet_id == *s))
                .unwrap_or(usize::MAX)
        });
        self.all_nacl_rows = nacl_rows(&self.nacls, &self.subnet_ids);
        self.apply_verdicts();
        self.rebuild();
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
        self.apply_verdicts();
        self.rebuild();
    }

    fn apply_verdicts(&mut self) {
        for row in &mut self.all_rows {
            (row.nacl, row.reply) = verdicts(&self.nacls, row);
        }
    }

    pub fn rebuild(&mut self) {
        let dir = self.direction;
        self.rows = self.all_rows.iter().filter(|r| dir.includes(r.inbound)).cloned().collect();
        self.nacl_rows = self.all_nacl_rows.iter().filter(|r| dir.includes(r.inbound)).cloned().collect();
        // The cursor follows its row, not its line number: the ACLs
        // arriving or `t` reshuffles every line.
        let was = self.selected_key();
        self.lines = self.build_lines();
        let find = |key: &str| self.lines.iter().position(|l| self.line_key(l).as_deref() == Some(key));
        let wanted = self.want_row.as_deref().and_then(find);
        let at = wanted.or_else(|| was.as_deref().and_then(find));
        if wanted.is_some() {
            self.want_row = None;
        }
        self.select_line(at.unwrap_or(0));
    }

    /// Per direction, the layers in the order AWS checks them: inbound
    /// meets the subnet's NACL first, outbound leaves through it last.
    fn build_lines(&self) -> Vec<LensLine> {
        let mut lines = Vec::new();
        let has_nacl = !self.subnet_ids.is_empty();
        let dirs: &[bool] = match self.direction {
            AccessDirection::Inbound => &[true],
            AccessDirection::Outbound => &[false],
            AccessDirection::Both => &[true, false],
        };
        for (n, &inbound) in dirs.iter().enumerate() {
            if n > 0 {
                lines.push(LensLine::Blank);
            }
            lines.push(LensLine::Direction(inbound));
            let (sg_num, acl_num) = match (has_nacl, inbound) {
                (false, _) => ("", ""),
                (true, true) => ("② ", "① "),
                (true, false) => ("① ", "② "),
            };
            if has_nacl && inbound {
                self.acl_lines(&mut lines, inbound, acl_num);
            }
            lines.push(LensLine::Layer {
                text: format!("{sg_num}Security groups · any rule allows"),
                acl: None,
                warn: false,
            });
            let before = lines.len();
            lines.extend((0..self.rows.len()).filter(|&i| self.rows[i].inbound == inbound).map(LensLine::Sg));
            if lines.len() == before && !self.loading {
                lines.push(LensLine::Note(if inbound {
                    "no inbound rules: nothing can connect in".to_string()
                } else {
                    "no outbound rules: nothing can connect out".to_string()
                }));
            }
            if has_nacl && !inbound {
                self.acl_lines(&mut lines, inbound, acl_num);
            }
        }
        lines
    }

    fn acl_lines(&self, lines: &mut Vec<LensLine>, inbound: bool, num: &str) {
        for acl in &self.nacls {
            let name = acl.tags.get("Name").map(|n| format!(" {n}")).unwrap_or_default();
            let subnets: Vec<&str> = self
                .subnet_ids
                .iter()
                .filter(|s| acl.associations.iter().any(|a| a.subnet_id == **s))
                .map(String::as_str)
                .collect();
            if nacl_allows_all(acl) {
                lines.push(LensLine::Layer {
                    text: format!("{num}NACL {}{name} · allow-all: this layer allows everything", acl.acl_id),
                    acl: Some(acl.acl_id.clone()),
                    warn: false,
                });
                continue;
            }
            lines.push(LensLine::Layer {
                text: format!("{num}NACL {}{name} · {} · first match wins", acl.acl_id, subnets.join(", ")),
                acl: Some(acl.acl_id.clone()),
                warn: false,
            });
            lines.extend(
                (0..self.nacl_rows.len())
                    .filter(|&i| self.nacl_rows[i].inbound == inbound && self.nacl_rows[i].acl_id == acl.acl_id)
                    .map(LensLine::Nacl),
            );
        }
        let missing = self.missing_subnets();
        if missing.is_empty() {
            return;
        }
        let text = if self.loading {
            format!("{num}NACL · describing…")
        } else if let Some(e) = &self.nacl_error {
            format!("{num}NACL · ⚠ {e}")
        } else {
            format!("{num}NACL · {}: not found", missing.join(", "))
        };
        lines.push(LensLine::Layer { text, acl: None, warn: !self.loading });
    }

    /// Rules on screen: the merged group rules plus the NACL entries drawn.
    pub fn rule_count(&self) -> usize {
        self.lines
            .iter()
            .filter(|l| matches!(l, LensLine::Sg(_) | LensLine::Nacl(_)))
            .count()
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn cycle_direction(&mut self) {
        self.direction = self.direction.next();
        self.rebuild();
    }

    /// Put the cursor on line `i`, or the nearest selectable line after it
    /// (else before) — a click on a heading lands on its first row.
    pub fn select_line(&mut self, i: usize) {
        let ok = |j: &usize| self.lines.get(*j).is_some_and(LensLine::selectable);
        self.selected = (i..self.lines.len())
            .find(ok)
            .or_else(|| (0..i.min(self.lines.len())).rev().find(ok))
            .unwrap_or(0);
    }

    fn step(&mut self, forward: bool, n: usize) {
        for _ in 0..n {
            let next = if forward {
                (self.selected + 1..self.lines.len()).find(|&j| self.lines[j].selectable())
            } else {
                (0..self.selected).rev().find(|&j| self.lines[j].selectable())
            };
            match next {
                Some(j) => self.selected = j,
                None => break,
            }
        }
    }

    pub fn selected_row(&self) -> Option<&AccessRow> {
        match self.lines.get(self.selected)? {
            LensLine::Sg(i) => self.rows.get(*i),
            _ => None,
        }
    }

    pub fn selected_nacl(&self) -> Option<&NaclRow> {
        match self.lines.get(self.selected)? {
            LensLine::Nacl(i) => self.nacl_rows.get(*i),
            _ => None,
        }
    }

    /// The ACL under the cursor: an entry's, or a selectable ACL heading's.
    pub fn selected_acl(&self) -> Option<String> {
        match self.lines.get(self.selected)? {
            LensLine::Nacl(i) => self.nacl_rows.get(*i).map(|r| r.acl_id.clone()),
            LensLine::Layer { acl, .. } => acl.clone(),
            _ => None,
        }
    }

    /// A row's identity across rebuilds — unlike its text, which gains a
    /// NACL verdict once the ACLs arrive.
    fn line_key(&self, line: &LensLine) -> Option<String> {
        let dir = |inbound: bool| if inbound { "in" } else { "out" };
        Some(match line {
            LensLine::Sg(i) => {
                let r = self.rows.get(*i)?;
                format!("sg {} {} {} {}", dir(r.inbound), r.protocol, r.ports, r.source)
            }
            LensLine::Nacl(i) => {
                let r = self.nacl_rows.get(*i)?;
                format!("acl {} {} {}", dir(r.inbound), r.acl_id, r.rule)
            }
            LensLine::Layer { acl: Some(id), .. } => format!("acl {id}"),
            _ => return None,
        })
    }

    pub fn selected_key(&self) -> Option<String> {
        self.lines.get(self.selected).and_then(|l| self.line_key(l))
    }

    /// The selected line as text, whichever kind it is.
    pub fn selected_text(&self) -> Option<String> {
        self.lines.get(self.selected).and_then(|l| self.line_text(l))
    }

    fn line_text(&self, line: &LensLine) -> Option<String> {
        Some(match line {
            LensLine::Direction(inbound) => direction_heading(*inbound, !self.subnet_ids.is_empty()),
            LensLine::Layer { text, .. } => text.clone(),
            LensLine::Sg(i) => self.rows.get(*i)?.text(),
            LensLine::Nacl(i) => self.nacl_rows.get(*i)?.text(),
            LensLine::Note(t) => t.clone(),
            LensLine::Blank => String::new(),
        })
    }

    /// The whole view, both directions, as text (the `e` editor view).
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
        if !self.subnet_ids.is_empty() {
            out.push_str(&format!("\nSubnets: {}", self.subnet_ids.join(", ")));
        }
        out.push_str("\n\n");
        let mut both = self.clone();
        both.direction = AccessDirection::Both;
        both.rebuild();
        for line in &both.lines {
            let indent = match line {
                LensLine::Direction(_) | LensLine::Blank => "",
                LensLine::Layer { .. } => "  ",
                _ => "    ",
            };
            out.push_str(indent);
            out.push_str(&both.line_text(line).unwrap_or_default());
            out.push('\n');
        }
        out
    }

    pub fn scroll_up(&mut self, n: usize) {
        self.step(false, n);
    }

    pub fn scroll_down(&mut self, n: usize) {
        self.step(true, n);
    }

    pub fn goto_top(&mut self) {
        self.select_line(0);
    }

    pub fn goto_bottom(&mut self) {
        self.select_line(usize::MAX);
    }
}

fn direction_heading(inbound: bool, has_nacl: bool) -> String {
    match (inbound, has_nacl) {
        (true, true) => "↓ Inbound · NACL, then security groups: both must allow".to_string(),
        (false, true) => "↑ Outbound · security groups, then NACL: both must allow".to_string(),
        (true, false) => "↓ Inbound".to_string(),
        (false, false) => "↑ Outbound".to_string(),
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
    let marks: Vec<bool> = st.rows.iter().filter_map(|r| r.nacl_mark(false)).map(|(_, all)| all).collect();
    let blocked = marks.iter().filter(|all| **all).count();
    let partly = marks.len() - blocked;
    let count = st.rule_count();
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
            format!("  {} rule{}", count, if count == 1 { "" } else { "s" }),
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
    if blocked > 0 {
        header.push(Span::styled(
            format!("  · {} blocked by a NACL", blocked),
            Style::default().fg(theme::error()),
        ));
    }
    if partly > 0 {
        header.push(Span::styled(
            format!("  · {} partly blocked", partly),
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
            format!("  {} describing groups + NACLs…", theme::spinner(app.tick_count)),
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
    // Keep a direction's heading on screen above its first rows.
    let top = if st.selected <= 2 { 0 } else { top };

    let mut body: Vec<Line> = Vec::with_capacity(viewport);
    if st.all_rows.is_empty() && st.nacls.is_empty() {
        let empty = if st.loading {
            "  Describing security groups…"
        } else {
            "  No groups resolved (see header)"
        };
        body.push(Line::styled(empty, Style::default().fg(theme::text_dim())));
    } else {
        let many_acls = st.nacls.len() > 1;
        // Size the source and group columns to what this view holds, so a
        // short table leaves room for the NACL verdict and description.
        let widths = (
            st.rows.iter().map(|r| r.source_display().chars().count()).max().unwrap_or(0).min(36),
            // One group: the chip line already names it, so no column.
            if st.groups.len() > 1 {
                st.rows.iter().map(|r| r.groups_display().chars().count()).max().unwrap_or(0).min(30)
            } else {
                0
            },
        );
        let has_nacl = !st.subnet_ids.is_empty();
        for (i, line) in st.lines.iter().enumerate().skip(top).take(viewport) {
            let selected = i == st.selected;
            body.push(match line {
                LensLine::Direction(inbound) => Line::from(Span::styled(
                    format!("  {}", direction_heading(*inbound, has_nacl)),
                    Style::default().fg(theme::heading()).add_modifier(Modifier::BOLD),
                )),
                LensLine::Layer { text, warn, .. } => {
                    let style = if *warn { theme::warning() } else { theme::text_primary() };
                    let mut style = Style::default().fg(style);
                    if selected {
                        style = style.add_modifier(Modifier::BOLD);
                    }
                    Line::from(vec![
                        Span::styled(if selected { "▸ " } else { "  " }, Style::default().fg(theme::accent())),
                        Span::styled(format!("  {text}"), style),
                    ])
                }
                LensLine::Sg(r) => sg_line(&st.rows[*r], selected, many_acls, widths),
                LensLine::Nacl(r) => nacl_line(&st.nacl_rows[*r], selected),
                LensLine::Note(t) => Line::styled(format!("      {t}"), Style::default().fg(theme::text_dim())),
                LensLine::Blank => Line::raw(""),
            });
        }
    }
    frame.render_widget(Paragraph::new(body), chunks[2]);
    {
        let mut hits = app.popup_hits.borrow_mut();
        hits.record_area(area);
        hits.record_rows(chunks[2], top, st.len());
    }

    // ── Footer ──
    let hint = |k: &'static str, d: &'static str| {
        vec![
            Span::styled(k, Style::default().fg(theme::warning())),
            Span::styled(d, Style::default().fg(theme::text_dim())),
        ]
    };
    let mut spans = vec![Span::raw("  ")];
    spans.extend(hint("j/k", " select  "));
    spans.extend(hint("⏎", " go to SG / NACL  "));
    spans.extend(hint("t", " direction  "));
    spans.extend(hint("e", " open all  "));
    spans.extend(hint("y", " copy  "));
    spans.extend(hint("r", " refetch  "));
    spans.extend(hint("Z", " width  "));
    spans.extend(hint("Esc", " close"));
    frame.render_widget(Paragraph::new(Line::from(spans)), chunks[3]);
}

/// A merged group rule: protocol, ports, source, the groups it comes from,
/// then the NACL's verdict when it stops anything (before the description,
/// so a narrow pane cuts the description first).
fn sg_line(row: &AccessRow, selected: bool, many_acls: bool, (src_w, grp_w): (usize, usize)) -> Line<'static> {
    let base = if selected {
        Style::default().fg(theme::text_primary()).add_modifier(Modifier::BOLD)
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
        Span::styled(if selected { "▸ " } else { "  " }, Style::default().fg(theme::accent())),
        Span::raw("    "),
        Span::styled(format!("{:<5} ", row.protocol), base),
        Span::styled(format!("{:<11} ", row.ports), base),
        Span::styled(format!("{:<src_w$}  ", trim(&row.source_display(), src_w)), source_style),
    ];
    if grp_w > 0 {
        spans.push(Span::styled(
            format!("← {:<grp_w$}  ", trim(&row.groups_display(), grp_w)),
            base.fg(theme::text_dim()),
        ));
    }
    if let Some((mark, all)) = row.nacl_mark(many_acls) {
        spans.push(Span::styled(
            format!("{mark}  "),
            base.fg(if all { theme::error() } else { theme::warning() }),
        ));
    }
    if !row.descriptions.is_empty() {
        spans.push(Span::styled(
            trim(&row.descriptions.join("; "), 40),
            base.fg(theme::text_dim()).add_modifier(Modifier::ITALIC),
        ));
    }
    Line::from(spans)
}

/// A NACL entry in rule order: number and verdict first, as AWS reads it.
fn nacl_line(row: &NaclRow, selected: bool) -> Line<'static> {
    let base = if selected {
        Style::default().fg(theme::text_primary()).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme::text_primary())
    };
    let shadowed = row.shadowed_by.is_some();
    let verdict = if shadowed || row.allow { base.fg(theme::text_dim()) } else { base.fg(theme::warning()) };
    let mut spans = vec![
        Span::styled(if selected { "▸ " } else { "  " }, Style::default().fg(theme::accent())),
        Span::styled(format!("    {:>5} ", row.rule), verdict),
        Span::styled(format!("{:<5} ", if row.allow { "allow" } else { "deny" }), verdict),
        Span::styled(format!("{:<5} ", row.protocol), if shadowed { base.fg(theme::text_dim()) } else { base }),
        Span::styled(format!("{:<11} ", row.ports), if shadowed { base.fg(theme::text_dim()) } else { base }),
        Span::styled(format!("{:<20} ", trim(&row.cidr, 20)), if shadowed { base.fg(theme::text_dim()) } else { base }),
    ];
    if let Some(by) = &row.shadowed_by {
        spans.push(Span::styled(format!("⊘ shadowed by #{by}"), base.fg(theme::text_dim())));
    }
    Line::from(spans)
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
            owner_id: None,
            unused: false,
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
            vec![],
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
            vec![],
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

    fn entry(n: i32, action: &str, protocol: &str, ports: &str, cidr: &str, egress: bool) -> AclEntry {
        AclEntry {
            rule_number: n,
            protocol: protocol.to_string(),
            action: action.to_string(),
            cidr: cidr.to_string(),
            port_range: ports.to_string(),
            egress,
        }
    }

    fn acl(id: &str, subnets: &[&str], inbound: Vec<AclEntry>, outbound: Vec<AclEntry>) -> NetworkAcl {
        NetworkAcl {
            acl_id: id.to_string(),
            vpc_id: "vpc-1".to_string(),
            is_default: false,
            inbound_entries: inbound,
            outbound_entries: outbound,
            associations: subnets
                .iter()
                .map(|s| crate::aws::services::vpc::AclAssociation {
                    subnet_id: s.to_string(),
                    association_id: String::new(),
                })
                .collect(),
            tags: HashMap::new(),
        }
    }

    fn allow_all(id: &str, subnets: &[&str]) -> NetworkAcl {
        acl(
            id,
            subnets,
            vec![entry(100, "allow", "All", "All", "0.0.0.0/0", false), entry(32767, "deny", "All", "All", "0.0.0.0/0", false)],
            vec![entry(100, "allow", "All", "All", "0.0.0.0/0", true), entry(32767, "deny", "All", "All", "0.0.0.0/0", true)],
        )
    }

    #[test]
    fn nacl_rows_keep_rule_order_and_mark_the_catch_all() {
        let a = acl(
            "acl-a",
            &["subnet-1", "subnet-9"],
            vec![
                entry(90, "deny", "TCP", "22", "0.0.0.0/0", false),
                entry(100, "allow", "TCP", "443", "0.0.0.0/0", false),
                entry(32767, "deny", "All", "All", "0.0.0.0/0", false),
            ],
            vec![entry(100, "allow", "All", "All", "0.0.0.0/0", true)],
        );
        let rows = nacl_rows(&[a], &["subnet-1".to_string()]);
        let summary: Vec<_> = rows.iter().map(|r| (r.inbound, r.rule.as_str(), r.allow)).collect();
        assert_eq!(summary, [(true, "90", false), (true, "100", true), (true, "*", false), (false, "100", true)]);
        // Only the resource's own subnets are listed against the ACL.
        assert_eq!(rows[0].subnets, vec!["subnet-1".to_string()]);
        assert_eq!(rows[0].acl_display(), "acl-a #90 deny");
    }

    #[test]
    fn allow_all_needs_both_directions_open_first() {
        assert!(nacl_allows_all(&allow_all("acl-d", &[])));
        let mut tight = allow_all("acl-t", &[]);
        tight.inbound_entries.insert(0, entry(50, "deny", "TCP", "22", "0.0.0.0/0", false));
        assert!(!nacl_allows_all(&tight));
    }

    /// Inbound reads NACL first, then the groups; outbound the reverse. An
    /// allow-all ACL is one selectable line, not its entries.
    #[test]
    fn layers_follow_the_order_aws_checks_them() {
        let mut st = AccessLensState::open(
            "i-1".to_string(),
            vec!["sg-a".to_string()],
            vec!["subnet-1".to_string(), "subnet-2".to_string()],
            AccessDirection::Both,
        );
        st.add_groups(vec![group(
            "sg-a",
            "app",
            vec![rule("TCP", "80", "10.0.0.0/8", None)],
            vec![rule("All", "All", "0.0.0.0/0", None)],
        )]);
        // An ACL on neither subnet is dropped; subnet-2 stays unresolved.
        st.add_nacls(vec![allow_all("acl-1", &["subnet-1"]), allow_all("acl-x", &["subnet-7"])]);
        assert_eq!(st.nacls.len(), 1);
        assert_eq!(st.missing_subnets(), vec!["subnet-2".to_string()]);
        let shape: Vec<String> = st
            .lines
            .iter()
            .map(|l| match l {
                LensLine::Direction(i) => if *i { "IN" } else { "OUT" }.to_string(),
                LensLine::Layer { text, .. } => text.split(" · ").next().unwrap().to_string(),
                LensLine::Sg(i) => format!("sg {}", st.rows[*i].ports),
                LensLine::Nacl(_) => "entry".to_string(),
                LensLine::Note(_) => "note".to_string(),
                LensLine::Blank => String::new(),
            })
            .collect();
        assert_eq!(
            shape,
            [
                "IN", "① NACL acl-1", "① NACL", "② Security groups", "sg 80", "",
                "OUT", "① Security groups", "sg All", "② NACL acl-1", "② NACL",
            ]
        );
        // The cursor stays on its rule when the ACLs arrive; it skips
        // headings, and `⏎` on the ACL's line goes to the ACL.
        assert_eq!(st.selected_row().unwrap().ports, "80");
        st.scroll_up(1);
        assert_eq!(st.selected_acl().as_deref(), Some("acl-1"));
        st.goto_bottom();
        assert_eq!(st.selected_acl().as_deref(), Some("acl-1"));
        assert!(st.selected_row().is_none());
        // A click on a heading lands on the row under it.
        st.select_line(3);
        assert_eq!(st.selected_row().unwrap().ports, "80");
        // `t` keeps the cursor on its row.
        st.cycle_direction();
        assert_eq!(st.direction, AccessDirection::Inbound);
        assert_eq!(st.selected_row().unwrap().ports, "80");
        let text = st.as_text();
        assert!(text.contains("Subnets: subnet-1, subnet-2"), "{text}");
        assert!(text.contains("↑ Outbound"), "`e` opens both directions: {text}");
    }

    fn private_acl() -> NetworkAcl {
        acl(
            "acl-p",
            &["subnet-1"],
            vec![
                entry(90, "deny", "TCP", "22", "0.0.0.0/0", false),
                entry(100, "allow", "TCP", "5432", "10.0.0.0/16", false),
                entry(110, "allow", "TCP", "443", "0.0.0.0/0", false),
                entry(120, "allow", "TCP", "443", "10.1.0.0/16", false),
                entry(32767, "deny", "All", "All", "0.0.0.0/0", false),
            ],
            vec![
                entry(100, "allow", "TCP", "1024-65535", "10.0.0.0/16", true),
                entry(32767, "deny", "All", "All", "0.0.0.0/0", true),
            ],
        )
    }

    fn verdict_of(acl: &NetworkAcl, protocol: &str, ports: &str, peer: &str) -> NaclVerdict {
        nacl_verdict(acl, true, protocol, ports, peer)
    }

    #[test]
    fn a_nacl_reads_first_match_wins() {
        let acl = private_acl();
        let blocked = |rule: &str| NaclVerdict::Blocked { acl: "acl-p".to_string(), rule: rule.to_string() };
        let partly = |rule: &str| NaclVerdict::Partly { acl: "acl-p".to_string(), rule: rule.to_string() };
        // #90 denies SSH before anything else is read.
        assert_eq!(verdict_of(&acl, "TCP", "22", "0.0.0.0/0"), blocked("90"));
        assert_eq!(verdict_of(&acl, "TCP", "22", "10.0.0.5/32"), blocked("90"));
        // Inside #100's range: allowed; wider: the rest falls to `*`.
        assert_eq!(verdict_of(&acl, "TCP", "5432", "10.0.3.0/24"), NaclVerdict::Allowed);
        assert_eq!(verdict_of(&acl, "TCP", "5432", "0.0.0.0/0"), partly("*"));
        // A range the entry only half covers is partly.
        assert_eq!(verdict_of(&acl, "TCP", "5000-6000", "10.0.0.0/16"), partly("*"));
        // Not mentioned at all: the catch-all.
        assert_eq!(verdict_of(&acl, "UDP", "53", "10.0.0.0/16"), blocked("*"));
        // A group or prefix list has no address here: only a /0 decides it.
        assert_eq!(verdict_of(&acl, "TCP", "443", "sg-web"), NaclVerdict::Allowed);
        assert_eq!(verdict_of(&acl, "TCP", "5432", "pl-corp"), partly("*"));
        // All traffic meets #90 for part of it.
        assert_eq!(verdict_of(&acl, "All", "All", "0.0.0.0/0"), partly("90"));
        // IPv6 never matches an IPv4 entry; with no IPv6 catch-all listed,
        // AWS's implicit deny applies.
        assert_eq!(verdict_of(&acl, "TCP", "443", "::/0"), blocked("*"));
    }

    #[test]
    fn group_rules_carry_the_nacl_verdict_and_their_replies() {
        let mut st = AccessLensState::open(
            "db".to_string(),
            vec!["sg-a".to_string()],
            vec!["subnet-1".to_string()],
            AccessDirection::Inbound,
        );
        st.add_groups(vec![group(
            "sg-a",
            "db",
            vec![
                rule("TCP", "22", "10.0.0.0/8", None),
                rule("TCP", "443", "0.0.0.0/0", None),
                rule("TCP", "5432", "10.0.0.0/16", None),
            ],
            vec![],
        )]);
        // No ACL yet: nothing claimed.
        assert!(st.rows.iter().all(|r| r.nacl.is_none() && r.nacl_mark(false).is_none()));
        st.add_nacls(vec![private_acl()]);
        let mark = |ports: &str| st.rows.iter().find(|r| r.ports == ports).unwrap().nacl_mark(false);
        assert_eq!(mark("22"), Some(("⊘ NACL #90 denies".to_string(), true)));
        // 443 gets in from anywhere, but replies only reach 10.0.0.0/16.
        assert_eq!(mark("443"), Some(("◐ replies: NACL #* blocks some".to_string(), false)));
        // Both ways open: nothing to say.
        assert_eq!(mark("5432"), None);
        let ssh = st.rows.iter().find(|r| r.ports == "22").unwrap();
        assert!(ssh.reply.is_none(), "a blocked rule has no replies to check");
        assert!(ssh.text().ends_with("⊘ NACL acl-p #90 denies"), "{}", ssh.text());
    }

    #[test]
    fn behind_two_acls_a_rule_one_blocks_is_partly() {
        let mut open = allow_all("acl-o", &["subnet-2"]);
        open.acl_id = "acl-o".to_string();
        let acls = [private_acl(), open];
        let row = &merge_rules(&[group("sg-a", "x", vec![rule("TCP", "22", "10.0.0.0/8", None)], vec![])])[0];
        let (own, _) = verdicts(&acls, row);
        assert_eq!(own, Some(NaclVerdict::Partly { acl: "acl-p".to_string(), rule: "90".to_string() }));
        let mut row = row.clone();
        row.nacl = own;
        assert_eq!(row.nacl_mark(true).unwrap().0, "◐ NACL acl-p #90 blocks some");
    }

    #[test]
    fn an_entry_an_earlier_one_covers_is_shadowed() {
        let rows = nacl_rows(&[private_acl()], &["subnet-1".to_string()]);
        let shadow = |rule: &str| rows.iter().find(|r| r.inbound && r.rule == rule).unwrap().shadowed_by.clone();
        assert_eq!(shadow("120"), Some("110".to_string()));
        assert_eq!(shadow("100"), None);
        assert_eq!(shadow("*"), None, "the catch-all is never marked");
    }
}
