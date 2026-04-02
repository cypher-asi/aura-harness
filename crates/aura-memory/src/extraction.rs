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

#[cfg(test)]
mod tests {
    use super::*;
    use aura_agent::AgentLoopResult;

    #[test]
    fn empty_text_yields_no_candidates() {
        let extractor = HeuristicExtractor;
        let result = AgentLoopResult::default();
        let candidates = extractor.extract(&result);
        assert!(candidates.is_empty());
    }

    #[test]
    fn pattern_match_extracts_fact() {
        let extractor = HeuristicExtractor;
        let result = AgentLoopResult {
            total_text: "The project uses React for the frontend".to_string(),
            iterations: 1,
            ..Default::default()
        };
        let candidates = extractor.extract(&result);
        let fact_candidates: Vec<_> = candidates
            .iter()
            .filter(|c| c.key.as_deref() == Some("project_technology"))
            .collect();
        assert!(!fact_candidates.is_empty());
    }

    #[test]
    fn value_truncation_at_period() {
        let extractor = HeuristicExtractor;
        let result = AgentLoopResult {
            total_text: "the build command is cargo build. And more text".to_string(),
            iterations: 1,
            ..Default::default()
        };
        let candidates = extractor.extract(&result);
        let bc: Vec<_> = candidates
            .iter()
            .filter(|c| c.key.as_deref() == Some("build_command"))
            .collect();
        assert!(!bc.is_empty());
        if let serde_json::Value::String(s) = &bc[0].value {
            assert!(!s.contains('.'));
        }
    }

    #[test]
    fn output_capped_at_15() {
        let extractor = HeuristicExtractor;
        let mut text = String::new();
        for i in 0..20 {
            text.push_str(&format!("the project uses tech{i}. "));
        }
        let result = AgentLoopResult {
            total_text: text,
            iterations: 1,
            ..Default::default()
        };
        let candidates = extractor.extract(&result);
        assert!(candidates.len() <= 15);
    }

    #[test]
    fn iterations_zero_skips_task_outcome() {
        let extractor = HeuristicExtractor;
        let result = AgentLoopResult {
            total_text: "the project uses Go".to_string(),
            iterations: 0,
            ..Default::default()
        };
        let candidates = extractor.extract(&result);
        let events: Vec<_> = candidates
            .iter()
            .filter(|c| matches!(c.candidate_type, CandidateType::Event))
            .collect();
        assert!(events.is_empty());
    }

    #[test]
    fn timed_out_outcome() {
        let extractor = HeuristicExtractor;
        let result = AgentLoopResult {
            iterations: 5,
            timed_out: true,
            ..Default::default()
        };
        let candidates = extractor.extract(&result);
        let event = candidates
            .iter()
            .find(|c| matches!(c.candidate_type, CandidateType::Event))
            .unwrap();
        if let Some(ref summary) = event.summary {
            assert!(summary.contains("timed_out"));
        }
    }

    #[test]
    fn completed_outcome() {
        let extractor = HeuristicExtractor;
        let result = AgentLoopResult {
            iterations: 3,
            ..Default::default()
        };
        let candidates = extractor.extract(&result);
        let event = candidates
            .iter()
            .find(|c| matches!(c.candidate_type, CandidateType::Event))
            .unwrap();
        if let Some(ref summary) = event.summary {
            assert!(summary.contains("completed"));
        }
    }
}
