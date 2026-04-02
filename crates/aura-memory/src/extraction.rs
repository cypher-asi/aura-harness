//! Stage 1: Heuristic candidate extraction from `AgentLoopResult`.

use crate::types::{CandidateType, MemoryCandidate};
use aura_agent::AgentLoopResult;

pub struct HeuristicExtractor;

#[allow(clippy::unused_self)]
impl HeuristicExtractor {
    pub fn extract(&self, result: &AgentLoopResult) -> Vec<MemoryCandidate> {
        let mut candidates = Vec::new();

        self.extract_from_text(result, &mut candidates);
        self.extract_task_outcome(result, &mut candidates);

        candidates.truncate(15);
        candidates
    }

    fn extract_from_text(
        &self,
        result: &AgentLoopResult,
        candidates: &mut Vec<MemoryCandidate>,
    ) {
        let text = &result.total_text;
        if text.is_empty() {
            return;
        }

        let patterns: &[(&str, &str)] = &[
            ("the project uses ", "project_technology"),
            ("i'll use ", "tool_preference"),
            ("the test command is ", "test_command"),
            ("the build command is ", "build_command"),
            ("preferred language", "preferred_language"),
            ("using framework", "framework"),
            ("deploy strategy", "deploy_strategy"),
        ];

        for (pattern, key) in patterns {
            if let Some(idx) = text.to_lowercase().find(pattern) {
                let start = idx + pattern.len();
                let value_text: String = text[start..]
                    .chars()
                    .take_while(|c| *c != '.' && *c != '\n' && *c != ',')
                    .collect();
                let value_text = value_text.trim().to_string();
                if !value_text.is_empty() && value_text.len() < 200 {
                    candidates.push(MemoryCandidate {
                        candidate_type: CandidateType::Fact,
                        key: Some((*key).to_string()),
                        value: serde_json::Value::String(value_text),
                        summary: None,
                        source_hint: "agent_statement".to_string(),
                        preliminary_confidence: 0.7,
                        preliminary_importance: 0.5,
                    });
                }
            }
        }
    }

    fn extract_task_outcome(
        &self,
        result: &AgentLoopResult,
        candidates: &mut Vec<MemoryCandidate>,
    ) {
        if result.iterations == 0 {
            return;
        }

        let outcome = if result.timed_out {
            "timed_out"
        } else if result.stalled {
            "stalled"
        } else if result.llm_error.is_some() {
            "llm_error"
        } else {
            "completed"
        };

        let summary = format!(
            "Task {} after {} iterations ({} input tokens, {} output tokens)",
            outcome, result.iterations, result.total_input_tokens, result.total_output_tokens
        );

        candidates.push(MemoryCandidate {
            candidate_type: CandidateType::Event,
            key: None,
            value: serde_json::json!({
                "outcome": outcome,
                "iterations": result.iterations,
                "input_tokens": result.total_input_tokens,
                "output_tokens": result.total_output_tokens,
            }),
            summary: Some(summary),
            source_hint: "task_outcome".to_string(),
            preliminary_confidence: 0.9,
            preliminary_importance: 0.6,
        });
    }
}
