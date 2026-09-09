//! `aws_smithy_types::Document` → JSON. The smithy crates ship no such
//! converter, and several services hand back untyped Documents (Control
//! Tower's enabled-control parameters and landing-zone manifest, AgentCore's
//! A2A agent card and memory event blobs).

/// Structure-preserving conversion. `Document::Number` splits three ways, so
/// this can't be a blanket `to_string`.
pub fn document_to_json(doc: &aws_smithy_types::Document) -> serde_json::Value {
    use aws_smithy_types::{Document, Number};
    match doc {
        Document::Object(map) => serde_json::Value::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), document_to_json(v)))
                .collect(),
        ),
        Document::Array(items) => {
            serde_json::Value::Array(items.iter().map(document_to_json).collect())
        }
        Document::Number(n) => match n {
            Number::PosInt(v) => serde_json::Value::from(*v),
            Number::NegInt(v) => serde_json::Value::from(*v),
            Number::Float(v) => serde_json::Value::from(*v),
        },
        Document::String(s) => serde_json::Value::String(s.clone()),
        Document::Bool(b) => serde_json::Value::Bool(*b),
        Document::Null => serde_json::Value::Null,
    }
}

/// Compact single-row rendering: bare strings print as-is (no JSON quotes),
/// everything else as one-line JSON.
pub fn document_display(doc: &aws_smithy_types::Document) -> String {
    match document_to_json(doc) {
        serde_json::Value::String(s) => s,
        other => other.to_string(),
    }
}

/// Pretty-printed JSON, for `$EDITOR` and detail bodies.
pub fn document_pretty(doc: &aws_smithy_types::Document) -> String {
    serde_json::to_string_pretty(&document_to_json(doc))
        .unwrap_or_else(|_| document_display(doc))
}
