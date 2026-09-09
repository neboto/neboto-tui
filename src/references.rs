//! "Referenced by" — the reverse of ownership: which *other* resources
//! mention this one? Answered **from the warm caches alone** (the `@all`
//! search precedent): no fetch ever fires, and the lens says how many
//! services were searched vs. not loaded so partial coverage is explicit.
//!
//! Matching is deliberately strict. A resource's id is unambiguous, but its
//! *name* can be a common word (`web`, `default`), so both are matched as
//! **whole tokens** — the key must be bounded by non-token characters on
//! each side (`is_token_char`). ARN-shaped keys (containing `:` or `/`) are
//! specific enough to match as plain substrings, which is what lets a bare
//! role name hit inside `arn:aws:iam::…:role/my-role`.
//!
//! The row model lives here (not in the widget) because the matcher is
//! pure logic with unit tests; `ui/widgets/refs_lens.rs` renders it.

use crate::aws::resource::Resource;
use crate::aws::service::ServiceType;

/// One resource that references the selected one.
#[derive(Debug, Clone, PartialEq)]
pub struct RefRow {
    pub service: ServiceType,
    pub resource_type: String,
    pub name: String,
    pub id: String,
    /// Which field(s) carried the reference — `Security Group`, `Role ARN`,
    /// … — or `search text` when only the search blob matched.
    pub via: String,
}

/// Identifiers to search for: the id, plus the name when it differs.
pub fn lookup_keys(resource: &dyn Resource) -> Vec<String> {
    let mut keys = vec![resource.id().trim().to_string()];
    let name = resource.name().trim();
    if !name.is_empty() && name != keys[0] {
        keys.push(name.to_string());
    }
    keys.retain(|k| k.len() >= 3);
    keys
}

fn is_token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')
}

/// Does `value` mention `key` — as a whole token, or as a substring when the
/// key is ARN/path-shaped?
pub fn value_mentions(value: &str, key: &str) -> bool {
    if key.len() < 3 || value.len() < key.len() {
        return false;
    }
    if key.contains(':') || key.contains('/') {
        return value.contains(key);
    }
    let mut start = 0;
    while let Some(pos) = value[start..].find(key) {
        let i = start + pos;
        let j = i + key.len();
        let before_ok = value[..i].chars().next_back().is_none_or(|c| !is_token_char(c));
        let after_ok = value[j..].chars().next().is_none_or(|c| !is_token_char(c));
        if before_ok && after_ok {
            return true;
        }
        start = j;
    }
    false
}

