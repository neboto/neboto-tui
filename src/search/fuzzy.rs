use crate::aws::resource::Resource;
use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher as FuzzyMatcherTrait;

pub struct FuzzyMatcher {
    matcher: SkimMatcherV2,
}

impl FuzzyMatcher {
    pub fn new() -> Self {
        Self {
            matcher: SkimMatcherV2::default(),
        }
    }

    /// Filter resources by query and return sorted (index, score) pairs
    pub fn filter_resources(
        &self,
        query: &str,
        resources: &[Box<dyn Resource>],
    ) -> Vec<(usize, i64)> {
        if query.is_empty() {
            // No query, return all resources with score 0
            return (0..resources.len()).map(|i| (i, 0)).collect();
        }

        let sanitized_query = Self::sanitize(query);
        let min_score = Self::min_score(sanitized_query);

        let mut matches: Vec<(usize, i64)> = resources
            .iter()
            .enumerate()
            .filter_map(|(idx, resource)| {
                // An exact id or name always matches, first: that's what a
                // jump, bookmark or `@all` / `@audit` result puts in the
                // query, and `resolve_pending_jump` only looks at what the
                // filter kept. A type whose `search_text` omits its own id
                // (IAM's synthetic account row) was otherwise unreachable.
                let q = sanitized_query.trim();
                if resource.id() == q || resource.name() == q {
                    return Some((idx, i64::MAX));
                }
                let text = resource.search_text();
                self.matcher
                    .fuzzy_match(&text, sanitized_query)
                    .filter(|&score| score >= min_score)
                    .map(|score| (idx, score))
            })
            .collect();

        // Sort by score descending (highest score first)
        matches.sort_by_key(|x| std::cmp::Reverse(x.1));

        matches
    }

    /// Filter plain strings (picker modals) with the same scoring and
    /// threshold as `filter_resources`. Returns sorted (index, score) pairs;
    /// an exact (case-insensitive) match ranks first, ties keep input order.
    pub fn filter_strings<S: AsRef<str>>(&self, query: &str, items: &[S]) -> Vec<(usize, i64)> {
        if query.is_empty() {
            return (0..items.len()).map(|i| (i, 0)).collect();
        }

        let sanitized_query = Self::sanitize(query);
        let min_score = Self::min_score(sanitized_query);
        let q = sanitized_query.trim();

        let mut matches: Vec<(usize, i64)> = items
            .iter()
            .enumerate()
            .filter_map(|(idx, item)| {
                let text = item.as_ref();
                if text.eq_ignore_ascii_case(q) {
                    return Some((idx, i64::MAX));
                }
                self.matcher
                    .fuzzy_match(text, sanitized_query)
                    .filter(|&score| score >= min_score)
                    .map(|score| (idx, score))
            })
            .collect();

        matches.sort_by_key(|x| std::cmp::Reverse(x.1));
        matches
    }

    /// Remove trailing backslashes that break fuzzy matching.
    fn sanitize(query: &str) -> &str {
        query.trim_end_matches('\\')
    }

    /// Minimum score scales with query length to cut off scattered character
    /// coincidences (e.g. "1a" matching "us-east-1b ... available" via CIDR + state).
    fn min_score(sanitized_query: &str) -> i64 {
        sanitized_query.len() as i64 * 10
    }
}

impl Default for FuzzyMatcher {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aws::resource::{Resource, ResourceState};
    use std::collections::HashMap;

    #[derive(Clone, Debug)]
    struct MockResource {
        id: String,
        name: String,
        tags: HashMap<String, String>,
    }

    impl Resource for MockResource {
        fn id(&self) -> &str {
            &self.id
        }

        fn name(&self) -> &str {
            &self.name
        }

        fn resource_type(&self) -> &str {
            "Mock"
        }

        fn state(&self) -> ResourceState {
            ResourceState::Running
        }

        fn tags(&self) -> &HashMap<String, String> {
            &self.tags
        }

        fn details(&self) -> Vec<(String, String)> {
            vec![]
        }

        fn as_any(&self) -> &dyn std::any::Any {
            self
        }

        fn clone_box(&self) -> Box<dyn Resource> {
            Box::new(self.clone())
        }
    }

    /// A resource whose search text leaves its id out.
    #[derive(Clone, Debug)]
    struct Opaque(MockResource);

