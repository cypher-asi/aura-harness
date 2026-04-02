//! Stage 2: LLM refinement of pre-filtered candidates.

use crate::error::MemoryError;
use crate::types::{CandidateType, MemoryCandidate, RefinedCandidate};
use aura_reasoner::{Message, ModelProvider, ModelRequest};
use std::fmt::Write;
use std::sync::Arc;

pub struct LlmRefiner {
    provider: Arc<dyn ModelProvider>,
    config: RefinerConfig,
}

pub struct RefinerConfig {
    pub model: String,
    pub auth_token: Option<String>,
}

impl LlmRefiner {
    pub fn new(provider: Arc<dyn ModelProvider>, config: RefinerConfig) -> Self {
        Self { provider, config }
    }

    /// Refine a batch of memory candidates via the LLM.
    ///
    /// # Errors
    /// Returns error on provider failure or unparseable response.
    pub async fn refine(
        &self,
        candidates: Vec<MemoryCandidate>,
    ) -> Result<Vec<RefinedCandidate>, MemoryError> {
        if candidates.is_empty() {
            return Ok(Vec::new());
        }

        let prompt = Self::build_prompt(&candidates);

        let request = ModelRequest::builder(&self.config.model, REFINER_SYSTEM_PROMPT)
            .messages(vec![Message::user(prompt)])
            .max_tokens(1024)
            .auth_token(self.config.auth_token.clone())
            .build();

        let response = self
            .provider
            .complete(request)
            .await
            .map_err(|e| MemoryError::Provider(e.to_string()))?;

        let response_text = response.message.text_content();
        Ok(Self::parse_response(&response_text, &candidates))
    }

    fn build_prompt(candidates: &[MemoryCandidate]) -> String {
        let mut prompt = String::from(
            "Given these candidate memories extracted from a work session, decide which to keep.\n\nCandidates:\n",
        );

        for (i, c) in candidates.iter().enumerate() {
            let type_str = match c.candidate_type {
                CandidateType::Fact => "fact",
                CandidateType::Event => "event",
                CandidateType::Procedure => "procedure",
            };
            let key_str = c.key.as_deref().unwrap_or("(none)");
            let summary_str = c.summary.as_deref().unwrap_or("");
            let _ = writeln!(
                prompt,
                "{}. {}: key={}, value={}, source={} {}",
                i + 1,
                type_str,
                key_str,
                c.value,
                c.source_hint,
                summary_str
            );
        }

        prompt.push_str(
            "\nFor each candidate, respond with one line in this format:\n\
             N. KEEP|DROP key=\"refined_key\" confidence=0.X importance=0.X reason=\"...\"\n\
             Where N is the candidate number.",
        );

        prompt
    }

    fn parse_response(response: &str, candidates: &[MemoryCandidate]) -> Vec<RefinedCandidate> {
        let mut refined = Vec::new();
        let mut seen_indices = Vec::new();

        for line in response.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            let parts: Vec<&str> = line.splitn(2, ". ").collect();
            if parts.len() != 2 {
                continue;
            }
            let idx: usize = match parts[0].trim().parse::<usize>() {
                Ok(n) if n >= 1 && n <= candidates.len() => n - 1,
                _ => continue,
            };

            let rest = parts[1];
            let keep = rest.starts_with("KEEP");

            let confidence = extract_float(rest, "confidence=")
                .unwrap_or(candidates[idx].preliminary_confidence);
            let importance = extract_float(rest, "importance=")
                .unwrap_or(candidates[idx].preliminary_importance);
            let key = extract_quoted(rest, "key=")
                .unwrap_or_else(|| candidates[idx].key.clone().unwrap_or_default());

            seen_indices.push(idx);
            refined.push(RefinedCandidate {
                candidate_type: candidates[idx].candidate_type.clone(),
                key,
                value: candidates[idx].value.clone(),
                summary: candidates[idx].summary.clone(),
                confidence,
                importance,
                keep,
            });
        }

        for (i, c) in candidates.iter().enumerate() {
            if !seen_indices.contains(&i) {
                refined.push(RefinedCandidate {
                    candidate_type: c.candidate_type.clone(),
                    key: c.key.clone().unwrap_or_default(),
                    value: c.value.clone(),
                    summary: c.summary.clone(),
                    confidence: c.preliminary_confidence,
                    importance: c.preliminary_importance,
                    keep: true,
                });
            }
        }

        refined
    }
}

fn extract_float(text: &str, prefix: &str) -> Option<f32> {
    let start = text.find(prefix)? + prefix.len();
    let end = text[start..]
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .map_or(text.len(), |e| start + e);
    text[start..end].parse().ok()
}

fn extract_quoted(text: &str, prefix: &str) -> Option<String> {
    let start = text.find(prefix)? + prefix.len();
    let rest = &text[start..];
    if let Some(stripped) = rest.strip_prefix('"') {
        let end = stripped.find('"')?;
        Some(stripped[..end].to_string())
    } else {
        let end = rest
            .find(|c: char| c.is_whitespace())
            .unwrap_or(rest.len());
        Some(rest[..end].to_string())
    }
}

const REFINER_SYSTEM_PROMPT: &str = "You are a memory curator for an AI agent. Given candidate memories \
extracted from a work session, decide which to KEEP or DROP. For kept items, refine their key names \
for consistency and assign confidence (0.0-1.0) and importance (0.0-1.0) scores. Be selective: \
only keep genuinely useful long-term knowledge. Drop transient observations.";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{CandidateType, MemoryCandidate};

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
    fn build_prompt_smoke_test() {
        let candidates = vec![make_candidate("test_key")];
        let prompt = LlmRefiner::build_prompt(&candidates);
        assert!(prompt.contains("test_key"));
        assert!(prompt.contains("KEEP|DROP"));
    }
}
