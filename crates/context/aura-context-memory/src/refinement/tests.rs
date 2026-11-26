use super::*;
use crate::types::{CandidateType, MemoryCandidate};
use aura_model::MockProvider;

fn make_candidate(key: &str) -> MemoryCandidate {
    MemoryCandidate {
        candidate_type: CandidateType::Fact,
        key: Some(key.to_string()),
        value: serde_json::Value::String("val".to_string()),
        summary: None,
        source_hint: "test".to_string(),
        preliminary_confidence: 0.7,
        preliminary_importance: 0.5,
    }
}

#[test]
fn extract_float_present() {
    assert!((extract_float("confidence=0.85 rest", "confidence=").unwrap() - 0.85).abs() < 1e-3);
}

#[test]
fn extract_float_missing() {
    assert!(extract_float("no match", "confidence=").is_none());
}

#[test]
fn extract_float_malformed() {
    assert!(extract_float("confidence=abc", "confidence=").is_none());
}

#[test]
fn extract_quoted_double_quoted() {
    let result = extract_quoted("key=\"hello world\" rest", "key=");
    assert_eq!(result.unwrap(), "hello world");
}

#[test]
fn extract_quoted_unquoted() {
    let result = extract_quoted("key=bare_value rest", "key=");
    assert_eq!(result.unwrap(), "bare_value");
}

#[test]
fn extract_quoted_missing() {
    assert!(extract_quoted("no match", "key=").is_none());
}

#[test]
fn parse_response_valid_keep_drop() {
    let candidates = vec![make_candidate("a"), make_candidate("b")];
    let response = "1. KEEP key=\"alpha\" confidence=0.9 importance=0.8\n2. DROP key=\"beta\" confidence=0.3 importance=0.1";
    let refined = LlmRefiner::parse_response(response, &candidates);
    assert_eq!(refined.len(), 2);
    assert!(refined[0].keep);
    assert!(!refined[1].keep);
    assert_eq!(refined[0].key, "alpha");
}

#[test]
fn parse_response_out_of_range_index_ignored() {
    let candidates = vec![make_candidate("a")];
    let response = "1. KEEP key=\"a\" confidence=0.9 importance=0.8\n5. KEEP key=\"bad\" confidence=0.9 importance=0.8";
    let refined = LlmRefiner::parse_response(response, &candidates);
    assert_eq!(refined.len(), 1);
}

#[test]
fn parse_response_malformed_lines_skipped() {
    let candidates = vec![make_candidate("a")];
    let response = "garbage\n1. KEEP key=\"a\" confidence=0.9 importance=0.8\nmore garbage";
    let refined = LlmRefiner::parse_response(response, &candidates);
    assert_eq!(refined.len(), 1);
    assert!(refined[0].keep);
}

#[test]
fn parse_response_empty() {
    let candidates = vec![make_candidate("a"), make_candidate("b")];
    let response = "";
    let refined = LlmRefiner::parse_response(response, &candidates);
    assert_eq!(refined.len(), 2);
    assert!(refined.iter().all(|r| r.keep));
}

#[test]
fn parse_fact_line_valid() {
    let line =
        "FACT key=\"favorite_dog\" value=\"Belgian Malanois\" confidence=0.95 importance=0.6";
    let result = parse_fact_line(line).unwrap();
    assert_eq!(result.key, "favorite_dog");
    assert_eq!(
        result.value,
        serde_json::Value::String("Belgian Malanois".into())
    );
    assert!((result.confidence - 0.95).abs() < 1e-3);
    assert!(result.keep);
}

#[test]
fn parse_response_with_new_facts() {
    let candidates = vec![make_candidate("a")];
    let response = "1. KEEP key=\"a\" confidence=0.9 importance=0.8\nFACT key=\"pet\" value=\"dog\" confidence=0.85 importance=0.5";
    let refined = LlmRefiner::parse_response(response, &candidates);
    assert_eq!(refined.len(), 2);
    assert_eq!(refined[1].key, "pet");
}

#[test]
fn build_extraction_prompt_with_turn() {
    let turn = ConversationTurn {
        user_message: "My favorite dog is a Belgian Malanois".to_string(),
        assistant_text: "Great choice!".to_string(),
    };
    let prompt = LlmRefiner::build_extraction_prompt(&[], Some(&turn), &[]);
    assert!(prompt.contains("Belgian Malanois"));
    assert!(prompt.contains("Great choice!"));
    assert!(prompt.contains("FACT"));
}

#[test]
fn build_extraction_prompt_without_turn() {
    let candidates = vec![make_candidate("test_key")];
    let prompt = LlmRefiner::build_extraction_prompt(&candidates, None, &[]);
    assert!(prompt.contains("test_key"));
    assert!(prompt.contains("KEEP|DROP"));
}

