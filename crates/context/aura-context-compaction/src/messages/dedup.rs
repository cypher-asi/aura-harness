//! Folding of older read-only tool results that a later result repeats or
//! supersedes.

use aura_model::{ContentBlock, Message, ToolResultContent};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};

/// Read-only tools whose results are safe to fold by `content_hash`.
/// Single source of truth lives in `aura_config::CACHEABLE_TOOLS`.
use aura_config::CACHEABLE_TOOLS as READ_ONLY_DEDUP_TOOLS;

use super::blocks::tool_result_text;

/// Marker the `read_file` tool ends a cut result with.
const READ_TRUNCATION_MARKER: &str = "\n... [truncated";

/// Inclusive line window of a `read_file` call.
type ReadWindow = (usize, usize);

/// Stable hex digest matching the one stamped by
/// `aura_exec_tools::fs_tools::read::content_hash_hex`. Re-derived locally
/// from the rendered ToolResult bytes because compaction sees the
/// `Message`-level shape (with no metadata side-channel) — and because
/// `DefaultHasher` is deterministic for the same bytes, the hash
/// produced here matches the one originally stamped by the read tool
/// for the same content.
fn dedup_content_hash_hex(bytes: &[u8]) -> String {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// Build a `tool_use_id -> tool_name` index by scanning every
/// `ContentBlock::ToolUse` in `messages`. The companion
/// `ContentBlock::ToolResult` only carries `tool_use_id`, so dedup
/// needs the index to filter by `READ_ONLY_DEDUP_TOOLS`.
fn tool_use_name_index(messages: &[Message]) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for msg in messages {
        for block in &msg.content {
            if let ContentBlock::ToolUse { id, name, .. } = block {
                map.insert(id.clone(), name.clone());
            }
        }
    }
    map
}

/// Look up the `path` argument from the `ContentBlock::ToolUse` whose
/// `id == tool_use_id`. Used to populate the `path` field of the dedup
/// marker so the model can still reason about which file the folded
/// read referred to. Returns `None` if the input has no `path` string
/// (e.g. `search_code`).
fn tool_use_input_path(messages: &[Message], tool_use_id: &str) -> Option<String> {
    for msg in messages {
        for block in &msg.content {
            if let ContentBlock::ToolUse { id, input, .. } = block {
                if id == tool_use_id {
                    return input.get("path").and_then(Value::as_str).map(String::from);
                }
            }
        }
    }
    None
}

/// Detect a previously-emitted dedup marker so a second pass over the
/// same `messages` doesn't fold it into a marker-of-a-marker. The
/// marker shape is `{ "_redacted": "<read_only_tool>", ... }`; any
/// other `_redacted` shape (e.g. the write-input shape produced by
/// [`RedactionMarker`](super::RedactionMarker)) is ignored here.
fn is_dedup_marker_text(text: &str) -> bool {
    let Ok(Value::Object(map)) = serde_json::from_str::<Value>(text) else {
        return false;
    };
    let Some(redacted) = map.get("_redacted").and_then(Value::as_str) else {
        return false;
    };
    READ_ONLY_DEDUP_TOOLS.contains(&redacted)
}

/// A read-only tool result eligible for content-hash dedup.
struct DedupCandidate {
    tool_use_id: String,
    tool_name: String,
    hash: String,
}

/// The dedup candidate `block` is, if it is a read-only tool result that
/// is not already a dedup marker.
fn dedup_candidate(
    block: &ContentBlock,
    name_index: &HashMap<String, String>,
) -> Option<DedupCandidate> {
    let ContentBlock::ToolResult {
        tool_use_id,
        content,
        ..
    } = block
    else {
        return None;
    };
    let rendered_text = tool_result_text(content);
    if is_dedup_marker_text(&rendered_text) {
        return None;
    }
    let tool_name = name_index.get(tool_use_id)?;
    if !READ_ONLY_DEDUP_TOOLS.contains(&tool_name.as_str()) {
        return None;
    }
    Some(DedupCandidate {
        tool_use_id: tool_use_id.clone(),
        tool_name: tool_name.clone(),
        hash: dedup_content_hash_hex(rendered_text.as_bytes()),
    })
}

fn dedup_marker_text(messages: &[Message], candidate: &DedupCandidate) -> String {
    let path = tool_use_input_path(messages, &candidate.tool_use_id).unwrap_or_default();
    let marker = serde_json::json!({
        "_redacted": candidate.tool_name,
        "path": path,
        "content_hash": candidate.hash,
        "note": "see later identical read",
    });
    marker.to_string()
}