    impl Resource for Opaque {
        fn id(&self) -> &str {
            self.0.id()
        }
        fn name(&self) -> &str {
            self.0.name()
        }
        fn resource_type(&self) -> &str {
            "Opaque"
        }
        fn state(&self) -> ResourceState {
            ResourceState::stateless()
        }
        fn tags(&self) -> &HashMap<String, String> {
            self.0.tags()
        }
        fn search_text(&self) -> String {
            "account settings password policy".to_string()
        }
        fn details(&self) -> Vec<(String, String)> {
            vec![]
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
        fn clone_box(&self) -> Box<dyn Resource> {
            Box::new(self.clone())
        }
    }

    #[test]
    fn an_exact_id_or_name_always_matches_first() {
        let matcher = FuzzyMatcher::new();
        let resources: Vec<Box<dyn Resource>> = vec![
            Box::new(MockResource {
                id: "i-1".to_string(),
                name: "iam-account-settings-ish".to_string(),
                tags: HashMap::new(),
            }),
            Box::new(Opaque(MockResource {
                id: "iam-account-settings".to_string(),
                name: "acme-prod".to_string(),
                tags: HashMap::new(),
            })),
        ];
        let by_id = matcher.filter_resources("iam-account-settings", &resources);
        assert_eq!(by_id.first().map(|m| m.0), Some(1), "{by_id:?}");
        let by_name = matcher.filter_resources("acme-prod", &resources);
        assert_eq!(by_name.first().map(|m| m.0), Some(1), "{by_name:?}");
    }

    #[test]
    fn test_fuzzy_matching() {
        let matcher = FuzzyMatcher::new();

        let resources: Vec<Box<dyn Resource>> = vec![
            Box::new(MockResource {
                id: "i-1234".to_string(),
                name: "web-server".to_string(),
                tags: HashMap::new(),
            }),
            Box::new(MockResource {
                id: "i-5678".to_string(),
                name: "database-server".to_string(),
                tags: HashMap::new(),
            }),
            Box::new(MockResource {
                id: "i-9012".to_string(),
                name: "cache-server".to_string(),
                tags: HashMap::new(),
            }),
        ];

        // Test exact match
        let matches = matcher.filter_resources("web", &resources);
        assert!(!matches.is_empty());
        assert_eq!(matches[0].0, 0); // web-server should be first

        // Test fuzzy match
        let matches = matcher.filter_resources("srv", &resources);
        assert_eq!(matches.len(), 3); // All have "server"

        // Test empty query
        let matches = matcher.filter_resources("", &resources);
        assert_eq!(matches.len(), 3); // All resources returned
    }

    #[test]
    fn test_backslash_sanitization() {
        let matcher = FuzzyMatcher::new();

        let resources: Vec<Box<dyn Resource>> = vec![
            Box::new(MockResource {
                id: "vpc-1".to_string(),
                name: "172.31.0.0/16".to_string(),
                tags: HashMap::new(),
            }),
            Box::new(MockResource {
                id: "subnet-1".to_string(),
                name: "10.0.1.0/24".to_string(),
                tags: HashMap::new(),
            }),
        ];

        // Test that trailing backslashes are handled correctly
        let matches_without = matcher.filter_resources("172.31", &resources);
        let matches_with = matcher.filter_resources("172.31\\", &resources);

        // Should find the same results
        assert_eq!(matches_without.len(), matches_with.len());
        assert_eq!(matches_without.len(), 1);
        assert_eq!(matches_without[0].0, matches_with[0].0);
    }

    #[test]
    fn filter_strings_ranks_fuzzy_matches_and_floats_exact_first() {
        let matcher = FuzzyMatcher::new();
        let items = vec![
            "prod-admin".to_string(),
            "staging".to_string(),
            "prod".to_string(),
            "dev-sandbox".to_string(),
        ];

        let all = matcher.filter_strings("", &items);
        assert_eq!(all.iter().map(|m| m.0).collect::<Vec<_>>(), vec![0, 1, 2, 3]);

        // Exact name beats the longer fuzzy hit.
        let exact = matcher.filter_strings("PROD", &items);
        assert_eq!(exact.first().map(|m| m.0), Some(2), "{exact:?}");

        // Non-contiguous characters match (a plain substring filter would not).
        let fuzzy = matcher.filter_strings("pdadm", &items);
        assert_eq!(fuzzy.iter().map(|m| m.0).collect::<Vec<_>>(), vec![0]);

        assert!(matcher.filter_strings("zzz", &items).is_empty());
    }
}
