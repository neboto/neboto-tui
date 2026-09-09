//! Ownership lens — "who owns this resource?", answered from tags alone.
//!
//! v1 is deliberately zero-API: everything here reads `Resource::tags()`,
//! which every type already carries. Three signals, in display order:
//!
//! - **CloudFormation**: the `aws:cloudformation:stack-name` /
//!   `:logical-id` tags CFN stamps on every resource it creates. The
//!   stack-name tag row also gets a jump classifier arm in
//!   `details_pane.rs` (→ the owning stack), and the `:stack-id` tag's ARN
//!   value already jumped via the generic `:stack/` rule.
//! - **ManagedBy**: the `ManagedBy`/`managed-by`/`managed_by` convention
//!   (values like `terraform`, `cdk`, `pulumi`). Matched case-insensitively
//!   on the key; the value is shown as written. The key list is
//!   configurable (`managed_by_tags`) for shops using a different marker.
//! - **Owner tags**: the first populated tag from the configured
//!   `owner_tags` list (config, default `owner`/`team`), matched
//!   case-insensitively so `Owner`/`Team` variants hit too.
//!
//! Rendered as the dim ribbon on the detail pane's bottom border
//! (`render_ownership_ribbon` in `details_pane.rs`) — one central hook, so
//! no per-pane wiring. The planned change-timeline lens will reuse
//! `resource_ownership` to pick which stack's events to merge.

use std::collections::HashMap;

/// Default `owner_tags` when the config doesn't set any. Case-insensitive,
/// so these also cover `Owner` / `Team`.
pub const DEFAULT_OWNER_TAGS: &[&str] = &["owner", "team"];

/// Default `managed_by_tags` when the config doesn't set any.
/// Case-insensitive, so `managedby` also covers `ManagedBy`.
pub const DEFAULT_MANAGED_BY_TAGS: &[&str] = &["managedby", "managed-by", "managed_by"];

const CFN_STACK_NAME_TAG: &str = "aws:cloudformation:stack-name";
const CFN_LOGICAL_ID_TAG: &str = "aws:cloudformation:logical-id";

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Ownership {
    /// Owning CloudFormation stack, from `aws:cloudformation:stack-name`.
    pub stack_name: Option<String>,
    /// The resource's logical id within that stack.
    pub logical_id: Option<String>,
    /// `ManagedBy`-convention value (`terraform`, `cdk`, …), as written.
    pub managed_by: Option<String>,
    /// First configured owner tag that is populated: `(display key, value)`.
    /// The display key is the *configured* name lowercased, not the tag's
    /// own casing, so the ribbon reads consistently across resources.
    pub owner: Option<(String, String)>,
}

impl Ownership {
    pub fn is_empty(&self) -> bool {
        self.stack_name.is_none() && self.managed_by.is_none() && self.owner.is_none()
    }

    /// One-line summary for the detail-pane ribbon:
    /// `stack my-stack (AppServer) · terraform · team payments`.
    pub fn summary(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if let Some(stack) = &self.stack_name {
            match &self.logical_id {
                Some(lid) => parts.push(format!("stack {} ({})", stack, lid)),
                None => parts.push(format!("stack {}", stack)),
            }
        }
        if let Some(mb) = &self.managed_by {
            parts.push(mb.clone());
        }
        if let Some((key, value)) = &self.owner {
            parts.push(format!("{} {}", key, value));
        }
        parts.join(" · ")
    }
}

