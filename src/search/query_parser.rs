use crate::aws::service::ServiceType;

/// Represents a parsed search query with optional service prefix
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedQuery {
    /// The service to switch to, if detected in the query
    pub service: Option<ServiceType>,
    /// The search text after removing the service prefix
    pub search_text: String,
    /// Whether this query represents a service switch
    pub is_service_switch: bool,
}

/// Parse a query string for service prefix patterns like @ec2, @vpc, etc.
///
/// Supported patterns:
/// - `@ec2` - Switch to EC2, empty search
/// - `@ec2 web` - Switch to EC2, search "web"
/// - `@vpc subnet` - Switch to VPC, search "subnet"
/// - `web` - No switch, search "web" in current service
///
/// # Examples
///
/// ```
/// let parsed = parse_query("@ec2 web-server");
/// assert_eq!(parsed.service, Some(ServiceType::EC2));
/// assert_eq!(parsed.search_text, "web-server");
/// assert!(parsed.is_service_switch);
/// ```
/// `@all <text>` → the cross-service search text: fuzzy-match every service
/// with a warm cache entry instead of switching to one. Checked before
/// `parse_query` (which would reject `all` as an unknown service prefix).
/// `@allx` is NOT an @all query — the prefix must end the token.
pub fn parse_all_query(query: &str) -> Option<&str> {
    let rest = query.trim_start().strip_prefix("@all")?;
    if rest.is_empty() {
        Some("")
    } else if rest.starts_with(char::is_whitespace) {
        Some(rest.trim())
    } else {
        None
    }
}

pub fn parse_query(query: &str) -> ParsedQuery {
    let trimmed = query.trim();

    // Empty query - no service switch, empty search
    if trimmed.is_empty() {
        return ParsedQuery {
            service: None,
            search_text: String::new(),
            is_service_switch: false,
        };
    }

    // Check for @ prefix pattern
    if trimmed.starts_with('@') {
        return parse_at_prefix(trimmed);
    }

    // Check for colon pattern (ec2:, vpc:, etc.) — but ONLY when the text before
    // the first colon is a *known* service prefix. Otherwise the colon is just
    // part of the search text (e.g. a pasted ARN `arn:aws:...`, or an id/value
    // dropped in by a jump), which should fuzzy-search literally rather than be
    // mistaken for an "invalid service prefix".
    if let Some(colon_pos) = trimmed.find(':') {
        if ServiceType::from_prefix(&trimmed[..colon_pos]).is_some() {
            return parse_colon_prefix(trimmed, colon_pos);
        }
    }

    // No prefix detected - regular search in current service
    ParsedQuery {
        service: None,
        search_text: trimmed.to_string(),
        is_service_switch: false,
    }
}

/// Parse @ prefix pattern like @ec2, @vpc
fn parse_at_prefix(query: &str) -> ParsedQuery {
    // Remove the @ symbol
    let without_at = &query[1..];

    // Find the end of the service name (space or end of string)
    let split_pos = without_at
        .find(char::is_whitespace)
        .unwrap_or(without_at.len());

    let service_str = &without_at[..split_pos];
    let remaining = without_at[split_pos..].trim();

    // Try to map to ServiceType
    let service = ServiceType::from_prefix(service_str);

    ParsedQuery {
        service,
        search_text: remaining.to_string(),
        is_service_switch: true,
    }
}

/// An exact tag filter extracted from a search query: `tag:key` (key present,
/// any value) or `tag:key=value` (exact value). Comparisons are
/// case-insensitive — friendlier at a terminal than AWS's case-sensitive tags.
#[derive(Debug, Clone, PartialEq)]
pub struct TagFilter {
    pub key: String,
    pub value: Option<String>,
}

impl TagFilter {
    /// Whether a resource's tag map satisfies this filter.
    pub fn matches(&self, tags: &std::collections::HashMap<String, String>) -> bool {
        tags.iter().any(|(k, v)| {
            k.eq_ignore_ascii_case(&self.key)
                && self
                    .value
                    .as_ref()
                    .is_none_or(|want| v.eq_ignore_ascii_case(want))
        })
    }
}

/// Split `tag:key[=value]` terms out of a search string, returning the exact
/// filters and the remaining text (re-joined) for fuzzy matching. A bare
/// `tag:` with no key stays literal search text.
pub fn split_tag_filters(text: &str) -> (Vec<TagFilter>, String) {
    let mut filters = Vec::new();
    let mut rest: Vec<&str> = Vec::new();
    for term in text.split_whitespace() {
        match term.strip_prefix("tag:") {
            Some(spec) if !spec.is_empty() => {
                let (key, value) = match spec.split_once('=') {
                    Some((k, v)) => (k.to_string(), Some(v.to_string())),
                    None => (spec.to_string(), None),
                };
                filters.push(TagFilter { key, value });
            }
            _ => rest.push(term),
        }
    }
    (filters, rest.join(" "))
}

