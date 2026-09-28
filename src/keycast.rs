//! Keycast (`--show-keys`, config `show_keys`): a small box in the corner
//! naming each key as it's pressed and what it did — `W  change timeline`,
//! `4  Networking`, `⏎  follow link`. For screen recordings, screen shares
//! and demos, where the viewer can see the screen change but not the key.
//!
//! The label comes from what the key visibly *did*, not a keymap table:
//! `App` snapshots a [`View`] before a key is dispatched and again after
//! (the same two-step the macro recorder uses), and [`classify`] names the
//! difference — a lens opened, the service switched, the section changed.
//! Only when nothing it can see changed does a small fallback table label
//! the key. That keeps the labels true across the 20k lines of `handle_key`
//! without a hook in any of them.
//!
//! Typing is never echoed. While a search box, filter or picker has the
//! keyboard, only the keys that finish the input (`⏎`, `Esc`) show, so a
//! filter or a picker query never turns up in a recording twice.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// How long an entry stays on screen.
pub const SHOW_FOR: Duration = Duration::from_millis(2500);
/// A repeat of the same key within this window folds into `j ×3`.
const FOLD_WITHIN: Duration = Duration::from_millis(1200);
/// Most entries on screen at once.
const MAX_ENTRIES: usize = 4;

/// The app state a key can visibly change. Built by `App::keycast_view`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct View {
    pub service: Option<String>,
    pub details_focused: bool,
    /// The detail pane's active section label (when a resource is selected).
    pub section: Option<String>,
    /// The list's sub-tab, as its resource-type filter.
    pub sub_tab: Option<String>,
    /// The in-pane view or modal that owns the keyboard, by display name.
    pub overlay: Option<&'static str>,
    /// What is taking typed text, if anything.
    pub input: Option<Input>,
    /// The global search box's text, while it has the keyboard.
    pub search_query: String,
    pub full_width: bool,
    pub selected_id: Option<String>,
}

/// Something that takes typed text, which decides what `⏎` did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Input {
    /// The global search box (`/`, `@`).
    Search,
    /// A filter inside a pane (detail body search, log tail filter).
    Filter,
    /// A type-to-filter picker (services, regions, profiles, accounts).
    Picker,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub key: String,
    pub action: Option<String>,
    pub count: u32,
    pub at: Instant,
}

#[derive(Debug, Default)]
pub struct Keycast {
    pub entries: VecDeque<Entry>,
    /// The key being dispatched and the view before it.
    pending: Option<(KeyEvent, View)>,
}

impl Keycast {
    pub fn note(&mut self, key: KeyEvent, before: View) {
        self.pending = Some((key, before));
    }

    /// Classify the pending key against the view after it ran.
    pub fn settle(&mut self, after: &View, now: Instant) {
        let Some((key, before)) = self.pending.take() else {
            return;
        };
        let Some((label, action)) = classify(key, &before, after) else {
            return;
        };
        if let Some(last) = self.entries.back_mut() {
            if last.key == label && last.action == action && now.duration_since(last.at) < FOLD_WITHIN {
                last.count += 1;
                last.at = now;
                return;
            }
        }
        self.entries.push_back(Entry {
            key: label,
            action,
            count: 1,
            at: now,
        });
        while self.entries.len() > MAX_ENTRIES {
            self.entries.pop_front();
        }
    }

    /// Entries still on screen at `now`, oldest first.
    pub fn visible(&self, now: Instant) -> impl Iterator<Item = &Entry> {
        self.entries
            .iter()
            .filter(move |e| now.duration_since(e.at) < SHOW_FOR)
    }
}

/// The key's display label and what it did, or `None` to show nothing
/// (typing into a text input).
pub fn classify(key: KeyEvent, before: &View, after: &View) -> Option<(String, Option<String>)> {
    let label = key_label(key)?;

    // Text input: show only the keys that finish it.
    if let Some(input) = before.input {
        return match key.code {
            KeyCode::Enter => {
                let q = before.search_query.trim();
                if let (Input::Search, Some(svc)) = (input, q.strip_prefix('@')) {
                    // `@ecs ⏎` reads better than a bare ⏎: the switch is the point.
                    if let Some(svc) = svc.split_whitespace().next() {
                        return Some((format!("@{svc} ⏎"), Some("switch service".into())));
                    }
                }
                let action = match input {
                    Input::Search => "search",
                    Input::Filter => "apply filter",
                    Input::Picker => "select",
                };
                Some((label, Some(action.into())))
            }
            KeyCode::Esc => Some((label, Some("cancel".into()))),
            _ => None,
        };
    }

    Some((label, action(key, before, after)))
}

