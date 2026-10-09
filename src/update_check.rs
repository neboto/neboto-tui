//! "A newer neboto is out" — a once-a-day check against the latest GitHub
//! Release, surfaced as a dim `↑ vX.Y.Z` chip on the service strip.
//!
//! The only request neboto makes that isn't to AWS, so it is easy to turn
//! off (config `update_check = false`, `--no-update-check`,
//! `NEBOTO_NO_UPDATE_CHECK=1`) and stays off by itself under `--demo`, in CI,
//! and for the headless subcommands (`main` never calls it for those).
//!
//! Best-effort throughout, like `bookmarks.rs`: the state file lives in the
//! XDG cache dir, a missing or corrupt one just means "check again", and a
//! failed request (offline, proxy, rate limit) never reaches the status bar.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The releases endpoint. `/latest` never returns drafts or pre-releases, so
/// an `-rc` tag can't announce itself.
const LATEST_URL: &str = "https://api.github.com/repos/neboto/neboto-tui/releases/latest";

/// Where the chip's click points people.
pub const RELEASES_URL: &str = "https://github.com/neboto/neboto-tui/releases/latest";

/// The upgrade command the chip copies — the README's primary install path.
/// neboto can't tell how it was installed, so binstall / `cargo install`
/// users get the release page alongside it.
pub const UPGRADE_COMMAND: &str =
    "curl -fsSL https://raw.githubusercontent.com/neboto/neboto-tui/main/install.sh | sh";

/// How long a check's answer is trusted. Unauthenticated GitHub API calls
/// allow 60/hour per IP; one a day per machine is nowhere near that.
const CHECK_INTERVAL_SECS: u64 = 24 * 60 * 60;

/// A slow network must not leave a thread hanging for the whole session.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(3);

/// The version this binary was built as.
pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// What the cache file holds.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdateState {
    /// Unix seconds of the last successful check.
    pub checked_at: u64,
    /// The latest release's version, without the `v`.
    pub latest: Option<String>,
    /// A version whose chip was clicked — not announced again.
    pub dismissed: Option<String>,
}

impl UpdateState {
    /// Whether the cached answer is too old to trust (or there isn't one).
    /// A clock that went backwards counts as stale.
    pub fn is_stale(&self, now: u64) -> bool {
        now < self.checked_at || now - self.checked_at >= CHECK_INTERVAL_SECS
    }

    /// The version to announce, if the cached latest is newer than `current`
    /// and hasn't been dismissed.
    pub fn announce(&self, current: &str) -> Option<String> {
        let latest = self.latest.as_deref()?;
        if self.dismissed.as_deref() == Some(latest) || !is_newer(latest, current) {
            return None;
        }
        Some(latest.to_string())
    }
}

/// Whether the check may run at all. `config_opt_in` is the config value
/// after the CLI overlay (`None` = default on).
pub fn enabled(config_opt_in: Option<bool>) -> bool {
    config_opt_in != Some(false)
        && std::env::var_os("NEBOTO_NO_UPDATE_CHECK").is_none_or(|v| v.is_empty() || v == "0")
        && std::env::var_os("CI").is_none()
        && !crate::demo::enabled()
}

/// `$NEBOTO_UPDATE_STATE`, then `$XDG_CACHE_HOME/neboto/update-check.json`
/// (default `~/.cache/…`).
fn state_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("NEBOTO_UPDATE_STATE") {
        return Some(PathBuf::from(p));
    }
    let home = std::env::var("HOME").ok()?;
    let xdg = std::env::var("XDG_CACHE_HOME").unwrap_or_else(|_| format!("{}/.cache", home));
    Some(PathBuf::from(format!("{}/neboto/update-check.json", xdg)))
}

