//! Recent item-browser queries, per table, kept across sessions so `↑` in the
//! query line brings back what you ran last time. Stored as JSON next to the
//! config file, like bookmarks; best-effort, never fatal.

use std::collections::HashMap;
use std::path::PathBuf;

/// Queries kept per table, newest first.
pub const PER_TABLE: usize = 30;

/// Honours `$NEBOTO_DDB_HISTORY`, then `$XDG_CONFIG_HOME`; defaults to
/// `~/.config/neboto/ddb_history.json`. Never the real file under test.
fn history_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("NEBOTO_DDB_HISTORY") {
        return Some(PathBuf::from(p));
    }
    if cfg!(test) {
        return None;
    }
    let home = std::env::var("HOME").ok()?;
    let xdg = std::env::var("XDG_CONFIG_HOME").unwrap_or_else(|_| format!("{}/.config", home));
    Some(PathBuf::from(format!("{}/neboto/ddb_history.json", xdg)))
}

fn load_all() -> HashMap<String, Vec<String>> {
    history_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|d| serde_json::from_str(&d).ok())
        .unwrap_or_default()
}

/// The table's recent queries, newest first. Keyed by table ARN, so the same
/// name in another account or region keeps its own history.
pub fn load(table_arn: &str) -> Vec<String> {
    load_all().remove(table_arn).unwrap_or_default()
}

/// Put `query` at the front of `list`, dropping an older copy and the excess.
pub fn remember(list: &mut Vec<String>, query: &str) {
    let q = query.trim();
    if q.is_empty() {
        return;
    }
    list.retain(|x| x != q);
    list.insert(0, q.to_string());
    list.truncate(PER_TABLE);
}

/// Persist the table's list (re-reading the file first, so two neboto
/// windows on different tables don't clobber each other).
pub fn save(table_arn: &str, list: &[String]) {
    let Some(path) = history_path() else {
        return;
    };
    let mut all = load_all();
    all.insert(table_arn.to_string(), list.to_vec());
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(&all) {
        let _ = std::fs::write(&path, json);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remember_dedups_newest_first_and_caps() {
        let mut l = vec!["a".to_string(), "b".to_string()];
        remember(&mut l, " b ");
        assert_eq!(l, ["b", "a"]);
        remember(&mut l, "   ");
        assert_eq!(l.len(), 2);
        for i in 0..50 {
            remember(&mut l, &format!("q{}", i));
        }
        assert_eq!(l.len(), PER_TABLE);
        assert_eq!(l[0], "q49");
    }
}