/// What the key did, from the difference it made; the fallback table only
/// when nothing visible changed.
fn action(key: KeyEvent, before: &View, after: &View) -> Option<String> {
    let jumped = before.details_focused
        && matches!(key.code, KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right)
        && (after.service != before.service || after.selected_id != before.selected_id);
    if jumped {
        return Some("follow link".into());
    }
    if matches!(key.code, KeyCode::Char('o') if key.modifiers.contains(KeyModifiers::CONTROL)) {
        return Some("back".into());
    }
    if after.overlay != before.overlay {
        return match (after.overlay, before.overlay) {
            (Some(opened), _) => Some(opened.to_string()),
            (None, Some(closed)) => Some(format!("close {closed}")),
            (None, None) => None,
        };
    }
    if let (Some(input), None) = (after.input, before.input) {
        return Some(match (input, key.code) {
            (Input::Search, KeyCode::Char('@')) => "switch service",
            (Input::Search, _) => "search",
            (Input::Filter, _) => "filter",
            (Input::Picker, _) => "pick",
        }
        .into());
    }
    if after.service != before.service {
        return after.service.as_ref().map(|s| format!("go to {s}"));
    }
    if after.details_focused != before.details_focused {
        return Some(if after.details_focused { "open" } else { "back to list" }.into());
    }
    if after.full_width != before.full_width {
        return Some(if after.full_width { "full width" } else { "split view" }.into());
    }
    // Sub-tab before section: `H` / `L` from the detail pane change both,
    // and the tab is the point (the section just resets to the default).
    if after.sub_tab != before.sub_tab {
        return after.sub_tab.clone();
    }
    if after.details_focused && after.section != before.section {
        return after.section.clone();
    }
    // Cursor movement speaks for itself.
    if matches!(
        key.code,
        KeyCode::Char('j' | 'k' | 'g' | 'G') | KeyCode::Up | KeyCode::Down | KeyCode::PageUp | KeyCode::PageDown
    ) && key.modifiers.difference(KeyModifiers::SHIFT).is_empty()
    {
        return None;
    }
    fallback(key, after).map(str::to_string)
}

/// Labels for keys whose effect the view can't see (a filter chip, a wrap
/// toggle, a copy). Kept small on purpose: anything visible is named above.
fn fallback(key: KeyEvent, after: &View) -> Option<&'static str> {
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return match key.code {
            KeyCode::Char('d') => Some("half page down"),
            KeyCode::Char('u') => Some("half page up"),
            KeyCode::Char('x') => Some("export list"),
            _ => None,
        };
    }
    let in_tail = matches!(after.overlay, Some("live tail" | "log search"));
    let in_lens = matches!(after.overlay, Some("change timeline" | "network access" | "referenced by"));
    match key.code {
        KeyCode::Char('t') if after.overlay == Some("network access") => Some("direction"),
        KeyCode::Char('[') | KeyCode::Char(']') if in_tail || in_lens => Some("time window"),
        KeyCode::Char('f') if in_lens => Some("source filter"),
        KeyCode::Char('w') if in_tail => Some("wrap lines"),
        KeyCode::Char('f') if in_tail => Some("follow"),
        KeyCode::Char('w') => Some("watch mode"),
        KeyCode::Char('f') => Some("filter"),
        KeyCode::Char('F') => Some("state filter"),
        KeyCode::Char('y') => Some("copy"),
        KeyCode::Char('C') => Some("copy CLI command"),
        KeyCode::Char('e') => Some("open in editor"),
        KeyCode::Char('r') => Some("refresh"),
        KeyCode::Char('z') => Some("sort"),
        KeyCode::Char('a') => Some("hide noise"),
        KeyCode::Char('X') => Some("export"),
        KeyCode::Char('\\') => Some("flat view"),
        _ => None,
    }
}

