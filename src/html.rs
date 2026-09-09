//! Minimal HTML → plain-text converter for AWS-supplied description blobs
//! (Trusted Advisor check descriptions, and any other field that arrives as a
//! small HTML fragment). The terminal can't render markup, so we flatten to
//! readable text: line/paragraph breaks become newlines, list items become
//! bullets, and `<a href="URL">text</a>` becomes `text (URL)` so links survive.
//!
//! This is intentionally small and dependency-free — it targets the constrained
//! HTML AWS emits (links, breaks, paragraphs, lists, bold/italic), not arbitrary
//! web pages.

/// Convert a small HTML fragment to clean plain text with preserved links.
pub fn html_to_text(html: &str) -> String {
    let chars: Vec<char> = html.chars().collect();
    let mut out = String::new();
    let mut href_stack: Vec<String> = Vec::new();
    let mut i = 0;

    while i < chars.len() {
        let c = chars[i];
        if c == '<' {
            // Read the tag up to '>'.
            let start = i + 1;
            let mut j = start;
            while j < chars.len() && chars[j] != '>' {
                j += 1;
            }
            let tag: String = chars[start..j].iter().collect();
            i = j + 1; // skip past '>'

            let tl = tag.trim().to_lowercase();
            if tl.starts_with("br") {
                out.push('\n');
            } else if tl == "/p" || tl == "/div" {
                out.push('\n');
            } else if tl == "p" || tl.starts_with("p ") || tl == "div" || tl.starts_with("div ") {
                if !out.is_empty() && !out.ends_with('\n') {
                    out.push('\n');
                }
            } else if tl == "li" || tl.starts_with("li ") {
                out.push_str("\n• ");
            } else if tl == "/li"
                || tl == "ul"
                || tl == "/ul"
                || tl == "ol"
                || tl == "/ol"
                || tl == "/tr"
            {
                out.push('\n');
            } else if tl == "a" || tl.starts_with("a ") {
                href_stack.push(extract_attr(&tag, "href").unwrap_or_default());
            } else if tl == "/a" {
                if let Some(href) = href_stack.pop() {
                    let href = href.trim();
                    if !href.is_empty() {
                        out.push_str(&format!(" ({})", href));
                    }
                }
            }
            // All other tags (b, i, strong, em, span, table, td, …) are dropped,
            // leaving their text content.
        } else {
            out.push(c);
            i += 1;
        }
    }

    normalize_whitespace(&decode_entities(&out))
}

/// Pull a quoted attribute value out of a tag's inner text (`href="..."`).
fn extract_attr(tag: &str, attr: &str) -> Option<String> {
    let lower = tag.to_lowercase();
    let key = format!("{}=", attr);
    let pos = lower.find(&key)? + key.len();
    let rest = &tag[pos..];
    let mut chars = rest.chars();
    match chars.next()? {
        q @ ('"' | '\'') => Some(chars.take_while(|&c| c != q).collect()),
        // Unquoted attribute: read until whitespace or end.
        first => {
            let mut v = String::from(first);
            v.extend(chars.take_while(|c| !c.is_whitespace()));
            Some(v)
        }
    }
}

/// Decode the handful of HTML entities AWS uses, plus numeric (`&#39;` /
/// `&#x27;`) references.
fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == '&' {
            if let Some(semi) = bytes[i + 1..].iter().position(|&c| c == ';') {
                let entity: String = bytes[i + 1..i + 1 + semi].iter().collect();
                let replacement = match entity.as_str() {
                    "amp" => Some('&'),
                    "lt" => Some('<'),
                    "gt" => Some('>'),
                    "quot" => Some('"'),
                    "apos" => Some('\''),
                    "nbsp" => Some(' '),
                    e if e.starts_with("#x") || e.starts_with("#X") => {
                        u32::from_str_radix(&e[2..], 16).ok().and_then(char::from_u32)
                    }
                    e if e.starts_with('#') => {
                        e[1..].parse::<u32>().ok().and_then(char::from_u32)
                    }
                    _ => None,
                };
                if let Some(ch) = replacement {
                    out.push(ch);
                    i += semi + 2; // '&' + entity + ';'
                    continue;
                }
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

/// Collapse intra-line whitespace runs, trim each line, and squeeze 2+ blank
/// lines down to one — so removed tags don't leave ragged gaps.
fn normalize_whitespace(s: &str) -> String {
    let mut lines: Vec<String> = s
        .split('\n')
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect();
    // Drop leading/trailing blank lines.
    while lines.first().is_some_and(|l| l.is_empty()) {
        lines.remove(0);
    }
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    // Collapse consecutive blanks.
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut prev_blank = false;
    for line in lines.drain(..) {
        let blank = line.is_empty();
        if blank && prev_blank {
            continue;
        }
        prev_blank = blank;
        out.push(line);
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_become_text_with_url() {
        let html = r#"See <a href="https://aws.amazon.com/x">Best Practices</a> for details."#;
        assert_eq!(
            html_to_text(html),
            "See Best Practices (https://aws.amazon.com/x) for details."
        );
    }

    #[test]
    fn breaks_paragraphs_and_lists() {
        let html = "<p>Intro line.</p>First<br>Second<ul><li>One</li><li>Two</li></ul>";
        let out = html_to_text(html);
        assert!(out.contains("Intro line."));
        assert!(out.contains("First\nSecond"));
        assert!(out.contains("• One"));
        assert!(out.contains("• Two"));
        // No raw tags survive.
        assert!(!out.contains('<'));
    }

    #[test]
    fn entities_decoded() {
        assert_eq!(
            html_to_text("Rock &amp; roll &lt;tag&gt; &#39;quote&#39; &#x41;"),
            "Rock & roll <tag> 'quote' A"
        );
    }

    #[test]
    fn collapses_blank_runs_and_trims() {
        let html = "<p>One</p><p></p><p></p><p>Two</p>";
        let out = html_to_text(html);
        assert_eq!(out, "One\n\nTwo");
    }

    #[test]
    fn plain_text_passthrough() {
        assert_eq!(html_to_text("just plain text"), "just plain text");
    }
}
