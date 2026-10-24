//! The command table: every key the `?` help lists, in one place.
//!
//! The help overlay draws its sections from [`COMMANDS`], and the status
//! bar's conditional hints (`m metrics`, `t tail`, `N access`…) come from
//! the same rows' [`Gate`]s. A gate is the `App::supports_*` check that
//! says the key works on the current selection, so one row carries both
//! the help line and the hint, and the help's "Here" section is just the
//! rows whose gate passes. Before this table the two lists were written
//! separately and drifted (DX VIFs shipped with a working `m` and no hint).
//!
//! A new `supports_*` gate goes here, on its command's row; the
//! `every_supports_gate_is_in_the_table` test fails until it does.
//!
//! Labels are one line each, short labels, never sentences: help has no
//! word wrap and truncates with `…`.

use crate::app::App;

/// Where a command is listed in the help, in column order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    ListPane,
    DetailPane,
    Actions,
    Search,
    Context,
    Mouse,
    Toggles,
}

impl Section {
    pub const LEFT: &'static [Section] = &[Section::ListPane, Section::DetailPane];
    pub const RIGHT: &'static [Section] =
        &[Section::Actions, Section::Search, Section::Context, Section::Mouse, Section::Toggles];

    pub fn title(self) -> &'static str {
        match self {
            Section::ListPane => "List pane",
            Section::DetailPane => "Detail pane",
            Section::Actions => "Actions",
            Section::Search => "Search",
            Section::Context => "Context",
            Section::Mouse => "Mouse",
            Section::Toggles => "Toggles",
        }
    }
}

/// When a command only works on some selections: the check, and the
/// status-bar hint shown while it passes.
pub struct Gate {
    pub check: fn(&App) -> bool,
    /// `(key, label)` as the status bar prints it.
    pub hint: (&'static str, &'static str),
    /// Also hinted while the detail pane is focused. That bar is kept short
    /// (the pane's own footer carries the rest), so most hints are
    /// list-pane only.
    pub in_detail: bool,
}

pub struct Command {
    pub section: Section,
    pub key: &'static str,
    pub label: &'static str,
    pub gate: Option<Gate>,
}

const fn cmd(section: Section, key: &'static str, label: &'static str) -> Command {
    Command { section, key, label, gate: None }
}

const fn gated(
    section: Section,
    key: &'static str,
    label: &'static str,
    check: fn(&App) -> bool,
    hint: (&'static str, &'static str),
    in_detail: bool,
) -> Command {
    Command { section, key, label, gate: Some(Gate { check, hint, in_detail }) }
}

use Section::*;

