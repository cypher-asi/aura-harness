use super::parse::{
    extract_float_value, extract_quoted_value, parse_merge_indices, parse_single_index,
};
use super::*;
use crate::types::MemoryScope;

#[test]
fn consolidation_uses_the_shared_low_cost_memory_model() {
    assert_eq!(ConsolidationConfig::default().model, DEFAULT_MEMORY_MODEL);
}

#[test]
fn parse_compress_response_with_summaries() {
    let response = "SUMMARY: First summary\nSUMMARY: Second summary\nNot a summary";
    let results = MemoryConsolidator::parse_compress_response(response);
    assert_eq!(results.len(), 2);
    assert_eq!(results[0], "First summary");
    assert_eq!(results[1], "Second summary");
}

#[test]
fn parse_compress_response_empty_lines_filtered() {
    let response = "SUMMARY: \nSUMMARY: Valid\n\n";
    let results = MemoryConsolidator::parse_compress_response(response);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], "Valid");
}

#[test]
fn parse_compress_response_no_prefix() {
    let response = "Just some text\nAnother line";
    let results = MemoryConsolidator::parse_compress_response(response);
    assert!(results.is_empty());
}

#[test]
fn parse_compress_response_colon_without_space() {
    let response = "SUMMARY:no space after colon";
    let results = MemoryConsolidator::parse_compress_response(response);
    assert_eq!(results.len(), 1);
}

#[test]
fn parse_merge_indices_valid() {
    assert_eq!(parse_merge_indices("1 3 key=\"merged\"", 5), Some((0, 2)));
}

#[test]
fn parse_merge_indices_out_of_bounds() {
    assert_eq!(parse_merge_indices("1 10 key=\"m\"", 5), None);
}

#[test]
fn parse_merge_indices_same() {
    assert_eq!(parse_merge_indices("2 2 key=\"m\"", 5), None);
}

#[test]
fn parse_single_index_valid() {
    assert_eq!(parse_single_index("3 value=\"x\"", 5), Some(2));
}

#[test]
fn parse_single_index_out_of_bounds() {
    assert_eq!(parse_single_index("6 value=\"x\"", 5), None);
}

#[test]
fn parse_single_index_zero() {
    assert_eq!(parse_single_index("0 value=\"x\"", 5), None);
}

#[test]
fn extract_quoted_value_double_quoted() {
    let result = extract_quoted_value("key=\"hello world\"", "key=");
    assert_eq!(result.unwrap(), "hello world");
}

#[test]
fn extract_quoted_value_bare() {
    let result = extract_quoted_value("value=bare rest", "value=");
    assert_eq!(result.unwrap(), "bare");
}

#[test]
fn extract_quoted_value_missing() {
    assert!(extract_quoted_value("no match here", "key=").is_none());
}

#[test]
fn extract_float_value_valid() {
    let result = extract_float_value("confidence=0.85 rest", "confidence=");
    assert!((result.unwrap() - 0.85).abs() < 1e-3);
}

#[test]
fn extract_float_value_missing() {
    assert!(extract_float_value("no match", "confidence=").is_none());
}

#[test]
fn consolidation_preserves_visibility_without_claiming_single_source() {
    let source = MemoryContinuity {
        scope: MemoryScope::Project,
        status: MemoryStatus::Active,
        sensitivity: MemorySensitivity::Normal,
        pinned: true,
        provenance: MemoryProvenance {
            session_id: Some("session-1".to_string()),
            excerpt: Some("source excerpt".to_string()),
            extractor_model: Some("model-1".to_string()),
            project_id: Some("project-1".to_string()),
            user_id: Some("user-1".to_string()),
            contributor_agent_id: Some("agent-1".to_string()),
        },
        superseded_by: Some("replacement".to_string()),
    };

    let result = consolidated_continuity(&source);

    assert_eq!(result.scope, MemoryScope::Project);
    assert_eq!(result.provenance.project_id.as_deref(), Some("project-1"));
    assert_eq!(result.provenance.user_id.as_deref(), Some("user-1"));
    assert!(result.provenance.session_id.is_none());
    assert!(result.provenance.contributor_agent_id.is_none());
    assert!(!result.pinned);
    assert!(result.superseded_by.is_none());
}

#[test]
fn consolidation_excludes_unapproved_or_sensitive_memory() {
    let mut continuity = MemoryContinuity::default();
    assert!(consolidatable(&continuity));

    continuity.status = MemoryStatus::Pending;
    assert!(!consolidatable(&continuity));

    continuity.status = MemoryStatus::Active;
    continuity.sensitivity = MemorySensitivity::Sensitive;
    assert!(!consolidatable(&continuity));
}
