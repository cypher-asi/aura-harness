use super::*;

#[test]
fn test_truncate_below_threshold() {
    let content = "short";
    assert_eq!(truncate_content(content, 100, None, None), "short");
}

#[test]
fn test_truncate_preserves_head_and_tail() {
    let content = "a".repeat(300);
    let result = truncate_content(&content, 200, None, None);
    assert!(result.contains("content truncated"));
    assert!(result.len() < 300);
}

#[test]
fn test_compact_older_preserves_recent() {
    let mut messages = vec![
        Message::user("first"),
        Message::user("second"),
        Message::user("third"),
        Message::user("fourth"),
    ];
    let config = CompactionConfig {
        tool_result_max_chars: 10,
        text_max_chars: 10,
        preserve_recent: 2,
    };
    compact_older_messages(&mut messages, &config);
    assert_eq!(messages.len(), 4);
}

#[test]
fn test_select_tier_85pct() {
    let tier = select_tier(0.85);
    assert!(tier.is_some());
    let config = tier.unwrap();
    assert_eq!(
        config.preserve_recent,
        CompactionConfig::micro().preserve_recent
    );
    assert_eq!(
        config.tool_result_max_chars,
        CompactionConfig::micro().tool_result_max_chars
    );
}

#[test]
fn test_select_tier_below_threshold() {
    let tier = select_tier(0.10);
    assert!(tier.is_none());
}

// ---- New tests ----

#[test]
fn test_signature_extract_rust() {
    let rust_code = r#"
use std::io;

pub fn compute_sum(a: i32, b: i32) -> i32 {
let result = a + b;
println!("sum = {}", result);
if result > 100 {
    panic!("too big");
}
result
}

pub struct Config {
pub name: String,
pub value: u64,
}

impl Config {
pub fn new(name: &str) -> Self {
    Self {
        name: name.to_string(),
        value: 0,
    }
}

pub fn set_value(&mut self, v: u64) {
    self.value = v;
    println!("value set to {}", v);
    if v > 1000 {
        panic!("value too large");
    }
}
}

fn helper_internal() {
let x = 42;
let y = x * 2;
println!("{}", y);
for i in 0..10 {
    println!("{}", i);
}
}
"#;
    let result = try_signature_compact(rust_code);
    assert!(result.is_some(), "should extract Rust signatures");
    let extracted = result.unwrap();
    assert!(
        extracted.contains("pub fn compute_sum"),
        "should preserve fn signature"
    );
    assert!(
        extracted.contains("// ... body omitted ..."),
        "should replace body with placeholder"
    );
    assert!(
        extracted.contains("pub fn new"),
        "should preserve impl method signature"
    );
    assert!(
        extracted.len() < rust_code.len(),
        "should be shorter than original"
    );
}

#[test]
fn test_signature_extract_non_rust() {
    let json = r#"{"key": "value", "nested": {"a": 1, "b": 2}}"#;
    assert!(
        try_signature_compact(json).is_none(),
        "JSON should not be treated as Rust"
    );

    let plain = "This is just some plain text with no code at all.\nIt has multiple lines.\nBut nothing resembling Rust.";
    assert!(
        try_signature_compact(plain).is_none(),
        "Plain text should return None"
    );
}

#[test]
fn test_5_tier_selection() {
    // ≥85% → micro (most aggressive)
    let t = select_tier(0.90).unwrap();
    assert_eq!(t.preserve_recent, 2);
    assert_eq!(t.tool_result_max_chars, 200);

    let t = select_tier(0.85).unwrap();
    assert_eq!(t.preserve_recent, 2);

    // ≥70% → aggressive
    let t = select_tier(0.75).unwrap();
    assert_eq!(t.preserve_recent, 4);
    assert_eq!(t.tool_result_max_chars, 500);

    let t = select_tier(0.70).unwrap();
    assert_eq!(t.preserve_recent, 4);

    // ≥60% → moderate
    let t = select_tier(0.65).unwrap();
    assert_eq!(t.preserve_recent, 6);
    assert_eq!(t.tool_result_max_chars, 1000);
    assert_eq!(t.text_max_chars, 1500);

    let t = select_tier(0.60).unwrap();
    assert_eq!(t.preserve_recent, 6);

    // ≥30% → light
    let t = select_tier(0.45).unwrap();
    assert_eq!(t.preserve_recent, 8);
    assert_eq!(t.tool_result_max_chars, 3000);
    assert_eq!(t.text_max_chars, 4000);

    let t = select_tier(0.30).unwrap();
    assert_eq!(t.preserve_recent, 8);

    // ≥15% → history (least aggressive)
    let t = select_tier(0.20).unwrap();
    assert_eq!(t.preserve_recent, 6);
    assert_eq!(t.tool_result_max_chars, 1500);
    assert_eq!(t.text_max_chars, 2000);

    let t = select_tier(0.15).unwrap();
    assert_eq!(t.preserve_recent, 6);

    // Below 15% → no compaction
    assert!(select_tier(0.10).is_none());
    assert!(select_tier(0.0).is_none());
}

#[test]
fn test_configurable_head_tail() {
    let content = "a".repeat(10_000);

    // Default 1/3 each
    let result_default = truncate_content(&content, 3000, None, None);
    assert!(result_default.starts_with(&"a".repeat(1000)));
    assert!(result_default.ends_with(&"a".repeat(1000)));
    assert!(result_default.contains("content truncated"));

    // Custom head=2000, tail=500
    let result_custom = truncate_content(&content, 3000, Some(2000), Some(500));
    let head_part: String = result_custom.chars().take(2000).collect();
    assert_eq!(head_part, "a".repeat(2000));
    assert!(result_custom.ends_with(&"a".repeat(500)));

    // Micro-style head=6000, tail=3000 on larger content
    let big_content = "b".repeat(20_000);
    let result_micro = truncate_content(&big_content, 10_000, Some(6000), Some(3000));
    assert!(result_micro.starts_with(&"b".repeat(6000)));
    assert!(result_micro.ends_with(&"b".repeat(3000)));
    assert!(result_micro.contains("content truncated"));
}
