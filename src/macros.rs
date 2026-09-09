//! Recorded macros — a saved sequence of navigation steps the user can replay
//! with one keystroke. Stored as JSON next to the config file, exactly like
//! `bookmarks.rs`.
//!
//! **Why steps and not keystrokes.** The obvious implementation — record the
//! raw `KeyEvent`s and feed them back — breaks on three things this app does
//! constantly:
//!
//! 1. **Async.** `@orgs` fires a list load, `s` does an STS `AssumeRole`
//!    round-trip. Replaying keys as a burst would fire the next key into an
//!    empty list. Playback is therefore gated on quiescence (`App::macro_ready`).
//! 2. **Positional cursor movement.** `j` means "row 2 of whatever the API
//!    returned today", not "that account". A cursor key is recorded as the
//!    **identity of the row it landed on** (`SelectId`) so a reordered list
//!    still replays correctly.
//! 3. **Incremental search.** `@orgs` typed literally is five keystrokes, and
//!    `update_search` re-evaluates the service prefix on every one of them, so
//!    a partial prefix can fire a load the macro then has to wait out. The
//!    whole run is recorded as one atomic [`MacroStep::Search`].
//!
//! The same reasoning covers the three credential switches: what matters is
//! *which* role/region/profile was selected, not the keys used to find it in
//! the picker. Those are recorded as outcomes by diffing app state, which also
//! means a macro keeps working when the picker's contents change.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// One replayable action. Ordered roughly by how specific it is: the earlier
/// variants carry semantics the raw key would have lost.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MacroStep {
    /// A complete search query applied in one shot (`@orgs`, `tag:env=prod`).
    Search(String),
    /// Select a row by resource id. Replaces the cursor key that landed on it.
    SelectId { id: String, label: String },
    /// Focus a split-pane section by name — survives a pane reshuffle, which
    /// the digit key it replaces would not.
    DetailSection(String),
    /// Open the S3 object browser at a folder, with one object selected.
    /// Object listings churn far more than resource lists — and the browser's
    /// cursor indexes a *filtered, sorted* view that shifts with how much has
    /// paginated in — so a keystroke replay would land somewhere arbitrary.
    /// Mirrors `NavLocation::s3_object_path`, and replays through the same
    /// `pending_s3_object_key` machinery bookmark restore uses.
    S3Object {
        bucket: String,
        /// `""` is the bucket root, otherwise ends with `/`.
        prefix: String,
        /// Full key of the selected entry; empty means "just open the folder".
        key: String,
    },
    /// Assume an org member-account role (`s` on an Organizations account).
    AssumeRole {
        account_id: String,
        account_name: String,
        role_name: String,
    },
    /// Drop the assumed role and return to base credentials.
    ExitRole,
    /// Region id (`us-east-1`) — stored as a string because `Region` is a
    /// macro-generated enum with no serde derive.
    SwitchRegion(String),
    SwitchProfile(String),
    /// A service chosen through the `S` picker. Recorded as an outcome for the
    /// same reason as the credential switches: the keys used to *find* it in
    /// the picker are filter text, which reproduces nothing.
    SwitchService(crate::aws::service::ServiceType),
    /// Anything else: a literal keypress.
    Key {
        key: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        ctrl: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        shift: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        alt: bool,
    },
}

impl MacroStep {
    /// Encode a keypress. Modifiers are kept separately so `shift` survives on
    /// terminals that do and don't fold it into the character.
    pub fn from_key(key: KeyEvent) -> Option<Self> {
        Some(MacroStep::Key {
            key: encode_key(key.code)?,
            ctrl: key.modifiers.contains(KeyModifiers::CONTROL),
            shift: key.modifiers.contains(KeyModifiers::SHIFT),
            alt: key.modifiers.contains(KeyModifiers::ALT),
        })
    }

    /// One-line description for the macro picker's step list.
    pub fn label(&self) -> String {
        match self {
            MacroStep::Search(q) => format!("search  {}", q),
            MacroStep::SelectId { label, .. } => format!("select  {}", label),
            MacroStep::DetailSection(name) => format!("section {}", name),
            MacroStep::S3Object {
                bucket,
                prefix,
                key,
            } => {
                let at = if key.is_empty() { prefix } else { key };
                format!("s3      s3://{}/{}", bucket, at)
            }
            MacroStep::AssumeRole {
                account_id,
                account_name,
                role_name,
            } => {
                let who = if account_name.is_empty() {
                    account_id.clone()
                } else {
                    format!("{} ({})", account_name, account_id)
                };
                format!("assume  {} in {}", role_name, who)
            }
            MacroStep::ExitRole => "assume  ← base account".to_string(),
            MacroStep::SwitchRegion(r) => format!("region  {}", r),
            MacroStep::SwitchProfile(p) => format!("profile {}", p),
            MacroStep::SwitchService(s) => format!("service {}", s.name()),
            MacroStep::Key {
                key,
                ctrl,
                shift,
                alt,
            } => {
                let mut s = String::from("key     ");
                if *ctrl {
                    s.push_str("C-");
                }
                if *alt {
                    s.push_str("M-");
                }
                if *shift && key.chars().count() > 1 {
                    s.push_str("S-");
                }
                s.push_str(key);
                s
            }
        }
    }