/// `⏎`, `Esc`, `Ctrl-O`, `⇥`; `None` for keys that never show (bare
/// modifiers, media keys).
pub fn key_label(key: KeyEvent) -> Option<String> {
    let base = match key.code {
        KeyCode::Char(' ') => "Space".to_string(),
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Enter => "⏎".into(),
        KeyCode::Esc => "Esc".into(),
        KeyCode::Tab => "⇥".into(),
        KeyCode::BackTab => "⇤".into(),
        KeyCode::Backspace => "⌫".into(),
        KeyCode::Up => "↑".into(),
        KeyCode::Down => "↓".into(),
        KeyCode::Left => "←".into(),
        KeyCode::Right => "→".into(),
        KeyCode::PageUp => "PgUp".into(),
        KeyCode::PageDown => "PgDn".into(),
        KeyCode::Home => "Home".into(),
        KeyCode::End => "End".into(),
        KeyCode::Delete => "Del".into(),
        KeyCode::F(n) => format!("F{n}"),
        _ => return None,
    };
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        Some(format!("Ctrl-{}", base.to_uppercase()))
    } else if key.modifiers.contains(KeyModifiers::ALT) {
        Some(format!("Alt-{base}"))
    } else {
        Some(base)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }
    fn code(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }
    fn list() -> View {
        View {
            service: Some("ECS".into()),
            sub_tab: Some("ECS Service".into()),
            ..View::default()
        }
    }

    #[test]
    fn names_what_the_key_did() {
        let before = list();
        let after = View { overlay: Some("change timeline"), ..list() };
        assert_eq!(classify(k('W'), &before, &after), Some(("W".into(), Some("change timeline".into()))));

        let after = View { details_focused: true, section: Some("Overview".into()), ..list() };
        assert_eq!(classify(code(KeyCode::Enter), &before, &after).unwrap().1.as_deref(), Some("open"));

        let before = View { details_focused: true, section: Some("Overview".into()), ..list() };
        let after = View { section: Some("Deployments".into()), ..before.clone() };
        assert_eq!(classify(k('2'), &before, &after).unwrap().1.as_deref(), Some("Deployments"));

        let after = View { service: Some("ELB".into()), selected_id: Some("tg".into()), ..before.clone() };
        assert_eq!(classify(code(KeyCode::Enter), &before, &after).unwrap().1.as_deref(), Some("follow link"));
    }

    #[test]
    fn never_echoes_typed_text() {
        let typing = View { input: Some(Input::Search), search_query: "secret-thing".into(), ..list() };
        assert_eq!(classify(k('s'), &typing, &typing), None);
        let (key, action) = classify(code(KeyCode::Enter), &typing, &list()).unwrap();
        assert_eq!((key.as_str(), action.as_deref()), ("⏎", Some("search")));

        // An @service switch is the exception: the service is the point.
        let at = View { input: Some(Input::Search), search_query: "@ecs".into(), ..list() };
        assert_eq!(classify(code(KeyCode::Enter), &at, &list()).unwrap().0, "@ecs ⏎");

        let picker = View { input: Some(Input::Picker), overlay: Some("regions"), ..list() };
        assert_eq!(classify(k('e'), &picker, &picker), None);
        assert_eq!(classify(code(KeyCode::Enter), &picker, &list()).unwrap().1.as_deref(), Some("select"));
    }

    #[test]
    fn cursor_keys_carry_no_label_and_repeats_fold() {
        let v = list();
        assert_eq!(classify(k('j'), &v, &v), Some(("j".into(), None)));
        let mut kc = Keycast::default();
        let t0 = Instant::now();
        for i in 0..3 {
            kc.note(k('j'), v.clone());
            kc.settle(&v, t0 + Duration::from_millis(100 * i));
        }
        assert_eq!(kc.entries.len(), 1);
        assert_eq!(kc.entries[0].count, 3);
    }

    #[test]
    fn entries_expire_and_cap() {
        let v = list();
        let mut kc = Keycast::default();
        let t0 = Instant::now();
        for (i, c) in "yeryeX".chars().enumerate() {
            kc.note(k(c), v.clone());
            kc.settle(&v, t0 + Duration::from_millis(10 * i as u64));
        }
        assert_eq!(kc.entries.len(), MAX_ENTRIES);
        assert_eq!(kc.visible(t0 + SHOW_FOR + Duration::from_secs(1)).count(), 0);
    }

    #[test]
    fn labels_modifiers() {
        let ctrl_o = KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL);
        assert_eq!(key_label(ctrl_o).as_deref(), Some("Ctrl-O"));
        assert_eq!(classify(ctrl_o, &list(), &list()).unwrap().1.as_deref(), Some("back"));
    }
}
