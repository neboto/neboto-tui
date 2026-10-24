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
//! The `:` command picker lists the rows it can run: a gated row runs its
//! hint key, any other row only with an explicit [`Command::runs`]. Running
//! a row replays its key, so the picker can't do anything the keyboard
//! can't. Rows that aren't one key (`j / k`, `Tab · 1–9`, mouse gestures)
//! stay help-only.
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

/// The key the `:` picker presses for a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Run {
    Key(char),
    Ctrl(char),
}

impl Run {
    pub fn key_event(self) -> crossterm::event::KeyEvent {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        match self {
            // Terminals report an uppercase letter with SHIFT, and the
            // list-pane arms match on it (`(Char('W'), SHIFT)`).
            Run::Key(c) if c.is_ascii_uppercase() => KeyEvent::new(KeyCode::Char(c), KeyModifiers::SHIFT),
            Run::Key(c) => KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE),
            Run::Ctrl(c) => KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL),
        }
    }
}

pub struct Command {
    pub section: Section,
    pub key: &'static str,
    pub label: &'static str,
    pub gate: Option<Gate>,
    run: Option<Run>,
}

impl Command {
    /// Make a row runnable from the `:` picker.
    const fn runs(mut self, run: Run) -> Self {
        self.run = Some(run);
        self
    }

    /// The key the picker presses: the explicit one, else a gated row's
    /// hint key.
    pub fn run(&self) -> Option<Run> {
        self.run.or_else(|| {
            let mut chars = self.gate.as_ref()?.hint.0.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => Some(Run::Key(c)),
                _ => None,
            }
        })
    }

    /// Whether the picker offers this row now: it runs a key, the key means
    /// this in the focused pane, and its gate (if any) passes.
    pub fn runnable_in(&self, app: &App) -> bool {
        self.run().is_some()
            && match self.section {
                Section::ListPane => !app.details_focused,
                Section::DetailPane => app.details_focused,
                _ => true,
            }
            && self.gate.as_ref().is_none_or(|g| (g.check)(app))
    }
}

const fn cmd(section: Section, key: &'static str, label: &'static str) -> Command {
    Command { section, key, label, gate: None, run: None }
}

const fn gated(
    section: Section,
    key: &'static str,
    label: &'static str,
    check: fn(&App) -> bool,
    hint: (&'static str, &'static str),
    in_detail: bool,
) -> Command {
    Command { section, key, label, gate: Some(Gate { check, hint, in_detail }), run: None }
}

use Run::{Ctrl, Key};
use Section::*;