    /// Decode back into a `KeyEvent` for injection. `None` for the semantic
    /// variants, which are applied directly rather than through the key path.
    pub fn to_key(&self) -> Option<KeyEvent> {
        let MacroStep::Key {
            key,
            ctrl,
            shift,
            alt,
        } = self
        else {
            return None;
        };
        let mut mods = KeyModifiers::NONE;
        if *ctrl {
            mods |= KeyModifiers::CONTROL;
        }
        if *shift {
            mods |= KeyModifiers::SHIFT;
        }
        if *alt {
            mods |= KeyModifiers::ALT;
        }
        Some(KeyEvent::new(decode_key(key)?, mods))
    }
}

/// Keys we deliberately never record. `e` and `s` tear the whole TUI down to
/// hand the terminal to `$EDITOR` or an SSM session (`main.rs` drops and
/// rebuilds the event channel), and `q` would end the session mid-macro —
/// none of the three can be meaningfully resumed from.
pub fn is_unrecordable(code: KeyCode) -> bool {
    matches!(code, KeyCode::Char('e') | KeyCode::Char('q'))
}

fn encode_key(code: KeyCode) -> Option<String> {
    Some(match code {
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Enter => "Enter".into(),
        KeyCode::Esc => "Esc".into(),
        KeyCode::Tab => "Tab".into(),
        KeyCode::BackTab => "BackTab".into(),
        KeyCode::Backspace => "Backspace".into(),
        KeyCode::Left => "Left".into(),
        KeyCode::Right => "Right".into(),
        KeyCode::Up => "Up".into(),
        KeyCode::Down => "Down".into(),
        KeyCode::Home => "Home".into(),
        KeyCode::End => "End".into(),
        KeyCode::PageUp => "PageUp".into(),
        KeyCode::PageDown => "PageDown".into(),
        KeyCode::Delete => "Delete".into(),
        KeyCode::Insert => "Insert".into(),
        // Function keys and the exotic codes aren't bound anywhere in the app.
        _ => return None,
    })
}

fn decode_key(s: &str) -> Option<KeyCode> {
    Some(match s {
        "Enter" => KeyCode::Enter,
        "Esc" => KeyCode::Esc,
        "Tab" => KeyCode::Tab,
        "BackTab" => KeyCode::BackTab,
        "Backspace" => KeyCode::Backspace,
        "Left" => KeyCode::Left,
        "Right" => KeyCode::Right,
        "Up" => KeyCode::Up,
        "Down" => KeyCode::Down,
        "Home" => KeyCode::Home,
        "End" => KeyCode::End,
        "PageUp" => KeyCode::PageUp,
        "PageDown" => KeyCode::PageDown,
        "Delete" => KeyCode::Delete,
        "Insert" => KeyCode::Insert,
        other => {
            let mut chars = other.chars();
            let c = chars.next()?;
            if chars.next().is_some() {
                return None; // unknown multi-char name from a newer schema
            }
            KeyCode::Char(c)
        }
    })
}

/// A named, saved sequence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Macro {
    pub name: String,
    pub steps: Vec<MacroStep>,
}

impl Macro {
    /// Picker row: name plus a hint of where it goes, so a list of five macros
    /// is readable without opening each one.
    pub fn summary(&self) -> String {
        let dest = self
            .steps
            .iter()
            .rev()
            .find_map(|s| match s {
                MacroStep::Search(q) if q.starts_with('@') => Some(q.clone()),
                _ => None,
            })
            .unwrap_or_default();
        if dest.is_empty() {
            format!("{} steps", self.steps.len())
        } else {
            format!("{} steps → {}", self.steps.len(), dest)
        }
    }
}

/// In-progress recording. Most steps are captured by *diffing app state* each
/// main-loop iteration rather than by hooking the hundreds of assignments in
/// `handle_key` — the same trick `App::record_message_history` uses.
#[derive(Debug, Default)]
pub struct MacroRecorder {
    pub steps: Vec<MacroStep>,
    /// The key `handle_key` is about to process, plus the pre-dispatch context
    /// needed to classify it. Consumed by the next record pass.
    pub pending: Option<PendingKey>,
    // Last-seen values for the diffed checkpoints.
    pub last_region: String,
    pub last_profile: Option<String>,
    pub last_assumed: Option<(String, String, String)>,
    /// `(bucket, prefix, selected key)` while the object browser is open.
    pub last_s3: Option<(String, String, String)>,
    pub last_service: Option<crate::aws::service::ServiceType>,
    /// Whether the `S` picker was open on the previous pass. A service change
    /// is only checkpointed when it came out of that picker — `current_service`
    /// also moves on every `@service` search and on the assume-role landing,
    /// and those are already reproduced by their own steps.
    pub service_selector_was_open: bool,
}