pub static COMMANDS: &[Command] = &[
    cmd(ListPane, "j / k ↑ / ↓", "move"),
    cmd(ListPane, "gg / G", "top / bottom"),
    cmd(ListPane, "^d / ^u", "half page"),
    cmd(ListPane, "l / → / ⏎", "open details · follow →"),
    cmd(ListPane, "⌫ / Ctrl-O", "history back"),
    cmd(ListPane, "Ctrl-F", "history forward"),
    cmd(ListPane, "`", "jump list (history)"),
    cmd(ListPane, "B / '", "bookmark / bookmarks"),
    cmd(ListPane, "Tab · 1–9", "sub-tabs"),
    cmd(ListPane, "H / L", "prev / next sub-tab"),
    cmd(ListPane, "V · J / K", "visual select · extend"),
    cmd(ListPane, "Ctrl-A", "select all rows"),
    cmd(ListPane, "y · X · ^X", "copy · export selection"),
    cmd(ListPane, "a", "hide noisy rows"),
    cmd(ListPane, "z", "cycle sort"),
    cmd(ListPane, "F", "cycle state filter"),
    cmd(ListPane, "w", "watch mode (+/- interval)"),
    cmd(ListPane, "r / F5", "refresh service"),
    cmd(DetailPane, "Tab · 1–9", "sections"),
    cmd(DetailPane, "H / L", "prev / next sub-tab"),
    cmd(DetailPane, "h / ← / Esc", "back to list"),
    cmd(DetailPane, "[[ / ]]", "prev / next header"),
    cmd(DetailPane, "/", "filter body text"),
    cmd(DetailPane, "V · J / K", "visual select · extend"),
    cmd(DetailPane, "Ctrl-A", "select all"),
    cmd(DetailPane, "y / c", "copy selection or row"),
    cmd(DetailPane, "\\", "flat view (all sections)"),
    cmd(DetailPane, "Ctrl-W", "wrap long values"),
    cmd(DetailPane, "Z", "full-width pane"),
    cmd(DetailPane, "r", "refresh this resource"),
    cmd(Actions, "O", "open in AWS Console"),
    cmd(Actions, "y", "copy ARN / id"),
    cmd(Actions, "C", "copy a CLI command (read · connect · change)"),
    cmd(Actions, "e", "open in $EDITOR"),
    gated(
        Actions,
        "m",
        "metrics charts (c: change markers from the W timeline)",
        App::supports_metrics_overlay,
        ("m", "metrics"),
        true,
    ),
    gated(Actions, "t", "live log tail ([/] window)", App::supports_log_tail, ("t", "tail"), true),
    cmd(Actions, "f", "log search · event filter"),
    gated(
        Actions,
        "W",
        "change timeline (what changed around this)",
        App::supports_trail_lens,
        ("W", "trail"),
        false,
    ),
    gated(
        Actions,
        "U",
        "Referenced-by lens (what uses this, from loaded services)",
        App::supports_refs_lens,
        ("U", "refs"),
        false,
    ),
    gated(
        Actions,
        "N",
        "Network-access lens (merged security-group rules + subnet NACLs)",
        App::supports_access_lens,
        ("N", "access"),
        false,
    ),
    gated(Actions, "o", "S3 objects", App::supports_object_browser, ("o", "objects"), false),
    gated(
        Actions,
        "i",
        "DynamoDB items · memory sessions",
        App::supports_item_browser,
        ("i", "items"),
        false,
    ),
    gated(Actions, "s", "SSM session · ECS exec", App::supports_session, ("s", "session"), true),
    gated(
        Actions,
        "x / Y",
        "reveal / copy a value (secret, parameter) · agent card",
        App::supports_value_reveal,
        ("x", "reveal"),
        false,
    ),
    gated(
        Actions,
        "x",
        "test a DNS record's answer",
        App::supports_dns_test,
        ("x", "test answer"),
        false,
    ),
    cmd(Actions, "d", "download Lambda package"),
    cmd(Actions, "X / ^X", "export detail / list"),
    cmd(Actions, "M", "message history"),
    cmd(Actions, ",", "macros (⏎ run · n record)"),
    cmd(Actions, "b", "toggle banner"),
    cmd(Actions, "? / q", "help (type to filter) / quit"),
    cmd(Search, "/", "fuzzy search"),
    cmd(Search, "@svc text", "switch service + search"),
    cmd(Search, "@all text", "search all cached services (⇥ steps through them)"),
    cmd(Search, "tag:k=v", "exact tag filter"),
    cmd(Search, "Tab", "complete @prefix"),
    cmd(Search, "↑ / ↓", "earlier searches (this session)"),
    cmd(Context, "S / R / P", "service / region / profile"),
    cmd(Context, "@ec2 …", "switch via prefix"),
    gated(
        Context,
        "s",
        "assume role (org account)",
        App::supports_org_assume,
        ("s", "assume role"),
        true,
    ),
    cmd(Mouse, "click", "select · tabs · hints · badges"),
    cmd(Mouse, "double-click", "⏎ (open · follow · confirm)"),
    cmd(Mouse, "click → · title bar", "follow link · full width"),
    cmd(Mouse, "click ⌫ back · ⌃F fwd", "back · forward through history"),
    cmd(Mouse, "click ↑ vX.Y.Z", "copy the upgrade command"),
    cmd(Mouse, "wheel · drag", "scroll · select rows"),
    cmd(Mouse, "right-click", "back · close a popup or view"),
    cmd(Mouse, "Shift-drag", "terminal text select (iTerm: ⌥)"),
    cmd(Toggles, "1–6 · 7", "cost group-by (5 tag, 6 category) · anomalies"),
    cmd(Toggles, "[ ] · t", "cost period: step · cycle"),
    cmd(Toggles, "t", "WAF scope · RAM owner"),
    cmd(Toggles, "f", "ECS task · execution filter"),
];

/// The rows listed under `section`, in table order.
pub fn in_section(section: Section) -> impl Iterator<Item = &'static Command> {
    COMMANDS.iter().filter(move |c| c.section == section)
}

/// The gated commands that work on the current selection, in table order:
/// the help's "Here" section.
pub fn available(app: &App) -> impl Iterator<Item = &'static Command> + '_ {
    COMMANDS.iter().filter(|c| c.gate.as_ref().is_some_and(|g| (g.check)(app)))
}

/// The status bar's conditional hints for the focused pane.
pub fn status_hints(app: &App, detail: bool) -> Vec<(&'static str, &'static str)> {
    available(app)
        .filter_map(|c| c.gate.as_ref())
        .filter(|g| !detail || g.in_detail)
        .map(|g| g.hint)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_section_is_in_a_column_and_has_rows() {
        for s in Section::LEFT.iter().chain(Section::RIGHT) {
            assert!(in_section(*s).next().is_some(), "{} is empty", s.title());
        }
        for c in COMMANDS {
            assert!(
                Section::LEFT.contains(&c.section) || Section::RIGHT.contains(&c.section),
                "{} is in no column",
                c.key
            );
        }
    }

    #[test]
    fn a_hint_names_its_rows_key() {
        // `x / Y` hints as `x`: the hint key is one of the row's keys.
        for c in COMMANDS {
            if let Some(g) = &c.gate {
                assert!(
                    c.key.split(" / ").any(|k| k == g.hint.0),
                    "row {:?} hints {:?}",
                    c.key,
                    g.hint.0
                );
            }
        }
    }

    /// The drift guard: a `supports_*` gate on `App` that isn't on a row
    /// here would be a working key with no help line and no hint.
    #[test]
    fn every_supports_gate_is_in_the_table() {
        let app = include_str!("app.rs");
        let table = include_str!("commands.rs");
        let gates: Vec<&str> = app
            .lines()
            .filter_map(|l| l.trim().strip_prefix("pub fn supports_"))
            .filter_map(|rest| rest.split('(').next())
            .collect();
        assert!(gates.len() >= 10, "found {gates:?}");
        for g in gates {
            assert!(
                table.contains(&format!("App::supports_{g},")),
                "App::supports_{g} has no row in COMMANDS"
            );
        }
    }
}
