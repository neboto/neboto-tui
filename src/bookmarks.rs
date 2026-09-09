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
        let _ = std::fs::write(&path, json);
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
        };
        let mut value = serde_json::to_value(&loc).unwrap();
        let obj = value.as_object_mut().unwrap();
        obj.remove("details_focused");
        obj.remove("detail_section");
        let back: NavLocation = serde_json::from_value(value).unwrap();
        assert!(!back.details_focused);
        assert!(back.detail_section.is_none());
    }

    #[test]
    fn corrupt_json_loads_as_empty() {
        assert!(serde_json::from_str::<Vec<NavLocation>>("not json").is_err());
    }
}