/// Parse colon prefix pattern like ec2:, vpc:
fn parse_colon_prefix(query: &str, colon_pos: usize) -> ParsedQuery {
    let service_str = &query[..colon_pos];
    let remaining = query[colon_pos + 1..].trim();

    // Try to map to ServiceType
    let service = ServiceType::from_prefix(service_str);

    ParsedQuery {
        service,
        search_text: remaining.to_string(),
        is_service_switch: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_all_query_requires_the_exact_prefix() {
        assert_eq!(parse_all_query("@all"), Some(""));
        assert_eq!(parse_all_query("@all "), Some(""));
        assert_eq!(parse_all_query("@all api prod"), Some("api prod"));
        assert_eq!(parse_all_query("  @all x"), Some("x"));
        assert_eq!(parse_all_query("@allx"), None); // prefix must end the token
        assert_eq!(parse_all_query("@ec2 all"), None);
        assert_eq!(parse_all_query("all"), None);
    }

    #[test]
    fn test_empty_query() {
        let parsed = parse_query("");
        assert_eq!(parsed.service, None);
        assert_eq!(parsed.search_text, "");
        assert!(!parsed.is_service_switch);
    }

    #[test]
    fn test_regular_search() {
        let parsed = parse_query("web-server");
        assert_eq!(parsed.service, None);
        assert_eq!(parsed.search_text, "web-server");
        assert!(!parsed.is_service_switch);
    }

    #[test]
    fn test_at_prefix_ec2_only() {
        let parsed = parse_query("@ec2");
        assert_eq!(parsed.service, Some(ServiceType::EC2));
        assert_eq!(parsed.search_text, "");
        assert!(parsed.is_service_switch);
    }

    #[test]
    fn test_at_prefix_ec2_with_search() {
        let parsed = parse_query("@ec2 web-server");
        assert_eq!(parsed.service, Some(ServiceType::EC2));
        assert_eq!(parsed.search_text, "web-server");
        assert!(parsed.is_service_switch);
    }

    #[test]
    fn test_at_prefix_vpc_with_search() {
        let parsed = parse_query("@vpc 10.0");
        assert_eq!(parsed.service, Some(ServiceType::VPC));
        assert_eq!(parsed.search_text, "10.0");
        assert!(parsed.is_service_switch);
    }

    #[test]
    fn test_colon_prefix() {
        let parsed = parse_query("ec2:web");
        assert_eq!(parsed.service, Some(ServiceType::EC2));
        assert_eq!(parsed.search_text, "web");
        assert!(parsed.is_service_switch);
    }

    #[test]
    fn test_arn_searches_literally() {
        // A pasted ARN (or an ARN id dropped in by a jump) must NOT be parsed as
        // a `service:` prefix — `arn` is not a service, so it's a normal search.
        let arn = "arn:aws:cloudformation:eu-west-1:123:stack/my-stack/abc";
        let parsed = parse_query(arn);
        assert_eq!(parsed.service, None);
        assert_eq!(parsed.search_text, arn);
        assert!(!parsed.is_service_switch);
    }

    #[test]
    fn test_unknown_colon_prefix_is_literal() {
        // Unknown word before a colon → literal search, not an invalid-prefix.
        let parsed = parse_query("foo:bar");
        assert_eq!(parsed.service, None);
        assert_eq!(parsed.search_text, "foo:bar");
        assert!(!parsed.is_service_switch);
    }

    #[test]
    fn test_invalid_service() {
        let parsed = parse_query("@unknown service");
        assert_eq!(parsed.service, None);
        assert_eq!(parsed.search_text, "service");
        assert!(parsed.is_service_switch);
    }

    #[test]
    fn test_case_insensitive() {
        let parsed = parse_query("@EC2 test");
        assert_eq!(parsed.service, Some(ServiceType::EC2));
        assert_eq!(parsed.search_text, "test");
    }

    #[test]
    fn test_whitespace_handling() {
        let parsed = parse_query("  @vpc   subnet  ");
        assert_eq!(parsed.service, Some(ServiceType::VPC));
        assert_eq!(parsed.search_text, "subnet");
        assert!(parsed.is_service_switch);
    }

    #[test]
    fn test_at_prefix_iam_with_search() {
        let parsed = parse_query("@iam admin");
        assert_eq!(parsed.service, Some(ServiceType::IAM));
        assert_eq!(parsed.search_text, "admin");
        assert!(parsed.is_service_switch);
    }

    #[test]
    fn test_at_prefix_elb_with_search() {
        let parsed = parse_query("@elb web");
        assert_eq!(parsed.service, Some(ServiceType::Elb));
        assert_eq!(parsed.search_text, "web");
        assert!(parsed.is_service_switch);
    }

    #[test]
    fn test_elb_aliases() {
        for alias in ["@lb", "@alb", "@nlb", "@tg", "@targetgroups", "@loadbalancers"] {
            let parsed = parse_query(alias);
            assert_eq!(parsed.service, Some(ServiceType::Elb), "alias {alias}");
        }
    }

    #[test]
    fn test_at_prefix_asg() {
        let parsed = parse_query("@asg web");
        assert_eq!(parsed.service, Some(ServiceType::Asg));
        assert_eq!(parsed.search_text, "web");
        assert!(parsed.is_service_switch);
        for alias in ["@autoscaling", "@scaling", "@scalinggroups"] {
            assert_eq!(parse_query(alias).service, Some(ServiceType::Asg), "alias {alias}");
        }
    }

    #[test]
    fn test_at_prefix_messaging() {
        let parsed = parse_query("@sqs orders");
        assert_eq!(parsed.service, Some(ServiceType::Messaging));
        assert_eq!(parsed.search_text, "orders");
        assert!(parsed.is_service_switch);
        for alias in ["@sns", "@msg", "@messaging", "@queues", "@topics"] {
            assert_eq!(
                parse_query(alias).service,
                Some(ServiceType::Messaging),
                "alias {alias}"
            );
        }
    }

    #[test]
    fn test_at_prefix_config() {
        let parsed = parse_query("@secrets db");
        assert_eq!(parsed.service, Some(ServiceType::Secrets));
        assert_eq!(parsed.search_text, "db");
        assert!(parsed.is_service_switch);
        for alias in ["@secret", "@sm", "@secretsmanager"] {
            assert_eq!(
                parse_query(alias).service,
                Some(ServiceType::Secrets),
                "alias {alias}"
            );
        }
        // SSM (Parameter Store, Documents, Fleet, Patch) is now its own service.
        for alias in ["@ssm", "@params", "@parameters", "@fleet", "@patch"] {
            assert_eq!(
                parse_query(alias).service,
                Some(ServiceType::Ssm),
                "alias {alias}"
            );
        }
    }

    #[test]
    fn test_multi_word_search() {
        let parsed = parse_query("@ec2 web server production");
        assert_eq!(parsed.service, Some(ServiceType::EC2));
        assert_eq!(parsed.search_text, "web server production");
    }

    fn tags(pairs: &[(&str, &str)]) -> std::collections::HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn test_split_tag_filters_key_value() {
        let (filters, rest) = split_tag_filters("tag:env=prod web");
        assert_eq!(filters.len(), 1);
        assert_eq!(filters[0].key, "env");
        assert_eq!(filters[0].value.as_deref(), Some("prod"));
        assert_eq!(rest, "web");
    }

    #[test]
    fn test_split_tag_filters_key_only_and_multiple() {
        let (filters, rest) = split_tag_filters("tag:Team api tag:env=prod");
        assert_eq!(filters.len(), 2);
        assert_eq!(filters[0].key, "Team");
        assert_eq!(filters[0].value, None);
        assert_eq!(filters[1].key, "env");
        assert_eq!(rest, "api");
    }

    #[test]
    fn test_split_tag_filters_bare_prefix_stays_literal() {
        let (filters, rest) = split_tag_filters("tag: something");
        assert!(filters.is_empty());
        assert_eq!(rest, "tag: something");
    }

    #[test]
    fn test_split_tag_filters_none() {
        let (filters, rest) = split_tag_filters("plain query text");
        assert!(filters.is_empty());
        assert_eq!(rest, "plain query text");
    }

    #[test]
    fn test_tag_filter_matches_case_insensitive() {
        let f = TagFilter {
            key: "env".to_string(),
            value: Some("prod".to_string()),
        };
        assert!(f.matches(&tags(&[("Env", "Prod")])));
        assert!(!f.matches(&tags(&[("Env", "staging")])));
        assert!(!f.matches(&tags(&[("team", "prod")])));

        let key_only = TagFilter {
            key: "team".to_string(),
            value: None,
        };
        assert!(key_only.matches(&tags(&[("Team", "anything")])));
        assert!(!key_only.matches(&tags(&[("env", "prod")])));
    }
}
