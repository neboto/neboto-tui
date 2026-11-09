//! Persistent, user-curated bookmarks — saved navigation locations the user can
//! jump back to across sessions (unlike the ephemeral, auto-pruned nav history).
//! Stored as JSON next to the config file. A `NavLocation` already captures
//! everything needed to navigate back (service + sub-tab + selected id + label).

use crate::app::NavLocation;
use std::path::PathBuf;

/// Resolve the bookmarks file path. Honours `$NEBOTO_BOOKMARKS`, then
/// `$XDG_CONFIG_HOME` (mirroring config.rs); defaults to
/// `~/.config/neboto/bookmarks.json`.
fn bookmarks_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("NEBOTO_BOOKMARKS") {
        return Some(PathBuf::from(p));
    }
    let home = std::env::var("HOME").ok()?;
    let xdg = std::env::var("XDG_CONFIG_HOME").unwrap_or_else(|_| format!("{}/.config", home));
    Some(PathBuf::from(format!("{}/neboto/bookmarks.json", xdg)))
}

/// Load saved bookmarks. Returns empty on a missing file or a file that
/// isn't a JSON list — bookmarks are best-effort, never fatal. An entry this
/// build can't read (one saved by a newer neboto, e.g. on a sub-tab this
/// build doesn't know) is skipped, not the whole file; `save` keeps it.
pub fn load() -> Vec<NavLocation> {
    let Some(path) = bookmarks_path() else {
        return Vec::new();
    };
    let Ok(data) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    parse(&data).0
}

/// The entries this build reads, and the ones it can't, verbatim.
fn parse(data: &str) -> (Vec<NavLocation>, Vec<serde_json::Value>) {
    let entries: Vec<serde_json::Value> = serde_json::from_str(data).unwrap_or_default();
    let mut read = Vec::new();
    let mut unread = Vec::new();
    for entry in entries {
        match serde_json::from_value::<NavLocation>(entry.clone()) {
            Ok(loc) => read.push(loc),
            Err(_) => unread.push(entry),
        }
    }
    (read, unread)
}

/// Persist bookmarks (best-effort; creates the parent dir if needed). The
/// entries already on disk that this build can't read are written back
/// after them, so going back a version doesn't erase the newer one's
/// bookmarks.
pub fn save(bookmarks: &[NavLocation]) {
    let Some(path) = bookmarks_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let on_disk = std::fs::read_to_string(&path).unwrap_or_default();
    if let Some(json) = merged(bookmarks, &on_disk) {
        let _ = write_private(&path, json.as_bytes());
    }
}

/// `bookmarks`, then `on_disk`'s unreadable entries, as the file's JSON.
fn merged(bookmarks: &[NavLocation], on_disk: &str) -> Option<String> {
    let mut entries: Vec<serde_json::Value> =
        bookmarks.iter().filter_map(|b| serde_json::to_value(b).ok()).collect();
    entries.extend(parse(on_disk).1);
    serde_json::to_string_pretty(&entries).ok()
}

/// Write `data` readable by the owner only. Bookmarks name accounts, ARNs,
/// profiles and roles — not secret, but a map of your estate that other
/// users on a shared host have no business reading. Tightens a file an
/// older version left at the umask default, too.
fn write_private(path: &std::path::Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        opts.mode(0o600);
        let mut file = opts.open(path)?;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        file.write_all(data)
    }
    #[cfg(not(unix))]
    {
        opts.open(path)?.write_all(data)
    }
}

#[cfg(test)]
mod tests {
    use crate::app::{EcsView, JumpView, NavLocation};
    use crate::aws::service::ServiceType;

