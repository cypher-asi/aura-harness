//! JSON extraction and execution-response parsing from model output.

use crate::agent_runner::FollowUpSuggestion;
use crate::file_ops::FileOp;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TaskExecution {
    pub notes: String,
    pub file_ops: Vec<FileOp>,
    pub follow_up_tasks: Vec<FollowUpSuggestion>,
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub parse_retries: u32,
    #[serde(default)]
    pub files_already_applied: bool,
}

#[derive(serde::Deserialize)]
struct RawExecutionResponse {
    notes: String,
    file_ops: Vec<FileOp>,
    #[serde(default)]
    follow_up_tasks: Vec<FollowUpSuggestion>,
}

pub fn parse_execution_response(response: &str) -> anyhow::Result<TaskExecution> {
    let trimmed = response.trim();

    if let Ok(parsed) = serde_json::from_str::<RawExecutionResponse>(trimmed) {
        return Ok(raw_to_execution(parsed));
    }

    if let Some(json_str) = extract_last_fenced_json(trimmed) {
        if let Ok(parsed) = serde_json::from_str::<RawExecutionResponse>(&json_str) {
            return Ok(raw_to_execution(parsed));
        }
    }

    if let Some(json_str) = extract_balanced_json(trimmed) {
        if let Ok(parsed) = serde_json::from_str::<RawExecutionResponse>(&json_str) {
            return Ok(raw_to_execution(parsed));
        }
    }

    anyhow::bail!(
        "failed to parse execution response: {}",
        &trimmed[..trimmed.len().min(500)]
    )
}

fn raw_to_execution(raw: RawExecutionResponse) -> TaskExecution {
    TaskExecution {
        notes: raw.notes,
        file_ops: raw.file_ops,
        follow_up_tasks: raw.follow_up_tasks,
        input_tokens: 0,
        output_tokens: 0,
        parse_retries: 0,
        files_already_applied: false,
    }
}

/// Extract JSON from the *last* fenced code block, which is more likely to
/// contain the final structured output when the model thinks out loud first.
fn extract_last_fenced_json(text: &str) -> Option<String> {
    let mut result = None;
    let start_markers = ["```json", "```"];

    for marker in &start_markers {
        let mut search_from = 0;
        while let Some(start) = text[search_from..].find(marker) {
            let abs_start = search_from + start;
            let after_marker = abs_start + marker.len();
            if let Some(end) = text[after_marker..].find("```") {
                result = Some(text[after_marker..after_marker + end].trim().to_string());
                search_from = after_marker + end + 3;
            } else {
                break;
            }
        }
        if result.is_some() {
            return result;
        }
    }
    None
}

/// Walk through the text looking for `{` and track brace depth to find a
/// complete top-level JSON object. Tries each `{` as a potential start so
/// it can skip over braces that appear inside prose.
pub fn extract_balanced_json(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        if bytes[i] == b'{' {
            let mut depth: i32 = 0;
            let mut in_string = false;
            let mut escape_next = false;
            let start = i;
            let mut j = i;

            while j < len {
                let ch = bytes[j];
                if escape_next {
                    escape_next = false;
                    j += 1;
                    continue;
                }
                if ch == b'\\' && in_string {
                    escape_next = true;
                    j += 1;
                    continue;
                }
                if ch == b'"' {
                    in_string = !in_string;
                } else if !in_string {
                    if ch == b'{' {
                        depth += 1;
                    } else if ch == b'}' {
                        depth -= 1;
                        if depth == 0 {
                            let candidate = &text[start..=j];
                            if serde_json::from_str::<serde_json::Value>(candidate).is_ok() {
                                return Some(candidate.to_string());
                            }
                            break;
                        }
                    }
                }
                j += 1;
            }
        }
        i += 1;
    }
    None
}