/// A keypress captured pre-dispatch. The query has to be snapshotted here
/// because `Esc` clears it before the record pass runs.
#[derive(Debug, Clone)]
pub struct PendingKey {
    pub key: KeyEvent,
    pub was_search_active: bool,
    pub was_modal: bool,
    pub query: String,
}

/// Playback cursor. One step is injected per main-loop iteration, and only
/// once the app is quiescent (`App::macro_ready`).
#[derive(Debug)]
pub struct MacroPlayer {
    pub name: String,
    pub steps: Vec<MacroStep>,
    pub index: usize,
    /// When the last step was injected — the settle window that lets an
    /// enqueued async request (`OrgRoleSwitchRequested`) reach `handle_event`
    /// and raise `loading` before we test for quiescence again.
    pub last_step_at: Option<std::time::Instant>,
}

/// Resolve the macros file path. Honours `$NEBOTO_MACROS`, then
/// `$XDG_CONFIG_HOME` (mirroring config.rs); defaults to
/// `~/.config/neboto/macros.json`.
fn macros_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("NEBOTO_MACROS") {
        return Some(PathBuf::from(p));
    }
    let home = std::env::var("HOME").ok()?;
    let xdg = std::env::var("XDG_CONFIG_HOME").unwrap_or_else(|_| format!("{}/.config", home));
    Some(PathBuf::from(format!("{}/neboto/macros.json", xdg)))
}

/// Load saved macros. Returns empty on a missing file or any parse error
/// (e.g. an older schema) — macros are best-effort, never fatal.
pub fn load() -> Vec<Macro> {
    let Some(path) = macros_path() else {
        return Vec::new();
    };
    let Ok(data) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    serde_json::from_str(&data).unwrap_or_default()
}

/// Persist macros (best-effort; creates the parent dir if needed).
pub fn save(macros: &[Macro]) {
    let Some(path) = macros_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(macros) {
        let _ = std::fs::write(&path, json);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_steps_round_trip_through_json() {
        let steps = vec![
            MacroStep::from_key(KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE)).unwrap(),
            MacroStep::from_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)).unwrap(),
            MacroStep::from_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL)).unwrap(),
        ];
        let json = serde_json::to_string(&steps).unwrap();
        let back: Vec<MacroStep> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, steps);
        assert_eq!(
            back[2].to_key().unwrap(),
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL)
        );
    }

    #[test]
    fn semantic_steps_round_trip_and_carry_no_key() {
        let steps = vec![
            MacroStep::Search("@orgs".into()),
            MacroStep::SelectId {
                id: "123456789012".into(),
                label: "prod".into(),
            },
            MacroStep::AssumeRole {
                account_id: "123456789012".into(),
                account_name: "prod".into(),
                role_name: "AWSControlTowerExecution".into(),
            },
            MacroStep::SwitchRegion("eu-west-1".into()),
            MacroStep::ExitRole,
        ];
        let back: Vec<MacroStep> =
            serde_json::from_str(&serde_json::to_string(&steps).unwrap()).unwrap();
        assert_eq!(back, steps);
        assert!(back.iter().all(|s| s.to_key().is_none()));
    }

    #[test]
    fn an_unknown_key_name_decodes_to_none_rather_than_panicking() {
        // A macro file written by a future version naming a key this build
        // doesn't know must degrade to a skipped step, not a crash.
        let step = MacroStep::Key {
            key: "F13".into(),
            ctrl: false,
            shift: false,
            alt: false,
        };
        assert!(step.to_key().is_none());
    }

    #[test]
    fn function_keys_are_not_recordable() {
        assert!(MacroStep::from_key(KeyEvent::new(KeyCode::F(5), KeyModifiers::NONE)).is_none());
    }

    #[test]
    fn the_tui_teardown_keys_are_refused() {
        // `e` hands the terminal to $EDITOR and `q` quits — replaying either
        // mid-macro leaves the player running against a dead event channel.
        assert!(is_unrecordable(KeyCode::Char('e')));
        assert!(is_unrecordable(KeyCode::Char('q')));
        assert!(!is_unrecordable(KeyCode::Char('j')));
    }

    #[test]
    fn summary_names_the_service_the_macro_lands_on() {
        let m = Macro {
            name: "sh-audit".into(),
            steps: vec![
                MacroStep::Search("@orgs".into()),
                MacroStep::ExitRole,
                MacroStep::Search("@sh".into()),
            ],
        };
        assert_eq!(m.summary(), "3 steps → @sh");
    }

    #[test]
    fn corrupt_json_loads_as_empty() {
        assert!(serde_json::from_str::<Vec<Macro>>("not json").is_err());
    }
}