    #[test]
    fn navlocation_roundtrips_through_json() {
        let loc = NavLocation {
            service: ServiceType::ECS,
            view: JumpView::Ecs(EcsView::Tasks),
            query: String::new(),
            selected_id: Some("arn:aws:ecs:r:a:task/cl/abc".to_string()),
            label: "ECS · web (abc)".to_string(),
            s3_object_path: None,
            details_focused: true,
            detail_section: Some("Networking".to_string()),
            region: Some("ap-southeast-2".to_string()),
            credentials: Some(crate::app::NavCredentials {
                profile: Some("mgmt".to_string()),
                assumed: Some(crate::app::NavAssumedRole {
                    account_id: "111122223333".to_string(),
                    account_name: "prod".to_string(),
                    role_name: "OrganizationAccountAccessRole".to_string(),
                }),
                account: Some("111122223333".to_string()),
                alias: Some("prod".to_string()),
            }),
            lens: None,
        };
        let json = serde_json::to_string(std::slice::from_ref(&loc)).unwrap();
        let back: Vec<NavLocation> = serde_json::from_str(&json).unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].service, ServiceType::ECS);
        assert!(matches!(back[0].view, JumpView::Ecs(EcsView::Tasks)));
        assert_eq!(back[0].selected_id.as_deref(), Some("arn:aws:ecs:r:a:task/cl/abc"));
        assert_eq!(back[0].label, "ECS · web (abc)");
        assert!(back[0].details_focused);
        assert_eq!(back[0].detail_section.as_deref(), Some("Networking"));
        assert_eq!(back[0].region.as_deref(), Some("ap-southeast-2"));
        let creds = back[0].credentials.as_ref().expect("credentials survive");
        assert_eq!(creds.profile.as_deref(), Some("mgmt"));
        assert_eq!(creds.assumed.as_ref().map(|a| a.account_id.as_str()), Some("111122223333"));
    }

    #[test]
    fn navlocation_without_detail_depth_fields_defaults() {
        // Bookmark files written before the fields existed must still load.
        let loc = NavLocation {
            service: ServiceType::ECS,
            view: JumpView::None,
            query: String::new(),
            selected_id: None,
            label: "ECS".to_string(),
            s3_object_path: None,
            details_focused: true,
            detail_section: Some("Tags".to_string()),
            region: Some("us-east-1".to_string()),
            credentials: Some(crate::app::NavCredentials { profile: Some("dev".to_string()), ..Default::default() }),
            lens: None,
        };
        let mut value = serde_json::to_value(&loc).unwrap();
        let obj = value.as_object_mut().unwrap();
        obj.remove("details_focused");
        obj.remove("detail_section");
        obj.remove("region");
        obj.remove("credentials");
        let back: NavLocation = serde_json::from_value(value).unwrap();
        assert!(!back.details_focused);
        assert!(back.detail_section.is_none());
        assert!(back.region.is_none(), "an old bookmark restores in the current region");
        assert!(back.credentials.is_none(), "…and under the current credentials");
    }

    #[cfg(unix)]
    #[test]
    fn saved_file_is_owner_only_even_over_an_old_world_readable_one() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("neboto-bm-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bookmarks.json");
        std::fs::write(&path, "[]").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        super::write_private(&path, b"[]").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn corrupt_json_loads_as_empty() {
        assert!(super::parse("not json").0.is_empty());
        assert!(super::parse(r#"{"not": "a list"}"#).0.is_empty());
    }

    fn bookmark(label: &str) -> NavLocation {
        NavLocation {
            service: ServiceType::ECS,
            view: JumpView::Ecs(EcsView::Tasks),
            query: String::new(),
            selected_id: None,
            label: label.to_string(),
            s3_object_path: None,
            details_focused: false,
            detail_section: None,
            region: None,
            credentials: None,
            lens: None,
        }
    }

    /// A file holding a bookmark from a newer neboto — here a sub-tab this
    /// build has no variant for.
    fn file_with_a_newer_entry() -> String {
        let mut newer = serde_json::to_value(bookmark("newer")).unwrap();
        newer["view"] = serde_json::json!({ "Quantum": "Circuits" });
        serde_json::to_string(&vec![serde_json::to_value(bookmark("web")).unwrap(), newer]).unwrap()
    }

    #[test]
    fn an_unreadable_entry_skips_only_itself() {
        let (read, unread) = super::parse(&file_with_a_newer_entry());
        assert_eq!(read.iter().map(|b| b.label.as_str()).collect::<Vec<_>>(), ["web"]);
        assert_eq!(unread.len(), 1);
    }

    #[test]
    fn saving_keeps_the_entries_it_cannot_read() {
        let on_disk = file_with_a_newer_entry();
        let (mut read, _) = super::parse(&on_disk);
        read.push(bookmark("api"));
        let saved = super::merged(&read, &on_disk).unwrap();
        let (read, unread) = super::parse(&saved);
        assert_eq!(read.iter().map(|b| b.label.as_str()).collect::<Vec<_>>(), ["web", "api"]);
        assert_eq!(unread.len(), 1);
        assert_eq!(unread[0]["view"], serde_json::json!({ "Quantum": "Circuits" }));
        assert_eq!(unread[0]["label"], "newer");

        // A deleted bookmark this build can read stays deleted.
        let saved = super::merged(&read[1..], &saved).unwrap();
        let (read, unread) = super::parse(&saved);
        assert_eq!(read.iter().map(|b| b.label.as_str()).collect::<Vec<_>>(), ["api"]);
        assert_eq!(unread.len(), 1);
    }
}
