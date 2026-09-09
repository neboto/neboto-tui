//! Shared helper for hand-rolled pagination loops (`next_token` / `next_marker`
//! / `position` / `last_evaluated_table_name`) — ops that have no fluent SDK
//! paginator (e.g. WAFv2, API Gateway v2, EventBridge) or loops that predate one.

/// Advance a manual pagination token, returning `None` (i.e. "stop paginating")
/// when the next token is **absent**, an **empty string**, or **identical to the
/// token we just used** (a non-advancing token).
///
/// These are exactly the three stop conditions the AWS SDK's own paginators
/// apply (`token.is_empty()` plus `stop_on_duplicate_token`). A plain
/// `if token.is_none() { break }` loop misses the latter two: an API that
/// returns `Some("")` or a stuck token on the last page sends the loop spinning
/// forever, re-fetching the same page and burning API calls. Pass the token
/// you're about to overwrite as `current`:
///
/// ```ignore
/// next = next_page_token(resp.next_token(), &next);
/// if next.is_none() { break; }
/// ```
pub fn next_page_token(new: Option<&str>, current: &Option<String>) -> Option<String> {
    let new = new.filter(|s| !s.is_empty())?;
    if Some(new) == current.as_deref() {
        return None; // token didn't advance → we'd re-fetch the same page
    }
    Some(new.to_string())
}

#[cfg(test)]
mod tests {
    use super::next_page_token;

    #[test]
    fn stops_on_none() {
        assert_eq!(next_page_token(None, &Some("abc".into())), None);
    }

    #[test]
    fn stops_on_empty_string() {
        // The bug that hung GuardDuty: an empty token must end pagination, not
        // restart it.
        assert_eq!(next_page_token(Some(""), &None), None);
    }

    #[test]
    fn stops_on_non_advancing_token() {
        assert_eq!(next_page_token(Some("tok"), &Some("tok".into())), None);
    }

    #[test]
    fn advances_on_new_token() {
        assert_eq!(
            next_page_token(Some("p2"), &Some("p1".into())),
            Some("p2".to_string())
        );
        assert_eq!(next_page_token(Some("p1"), &None), Some("p1".to_string()));
    }
}