#[test]
fn refinement_request_preserves_aura_routing_context() {
    let refiner = LlmRefiner::new(Arc::new(MockProvider::new()), RefinerConfig::default());
    let request = refiner
        .build_request(
            "extract memory".to_string(),
            RefinementRequestContext {
                auth_token: Some("jwt".to_string()),
                aura_project_id: Some("project-123".to_string()),
                aura_agent_id: Some("agent-123".to_string()),
                aura_session_id: Some("session-123".to_string()),
                aura_org_id: Some("org-123".to_string()),
                user_id: Some("user-123".to_string()),
            },
        )
        .expect("request should build");

    assert_eq!(request.auth_token.as_deref(), Some("jwt"));
    assert_eq!(request.aura_project_id.as_deref(), Some("project-123"));
    assert_eq!(request.aura_agent_id.as_deref(), Some("agent-123"));
    assert_eq!(request.aura_session_id.as_deref(), Some("session-123"));
    assert_eq!(request.aura_org_id.as_deref(), Some("org-123"));
    assert_eq!(request.model.as_str(), DEFAULT_MEMORY_MODEL);
}

#[test]
fn build_extraction_prompt_with_skills() {
    let turn = ConversationTurn {
        user_message: "Store notes in obsidian".to_string(),
        assistant_text: "Got it!".to_string(),
    };
    let skills = vec!["obsidian".to_string(), "git".to_string()];
    let prompt = LlmRefiner::build_extraction_prompt(&[], Some(&turn), &skills);
    assert!(prompt.contains("## Active skills"));
    assert!(prompt.contains("- obsidian"));
    assert!(prompt.contains("- git"));
}

#[test]
fn truncate_does_not_split_utf8() {
    let truncated = truncate("abc\u{1F980}def", 5);
    assert_eq!(truncated, "abc...");
}

#[test]
fn parse_procedure_line_valid() {
    let line = "PROCEDURE name=\"save_research\" trigger=\"when creating research reports\" steps=\"Store in Research/ folder;Use YAML frontmatter\" skill=\"obsidian\" confidence=0.9 importance=0.8";
    let result = parse_procedure_line(line).unwrap();
    assert_eq!(result.key, "save_research");
    assert!(matches!(result.candidate_type, CandidateType::Procedure));
    assert_eq!(
        result.trigger.as_deref(),
        Some("when creating research reports")
    );
    assert_eq!(result.steps.as_ref().unwrap().len(), 2);
    assert_eq!(result.skill_name.as_deref(), Some("obsidian"));
    assert!((result.confidence - 0.9).abs() < 1e-3);
    assert!(result.keep);
}

#[test]
fn parse_procedure_line_no_skill() {
    let line = "PROCEDURE name=\"format_code\" trigger=\"when writing code\" steps=\"Use 4-space indent\" skill=\"none\" confidence=0.85 importance=0.6";
    let result = parse_procedure_line(line).unwrap();
    assert_eq!(result.key, "format_code");
    assert!(result.skill_name.is_none());
}

#[test]
fn parse_procedure_line_missing_name() {
    let line = "PROCEDURE trigger=\"when\" steps=\"do stuff\" skill=\"none\" confidence=0.8 importance=0.5";
    assert!(parse_procedure_line(line).is_none());
}

#[test]
fn parse_response_with_procedure() {
    let candidates = vec![make_candidate("a")];
    let response = "1. KEEP key=\"a\" confidence=0.9 importance=0.8\nPROCEDURE name=\"save_notes\" trigger=\"when saving notes\" steps=\"Use obsidian\" skill=\"obsidian\" confidence=0.85 importance=0.7";
    let refined = LlmRefiner::parse_response(response, &candidates);
    assert_eq!(refined.len(), 2);
    assert_eq!(refined[0].key, "a");
    assert_eq!(refined[1].key, "save_notes");
    assert!(matches!(
        refined[1].candidate_type,
        CandidateType::Procedure
    ));
    assert_eq!(refined[1].skill_name.as_deref(), Some("obsidian"));
}

#[test]
fn parse_response_mixed_facts_and_procedures() {
    let response = "FACT key=\"dog_name\" value=\"Kaya\" confidence=0.95 importance=0.6\nPROCEDURE name=\"research_reports\" trigger=\"when creating research reports\" steps=\"Store in Research/ folder\" skill=\"obsidian\" confidence=0.9 importance=0.8";
    let refined = LlmRefiner::parse_response(response, &[]);
    assert_eq!(refined.len(), 2);
    assert!(matches!(refined[0].candidate_type, CandidateType::Fact));
    assert!(matches!(
        refined[1].candidate_type,
        CandidateType::Procedure
    ));
}
