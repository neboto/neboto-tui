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

        // Sanitize query: remove trailing backslashes that break fuzzy matching
        let sanitized_query = query.trim_end_matches('\\');

        // Minimum score scales with query length to cut off scattered character
        // coincidences (e.g. "1a" matching "us-east-1b ... available" via CIDR + state).
        let min_score = sanitized_query.len() as i64 * 10;

        let mut matches: Vec<(usize, i64)> = resources
            .iter()
            .enumerate()
            .filter_map(|(idx, resource)| {
                let text = resource.search_text();
                self.matcher
                    .fuzzy_match(&text, sanitized_query)
                    .filter(|&score| score >= min_score)
                    .map(|score| (idx, score))
            })
            .collect();

        // Sort by score descending (highest score first)
        matches.sort_by(|a, b| b.1.cmp(&a.1));

        matches
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
}
