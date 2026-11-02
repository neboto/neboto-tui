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

/// Load saved bookmarks. Returns empty on a missing file or any parse error
/// (e.g. an older schema) — bookmarks are best-effort, never fatal.
pub fn load() -> Vec<NavLocation> {
    let Some(path) = bookmarks_path() else {
        return Vec::new();
    };
    let Ok(data) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    serde_json::from_str(&data).unwrap_or_default()
}

/// Persist bookmarks (best-effort; creates the parent dir if needed).
pub fn save(bookmarks: &[NavLocation]) {
    let Some(path) = bookmarks_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(bookmarks) {
        let _ = write_private(&path, json.as_bytes());
    }
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
        assert!(serde_json::from_str::<Vec<NavLocation>>("not json").is_err());
    }
}