pub static COMMANDS: &[Command] = &[
    cmd(ListPane, "j / k ↑ / ↓", "move"),
    cmd(ListPane, "gg / G", "top / bottom"),
    cmd(ListPane, "^d / ^u", "half page"),
    cmd(ListPane, "l / → / ⏎", "open details · follow →"),
    cmd(ListPane, "⌫ / Ctrl-O", "history back").runs(Ctrl('o')),
    cmd(ListPane, "Ctrl-F", "history forward").runs(Ctrl('f')),
    cmd(ListPane, "`", "jump list (history)").runs(Key('`')),
    cmd(ListPane, "B", "bookmark this").runs(Key('B')),
    cmd(ListPane, "'", "bookmarks").runs(Key('\'')),
    cmd(ListPane, "Tab · 1–9", "sub-tabs"),
    cmd(ListPane, "H / L", "prev / next sub-tab"),
    cmd(ListPane, "V · J / K", "visual select · extend"),
    cmd(ListPane, "Ctrl-A", "select all rows"),
    cmd(ListPane, "y · X · ^X", "copy · export selection"),
    cmd(ListPane, "a", "hide noisy rows").runs(Key('a')),
    cmd(ListPane, "z", "cycle sort").runs(Key('z')),
    cmd(ListPane, "F", "cycle state filter").runs(Key('F')),
    cmd(ListPane, "w", "watch mode (+/- interval)").runs(Key('w')),
    cmd(ListPane, "r / F5", "refresh service").runs(Key('r')),
    cmd(DetailPane, "Tab · 1–9", "sections"),
    cmd(DetailPane, "H / L", "prev / next sub-tab"),
    cmd(DetailPane, "h / ← / Esc", "back to list"),
    cmd(DetailPane, "[[ / ]]", "prev / next header"),
    cmd(DetailPane, "/", "filter body text"),
    cmd(DetailPane, "n / N", "next / previous filter match"),
    cmd(DetailPane, "V · J / K", "visual select · extend"),
    cmd(DetailPane, "Ctrl-A", "select all"),
    cmd(DetailPane, "y / c", "copy selection or row"),
    cmd(DetailPane, "\\", "flat view (all sections)").runs(Key('\\')),
    cmd(DetailPane, "Ctrl-W", "wrap long values").runs(Ctrl('w')),
    cmd(DetailPane, "Z", "full-width pane").runs(Key('Z')),
    cmd(DetailPane, "r", "refresh this resource").runs(Key('r')),
    cmd(Actions, "O", "open in AWS Console").runs(Key('O')),
    cmd(Actions, "y", "copy ARN / id").runs(Key('y')),
    cmd(Actions, "C", "copy a CLI command (read · connect · change)").runs(Key('C')),
    cmd(Actions, "e", "open in $EDITOR").runs(Key('e')),
    gated(
        Actions,
        "m",
        "metrics charts (c: change markers from the W timeline)",
        App::supports_metrics_overlay,
        ("m", "metrics"),
        true,
    ),
    gated(Actions, "t", "live log tail ([/] window)", App::supports_log_tail, ("t", "tail"), true),
    cmd(Actions, "f", "log search · event filter").runs(Key('f')),
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
    cmd(Actions, "d", "download Lambda package").runs(Key('d')),
    cmd(Actions, "X", "export (detail · selected rows)").runs(Key('X')),
    cmd(Actions, "^X", "export the list").runs(Ctrl('x')),
    cmd(Actions, "M", "message history").runs(Key('M')),
    cmd(Actions, ",", "macros (⏎ run · n record)").runs(Key(',')),
    cmd(Actions, "b", "toggle banner").runs(Key('b')),
    cmd(Actions, ":", "command picker (run a key by name)"),
    cmd(Actions, "? / q", "help (type to filter) / quit"),
    cmd(Search, "/", "fuzzy search").runs(Key('/')),
    cmd(Search, "@svc text", "switch service + search").runs(Key('@')),
    cmd(Search, "@all text", "search all cached services (⇥ steps through them)"),
    cmd(Search, "tag:k=v", "exact tag filter"),
    cmd(Search, "Tab", "complete @prefix"),
    cmd(Search, "↑ / ↓", "earlier searches (this session)"),
    cmd(Context, "S", "switch service").runs(Key('S')),
    cmd(Context, "R", "switch region").runs(Key('R')),
    cmd(Context, "P", "switch profile").runs(Key('P')),
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

/// What the `:` picker lists for `query`: with no query, the rows that work
/// on the selection ("Here") first, then the rest in table order; with one,
/// best match first over key, label and section title (an exact key match
/// wins, so `:m` puts metrics on top).
pub fn picker_rows(app: &App, query: &str) -> Vec<&'static Command> {
    let rows: Vec<&'static Command> = COMMANDS.iter().filter(|c| c.runnable_in(app)).collect();
    if query.trim().is_empty() {
        let (here, rest): (Vec<_>, Vec<_>) = rows.into_iter().partition(|c| c.gate.is_some());
        return here.into_iter().chain(rest).collect();
    }
    crate::search::fuzzy::FuzzyMatcher::new()
        .filter_fields(query, &rows, |c| vec![c.key, c.label, c.section.title()])
        .into_iter()
        .map(|(i, _)| rows[i])
        .collect()
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

    #[test]
    fn a_gated_row_runs_its_hint_key() {
        let m = COMMANDS.iter().find(|c| c.key == "m").unwrap();
        assert_eq!(m.run(), Some(Run::Key('m')));
        let reveal = COMMANDS.iter().find(|c| c.key == "x / Y").unwrap();
        assert_eq!(reveal.run(), Some(Run::Key('x')));
        let moves = COMMANDS.iter().find(|c| c.key == "j / k ↑ / ↓").unwrap();
        assert_eq!(moves.run(), None, "not one key: help only");
    }

    #[test]
    fn an_explicit_run_is_one_of_the_rows_keys() {
        // `⌫ / Ctrl-O` runs Ctrl-O; `r / F5` runs r; `@svc text` runs @.
        for c in COMMANDS.iter().filter(|c| c.run.is_some()) {
            let shown = match c.run.unwrap() {
                Run::Key(k) => k.to_string(),
                Run::Ctrl(k) => format!("Ctrl-{}", k.to_ascii_uppercase()),
            };
            let alt = shown.replace("Ctrl-", "^");
            assert!(
                c.key.split([' ', '/']).any(|k| k == shown || k == alt) || c.key.starts_with(&shown),
                "row {:?} runs {shown:?}",
                c.key
            );
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