/// Collapse older read-only tool results whose `content_hash` matches
/// a later occurrence to a short structured marker, keeping the newest
/// copy verbatim. Returns the number of folds applied.
///
/// Walks newest-to-oldest. For each `ContentBlock::ToolResult` whose
/// matching `ContentBlock::ToolUse` names a tool in
/// `READ_ONLY_DEDUP_TOOLS`, the helper hashes the rendered tool
/// output text and tracks `(tool_name, content_hash)` in a set. The
/// first time a hash is seen, the message is kept verbatim and the
/// hash is recorded; on every subsequent (older) occurrence of the
/// same `(tool_name, content_hash)` pair, the tool result's text is
/// replaced with a one-line JSON marker:
///
/// ```json
/// {"_redacted":"read_file","path":"<path>","content_hash":"<hash>","note":"see later identical read"}
/// ```
///
/// The message structure (role, `tool_use_id`, `is_error`) is left
/// untouched so the assistant/tool-result pairing the Anthropic API
/// requires stays intact — only the text content shrinks. Existing
/// dedup markers are detected via `is_dedup_marker_text` and skipped
/// so re-running compaction does not collapse markers into
/// markers-of-markers.
///
/// Exposed publicly so the Phase 6 Shamir replay harness in
/// `aura-agent` can drive the same dedup pass against its captured
/// transcript without going through the full `compact_messages`
/// pressure-tier dance. Production callers should still go through
/// [`compact_messages`](super::compact_messages), which is the only path
/// that decides whether dedup should fire for a given pressure.
pub fn dedup_read_results_by_content_hash(messages: &mut [Message]) -> usize {
    if messages.is_empty() {
        return 0;
    }

    let name_index = tool_use_name_index(messages);
    let mut seen: HashSet<(String, String)> = HashSet::new();
    let mut folded = 0usize;

    for i in (0..messages.len()).rev() {
        for block_idx in 0..messages[i].content.len() {
            let Some(candidate) = dedup_candidate(&messages[i].content[block_idx], &name_index)
            else {
                continue;
            };
            let key = (candidate.tool_name.clone(), candidate.hash.clone());
            if !seen.contains(&key) {
                seen.insert(key);
                continue;
            }
            let marker = dedup_marker_text(messages, &candidate);
            if let ContentBlock::ToolResult { content, .. } = &mut messages[i].content[block_idx] {
                *content = ToolResultContent::Text(marker);
                folded += 1;
            }
        }
    }
    folded
}

/// The lines a `read_file` call with arguments `input` asked for, as an
/// inclusive range; an open end runs to the end of the file.
fn read_window(input: &Value) -> ReadWindow {
    let line = |key: &str| {
        input
            .get(key)
            .and_then(Value::as_u64)
            .and_then(|n| usize::try_from(n).ok())
    };
    (
        line("start_line").unwrap_or(1).max(1),
        line("end_line").unwrap_or(usize::MAX),
    )
}

/// Index every `read_file` call by `tool_use_id`: its normalized path and
/// the line window it asked for.
fn read_file_requests(messages: &[Message]) -> HashMap<String, (String, ReadWindow)> {
    let mut reads = HashMap::new();
    for msg in messages {
        for block in &msg.content {
            if let ContentBlock::ToolUse {
                id, name, input, ..
            } = block
            {
                if name == "read_file" {
                    if let Some(path) = input.get("path").and_then(Value::as_str) {
                        let path = path.trim_start_matches("./").to_string();
                        reads.insert(id.clone(), (path, read_window(input)));
                    }
                }
            }
        }
    }
    reads
}

/// Fold older `read_file` results that a later read of the same path
/// shows in full: the later read is the file as it is now, so the older
/// copy is at best a duplicate and at worst stale (the file was edited in
/// between). Returns the number of folds applied.
///
/// A later read supersedes an older one when its line window contains the
/// older window and it was not cut short (no truncation marker) and did
/// not fail. Unlike [`dedup_read_results_by_content_hash`] this catches
/// re-reads after an edit, whose bytes differ. Folded results become the
/// same `_redacted` marker shape, so neither pass folds a marker again,
/// and the tool_use / tool_result pairing stays intact.
pub fn supersede_older_reads(messages: &mut [Message]) -> usize {
    let reads = read_file_requests(messages);
    if reads.is_empty() {
        return 0;
    }

    let mut complete: HashMap<String, Vec<ReadWindow>> = HashMap::new();
    let mut folded = 0usize;
    for msg in messages.iter_mut().rev() {
        for block in &mut msg.content {
            let ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } = block
            else {
                continue;
            };
            let Some((path, (start, end))) = reads.get(tool_use_id.as_str()) else {
                continue;
            };
            let ToolResultContent::Text(text) = content else {
                continue;
            };
            if *is_error || is_dedup_marker_text(text) {
                continue;
            }
            let windows = complete.entry(path.clone()).or_default();
            if windows.iter().any(|(s, e)| s <= start && end <= e) {
                let marker = serde_json::json!({
                    "_redacted": "read_file",
                    "path": path,
                    "note": "superseded by a later read of the same lines",
                });
                *content = ToolResultContent::Text(marker.to_string());
                folded += 1;
            } else if !text.contains(READ_TRUNCATION_MARKER) {
                windows.push((*start, *end));
            }
        }
    }
    folded
}