pub fn load() -> UpdateState {
    state_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save(state: &UpdateState) {
    let Some(path) = state_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(state) {
        let _ = std::fs::write(&path, json);
    }
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Ask GitHub for the latest release's version. Blocking — call it from
/// `spawn_blocking`.
pub fn fetch_latest() -> Result<String, String> {
    #[derive(Deserialize)]
    struct Release {
        tag_name: String,
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(REQUEST_TIMEOUT))
        .build()
        .into();
    let mut resp = agent
        .get(LATEST_URL)
        // GitHub rejects requests without a User-Agent.
        .header("User-Agent", &format!("neboto/{}", current_version()))
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| e.to_string())?;
    let body = resp.body_mut().read_to_string().map_err(|e| e.to_string())?;
    let release: Release = serde_json::from_str(&body).map_err(|e| e.to_string())?;
    let version = release.tag_name.trim_start_matches('v').to_string();
    parse(&version).ok_or_else(|| format!("unrecognised release tag {}", release.tag_name))?;
    Ok(version)
}

/// `X.Y.Z[-pre][+build]` → `((X, Y, Z), has_pre)`. Anything else is `None`.
fn parse(v: &str) -> Option<((u64, u64, u64), bool)> {
    let v = v.trim().trim_start_matches('v');
    let v = v.split('+').next()?;
    let (core, pre) = match v.split_once('-') {
        Some((core, _)) => (core, true),
        None => (v, false),
    };
    let mut it = core.split('.').map(|p| p.parse::<u64>().ok());
    let triple = (it.next()??, it.next()??, it.next()??);
    if it.next().is_some() {
        return None;
    }
    Some((triple, pre))
}

/// Whether `latest` is a release strictly newer than `current`. A pre-release
/// `latest` never counts (the endpoint shouldn't return one, but a manual
/// "latest" flag could), and a local build ahead of the release stays quiet.
pub fn is_newer(latest: &str, current: &str) -> bool {
    let (Some((l, l_pre)), Some((c, c_pre))) = (parse(latest), parse(current)) else {
        return false;
    };
    if l_pre {
        return false;
    }
    // 0.4.0 is newer than 0.4.0-rc.1.
    l > c || (l == c && c_pre)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_compares_numerically() {
        assert!(is_newer("0.3.2", "0.3.1"));
        assert!(is_newer("0.10.0", "0.9.9"));
        assert!(is_newer("1.0.0", "0.99.0"));
        assert!(is_newer("v0.4.0", "0.3.1"));
        assert!(!is_newer("0.3.1", "0.3.1"));
        assert!(!is_newer("0.3.0", "0.3.1"));
    }

    #[test]
    fn prereleases_never_announce_but_release_beats_its_rc() {
        assert!(!is_newer("0.5.0-rc.1", "0.4.0"));
        assert!(is_newer("0.4.0", "0.4.0-rc.1"));
        assert!(!is_newer("0.4.0-rc.2", "0.4.0-rc.1"));
    }

    #[test]
    fn garbage_versions_never_announce() {
        assert!(!is_newer("latest", "0.3.1"));
        assert!(!is_newer("0.4", "0.3.1"));
        assert!(!is_newer("0.4.0.1", "0.3.1"));
        assert!(!is_newer("0.4.0", "dev"));
    }

    #[test]
    fn staleness_window() {
        let s = UpdateState { checked_at: 1_000_000, ..Default::default() };
        assert!(!s.is_stale(1_000_000));
        assert!(!s.is_stale(1_000_000 + CHECK_INTERVAL_SECS - 1));
        assert!(s.is_stale(1_000_000 + CHECK_INTERVAL_SECS));
        // Clock moved backwards: don't trust the stamp.
        assert!(s.is_stale(999_999));
        assert!(UpdateState::default().is_stale(now_secs()));
    }

    #[test]
    fn announce_respects_dismissal_and_version() {
        let mut s = UpdateState { latest: Some("0.4.0".into()), ..Default::default() };
        assert_eq!(s.announce("0.3.1").as_deref(), Some("0.4.0"));
        assert_eq!(s.announce("0.4.0"), None);
        s.dismissed = Some("0.4.0".into());
        assert_eq!(s.announce("0.3.1"), None);
        // A newer release than the dismissed one announces again.
        s.latest = Some("0.4.1".into());
        assert_eq!(s.announce("0.3.1").as_deref(), Some("0.4.1"));
    }

    /// Hits the real GitHub API — `cargo test live_fetch -- --ignored`.
    #[test]
    #[ignore]
    fn live_fetch_returns_a_version() {
        let v = fetch_latest().expect("GitHub reachable");
        assert!(parse(&v).is_some(), "{v}");
    }

    #[test]
    fn corrupt_or_old_state_loads_as_default() {
        assert_eq!(serde_json::from_str::<UpdateState>("{}").unwrap(), UpdateState::default());
        let s: UpdateState = serde_json::from_str(r#"{"latest":"0.4.0","extra":1}"#).unwrap();
        assert_eq!(s.latest.as_deref(), Some("0.4.0"));
    }
}