/// Resolve ownership from a resource's tag map. `owner_tags` and
/// `managed_by_tags` are the configured (or default) tag-key lists, each
/// tried in order.
pub fn resource_ownership(
    tags: &HashMap<String, String>,
    owner_tags: &[String],
    managed_by_tags: &[String],
) -> Ownership {
    let non_empty = |v: &String| {
        let t = v.trim();
        (!t.is_empty()).then(|| t.to_string())
    };
    // Case-insensitive lookup; tags is small, a scan per key is fine.
    let find_ci = |want: &str| {
        tags.iter()
            .find(|(k, _)| k.trim().eq_ignore_ascii_case(want))
            .and_then(|(_, v)| non_empty(v))
    };

    let stack_name = find_ci(CFN_STACK_NAME_TAG);
    // A logical id without a stack is meaningless — only read it alongside.
    let logical_id = stack_name
        .is_some()
        .then(|| find_ci(CFN_LOGICAL_ID_TAG))
        .flatten();
    let managed_by = managed_by_tags.iter().find_map(|k| find_ci(k));
    let owner = owner_tags.iter().find_map(|k| {
        find_ci(k).map(|v| (k.trim().to_ascii_lowercase(), v))
    });

    Ownership {
        stack_name,
        logical_id,
        managed_by,
        owner,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn default_owner_tags() -> Vec<String> {
        DEFAULT_OWNER_TAGS.iter().map(|s| s.to_string()).collect()
    }

    fn default_managed_by_tags() -> Vec<String> {
        DEFAULT_MANAGED_BY_TAGS.iter().map(|s| s.to_string()).collect()
    }

    fn resolve(tags: &HashMap<String, String>, owner_tags: &[String]) -> Ownership {
        resource_ownership(tags, owner_tags, &default_managed_by_tags())
    }

    #[test]
    fn empty_tags_yield_empty_ownership() {
        let o = resolve(&tags(&[]), &default_owner_tags());
        assert!(o.is_empty());
        assert_eq!(o.summary(), "");
    }

    #[test]
    fn cfn_tags_resolve_stack_and_logical_id() {
        let o = resolve(
            &tags(&[
                ("aws:cloudformation:stack-name", "my-stack"),
                ("aws:cloudformation:logical-id", "AppServer"),
                ("aws:cloudformation:stack-id", "arn:aws:cloudformation:eu-west-1:1:stack/my-stack/guid"),
            ]),
            &default_owner_tags(),
        );
        assert_eq!(o.stack_name.as_deref(), Some("my-stack"));
        assert_eq!(o.logical_id.as_deref(), Some("AppServer"));
        assert_eq!(o.summary(), "stack my-stack (AppServer)");
    }

    #[test]
    fn logical_id_without_stack_is_ignored() {
        let o = resolve(
            &tags(&[("aws:cloudformation:logical-id", "AppServer")]),
            &default_owner_tags(),
        );
        assert!(o.is_empty());
    }

    #[test]
    fn managed_by_matches_key_variants_case_insensitively() {
        for key in ["ManagedBy", "managed-by", "MANAGED_BY"] {
            let o = resolve(
                &tags(&[(key, "terraform")]),
                &default_owner_tags(),
            );
            assert_eq!(o.managed_by.as_deref(), Some("terraform"), "key {key}");
        }
    }

    #[test]
    fn managed_by_tags_are_configurable() {
        let t = tags(&[("provisioner", "terraform"), ("ManagedBy", "cdk")]);
        // Custom list replaces the defaults — `ManagedBy` no longer matches.
        let o = resource_ownership(
            &t,
            &default_owner_tags(),
            &["Provisioner".to_string()],
        );
        assert_eq!(o.managed_by.as_deref(), Some("terraform"));
    }

    #[test]
    fn owner_tags_tried_in_configured_order_case_insensitive() {
        let t = tags(&[("Team", "payments"), ("Owner", "alice")]);
        // Default order: owner first.
        let o = resolve(&t, &default_owner_tags());
        assert_eq!(o.owner, Some(("owner".to_string(), "alice".to_string())));
        // Custom order: team first, display key lowercased from config.
        let o = resolve(&t, &["Team".to_string(), "owner".to_string()]);
        assert_eq!(o.owner, Some(("team".to_string(), "payments".to_string())));
    }

    #[test]
    fn empty_and_whitespace_values_are_skipped() {
        let o = resolve(
            &tags(&[("owner", "  "), ("team", "payments")]),
            &default_owner_tags(),
        );
        assert_eq!(o.owner, Some(("team".to_string(), "payments".to_string())));
    }

    #[test]
    fn full_summary_reads_in_order() {
        let o = resolve(
            &tags(&[
                ("aws:cloudformation:stack-name", "net"),
                ("ManagedBy", "cdk"),
                ("team", "platform"),
            ]),
            &default_owner_tags(),
        );
        assert_eq!(o.summary(), "stack net · cdk · team platform");
    }
}