/// Scan one cached list for resources referencing any of `keys`. `self_id`
/// excludes the selected resource itself (and any duplicate of it).
pub fn find_references(
    service: ServiceType,
    list: &[Box<dyn Resource>],
    keys: &[String],
    self_id: &str,
) -> Vec<RefRow> {
    let mut out = Vec::new();
    for r in list {
        if r.id() == self_id || keys.iter().any(|k| k == r.id()) {
            continue;
        }
        let mut via: Vec<String> = Vec::new();
        // Explicit links first — labelled, and the only place rich
        // split-pane types expose what they point at.
        for (label, value) in r.references() {
            if value.is_empty() || via.contains(&label) {
                continue;
            }
            if keys.iter().any(|k| value == *k || value_mentions(&value, k)) {
                via.push(label);
                if via.len() == 2 {
                    break;
                }
            }
        }
        // Tag values — `aws:cloudformation:stack-name` makes `U` on a
        // stack list its resources; an `owner`-style tag naming a resource
        // shows up too.
        if via.len() < 2 {
            for (k, v) in r.tags() {
                if keys.iter().any(|key| v == key || value_mentions(v, key)) {
                    let label = format!("Tag {}", k);
                    if !via.contains(&label) {
                        via.push(label);
                        if via.len() == 2 {
                            break;
                        }
                    }
                }
            }
        }
        for (label, value) in r.details() {
            if via.len() == 2 {
                break;
            }
            // A candidate's own name/id row is not a reference to us — a
            // same-named resource in another service would otherwise hit.
            if value == r.id() || value == r.name() {
                continue;
            }
            if keys.iter().any(|k| value_mentions(&value, k)) && !via.contains(&label) {
                via.push(label);
                if via.len() == 2 {
                    break;
                }
            }
        }
        if via.is_empty() {
            let blob = r.search_text();
            // Same own-name guard, on the blob: strip the candidate's own
            // identifiers before testing.
            let stripped = blob.replace(r.id(), " ").replace(r.name(), " ");
            if keys.iter().any(|k| value_mentions(&stripped, k)) {
                via.push("search text".to_string());
            }
        }
        if via.is_empty() {
            continue;
        }
        out.push(RefRow {
            service,
            resource_type: r.resource_type().to_string(),
            name: r.name().to_string(),
            id: r.id().to_string(),
            via: via.join(", "),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aws::resource::ResourceState;
    use std::collections::HashMap;

    /// Minimal candidate: a name, explicit references, tags, and a flat
    /// `details()` that deliberately says nothing about its links — the
    /// shape of every rich split-pane type.
    #[derive(Clone, Debug)]
    struct Mock {
        id: String,
        refs: Vec<(String, String)>,
        tags: HashMap<String, String>,
    }

    impl Resource for Mock {
        fn id(&self) -> &str {
            &self.id
        }
        fn name(&self) -> &str {
            &self.id
        }
        fn resource_type(&self) -> &str {
            "Mock"
        }
        fn state(&self) -> ResourceState {
            ResourceState::Unknown(String::new())
        }
        fn tags(&self) -> &HashMap<String, String> {
            &self.tags
        }
        fn details(&self) -> Vec<(String, String)> {
            vec![("ID".to_string(), self.id.clone())]
        }
        fn references(&self) -> Vec<(String, String)> {
            self.refs.clone()
        }
        fn clone_box(&self) -> Box<dyn Resource> {
            Box::new(self.clone())
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    fn mock(id: &str, refs: &[(&str, &str)], tags: &[(&str, &str)]) -> Box<dyn Resource> {
        Box::new(Mock {
            id: id.to_string(),
            refs: refs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            tags: tags.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        })
    }

    #[test]
    fn explicit_references_and_tags_are_found_with_labels() {
        let list = vec![
            mock("i-1", &[("Security Group", "sg-aaa"), ("Subnet", "subnet-1")], &[]),
            mock("i-2", &[("Subnet", "subnet-1")], &[("aws:cloudformation:stack-name", "net")]),
            mock("sg-aaa", &[], &[]), // the selected resource itself
        ];
        let keys = vec!["sg-aaa".to_string()];
        let rows = find_references(ServiceType::EC2, &list, &keys, "sg-aaa");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "i-1");
        assert_eq!(rows[0].via, "Security Group");

        let keys = vec!["net".to_string()];
        let rows = find_references(ServiceType::EC2, &list, &keys, "arn:stack/net");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "i-2");
        assert_eq!(rows[0].via, "Tag aws:cloudformation:stack-name");
    }

    #[test]
    fn ids_match_as_whole_tokens_only() {
        assert!(value_mentions("sg-0123abcd", "sg-0123abcd"));
        assert!(value_mentions("sg-0123abcd (web)", "sg-0123abcd"));
        assert!(value_mentions("a, sg-0123abcd, b", "sg-0123abcd"));
        // A longer id that merely starts with the key is not a match.
        assert!(!value_mentions("sg-0123abcdef", "sg-0123abcd"));
        assert!(!value_mentions("xsg-0123abcd", "sg-0123abcd"));
    }

    #[test]
    fn short_names_need_boundaries() {
        assert!(value_mentions("service web (2 tasks)", "web"));
        assert!(!value_mentions("webserver", "web"));
        assert!(!value_mentions("my-web", "web"));
        // Too short to be meaningful.
        assert!(!value_mentions("a b c", "ab"));
    }

    #[test]
    fn arn_keys_match_as_substrings_and_names_hit_inside_arns() {
        let arn = "arn:aws:iam::123456789012:role/my-role";
        assert!(value_mentions(&format!("Role: {arn}"), arn));
        assert!(value_mentions(arn, "my-role"));
        assert!(!value_mentions("arn:aws:iam::123456789012:role/my-role-2", "my-role"));
    }
}
