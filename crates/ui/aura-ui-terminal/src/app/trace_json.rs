//! A run-graph node as pretty-printed JSON, for the full-panel step view.
//!
//! Fields keep their recorded order (serde_json's map sorts keys), and
//! metadata values that are themselves JSON (tool arguments, results,
//! id lists) are expanded instead of shown as escaped strings.

use crate::events::{TraceNode, TraceNodeStatus};
use serde_json::Value;

/// A link from the node: (edge kind, other node id, other node label).
pub(super) type NodeLink = (String, String, Option<String>);

/// `node` with its `links`, as indented JSON.
pub(super) fn node_json(node: &TraceNode, links: &[NodeLink]) -> String {
    let text = |s: &str| quoted(s);
    let mut fields = vec![
        ("id".to_string(), text(&node.id)),
        ("kind".to_string(), text(&node.kind)),
        ("label".to_string(), text(&node.label)),
        ("status".to_string(), text(status_name(node.status))),
    ];
    if let Some(seq) = node.seq {
        fields.push(("seq".to_string(), seq.to_string()));
    }
    if let Some(parent) = &node.parent {
        fields.push(("parent".to_string(), text(parent)));
    }
    if !node.detail.is_empty() {
        fields.push(("detail".to_string(), text(&node.detail)));
    }
    if !node.meta.is_empty() {
        let meta = node
            .meta
            .iter()
            .map(|(key, value)| (key.clone(), pretty(&meta_value(value))))
            .collect();
        fields.push(("meta".to_string(), object(meta)));
    }
    if !links.is_empty() {
        let items = links
            .iter()
            .map(|(kind, other, label)| {
                let mut link = vec![
                    ("kind".to_string(), text(kind)),
                    ("node".to_string(), text(other)),
                ];
                if let Some(label) = label {
                    link.push(("label".to_string(), text(label)));
                }
                object(link)
            })
            .collect();
        fields.push(("links".to_string(), array(items)));
    }
    let mut out = object(fields);
    out.push('\n');
    out
}

const fn status_name(status: TraceNodeStatus) -> &'static str {
    match status {
        TraceNodeStatus::Pending => "pending",
        TraceNodeStatus::Ok => "ok",
        TraceNodeStatus::Warn => "warn",
        TraceNodeStatus::Error => "error",
    }
}

/// A metadata value: parsed when it is JSON, otherwise kept as text.
fn meta_value(raw: &str) -> Value {
    serde_json::from_str(raw.trim()).unwrap_or_else(|_| Value::String(raw.to_string()))
}

fn quoted(s: &str) -> String {
    Value::String(s.to_string()).to_string()
}

fn pretty(value: &Value) -> String {
    // Serializing a `Value` cannot fail: every key is a string.
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

/// An object from already-rendered values, in the given order.
fn object(fields: Vec<(String, String)>) -> String {
    let entries = fields
        .into_iter()
        .map(|(key, value)| format!("{}: {value}", quoted(&key)))
        .collect();
    block('{', entries, '}')
}

/// An array from already-rendered items.
fn array(items: Vec<String>) -> String {
    block('[', items, ']')
}

fn block(open: char, entries: Vec<String>, close: char) -> String {
    if entries.is_empty() {
        return format!("{open}{close}");
    }
    let body: Vec<String> = entries
        .into_iter()
        .map(|entry| format!("  {}", entry.replace('\n', "\n  ")))
        .collect();
    format!("{open}\n{}\n{close}", body.join(",\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_keep_order_and_json_meta_is_expanded() {
        let node = TraceNode {
            id: "tc:1".into(),
            kind: "tool".into(),
            label: "run_command".into(),
            status: TraceNodeStatus::Warn,
            seq: Some(7),
            parent: Some("it:1".into()),
            meta: vec![
                ("tokens".into(), "1.2k in".into()),
                ("args".into(), r#"{"cmd":"cargo test","timeout":30}"#.into()),
                ("record_seqs".into(), "[7,8]".into()),
            ],
            ..TraceNode::default()
        };
        let links = vec![("←triggered".into(), "d:1".into(), Some("nudge".into()))];
        let json = node_json(&node, &links);
        let parsed: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["meta"]["args"]["timeout"], 30);
        assert_eq!(parsed["meta"]["tokens"], "1.2k in");
        assert_eq!(parsed["meta"]["record_seqs"][1], 8);
        assert_eq!(parsed["links"][0]["label"], "nudge");
        assert_eq!(parsed["status"], "warn");
        let id = json.find("\"id\"").unwrap();
        let meta = json.find("\"meta\"").unwrap();
        let tokens = json.find("\"tokens\"").unwrap();
        let args = json.find("\"args\"").unwrap();
        assert!(id < meta && tokens < args, "{json}");
        assert!(json.contains("\n    \"args\": {\n      \"cmd\""), "{json}");
    }

    #[test]
    fn empty_node_is_still_valid_json() {
        let json = node_json(&TraceNode::default(), &[]);
        assert!(serde_json::from_str::<Value>(&json).is_ok(), "{json}");
    }
}
