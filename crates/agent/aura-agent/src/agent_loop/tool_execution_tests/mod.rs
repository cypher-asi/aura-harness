use aura_model::ContentBlock;

use crate::types::ToolCallInfo;

use crate::types::ToolCallResult;

use aura_config::tool_result_cache_key;

use super::search_cache::normalized_search_key;

use super::tool_execution::{split_cached, truncate_preview, update_cache};

use super::tool_pipeline::push_tool_result_message;

use super::ToolResultCache;

fn ok_result(id: &str, content: &str) -> ToolCallResult {
    ToolCallResult {
        tool_use_id: id.to_string(),
        content: content.to_string(),
        is_error: false,
        kind: aura_core_types::ToolResultKind::Ok,
        stop_loop: false,
        file_changes: Vec::new(),
        image: None,
    }
}

fn call(id: &str, name: &str, input: serde_json::Value) -> ToolCallInfo {
    ToolCallInfo {
        id: id.to_string(),
        name: name.to_string(),
        input,
    }
}

/// A cache holding one read of `src/lib.rs` and one search.
fn cache_with_a_read_and_a_search() -> (ToolResultCache, ToolCallInfo, ToolCallInfo) {
    let read = call("r", "read_file", serde_json::json!({"path": "src/lib.rs"}));
    let search = call("s", "search_code", serde_json::json!({"pattern": "greet"}));
    let mut cache = ToolResultCache::default();
    update_cache(
        &mut cache,
        &[read.clone(), search.clone()],
        &[ok_result("r", "before"), ok_result("s", "hits")],
    );
    (cache, read, search)
}

// ---------------------------------------------------------------------------
// Phase 1: range-aware `read_file` cache + path-scoped invalidation
// ---------------------------------------------------------------------------

/// Build the line-numbered `read_file` rendering the way
/// `aura_exec_tools::fs_tools::read::fs_read` emits it (`{:>6}|{content}`,
/// newline-joined). Each line body is the literal `body` string.
fn fake_read_file_rendered(start: usize, end: usize, body: &str) -> String {
    (start..=end)
        .map(|n| format!("{n:>6}|{body}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Drive a single `read_file` call through the cache surface and
/// record whether the underlying executor would have been invoked.
/// Returns the cached `ToolCallResult` (synthesising one with
/// `responder` on miss).
fn drive_read_file(
    cache: &mut ToolResultCache,
    id: &str,
    path: &str,
    start: Option<usize>,
    end: Option<usize>,
    fs_read_calls: &mut usize,
    responder: impl FnOnce(Option<usize>, Option<usize>) -> String,
) -> ToolCallResult {
    let mut input = serde_json::json!({"path": path});
    if let Some(s) = start {
        input["start_line"] = serde_json::json!(s);
    }
    if let Some(e) = end {
        input["end_line"] = serde_json::json!(e);
    }
    let call = ToolCallInfo {
        id: id.to_string(),
        name: "read_file".to_string(),
        input,
    };

    let (cached, uncached) = split_cached(std::slice::from_ref(&call), cache);
    if let Some(hit) = cached.into_iter().next() {
        assert!(
            uncached.is_empty(),
            "miss + hit on the same call is invalid"
        );
        return hit;
    }

    assert_eq!(uncached.len(), 1, "miss must produce one uncached entry");
    *fs_read_calls += 1;
    let content = responder(start, end);
    let result = ToolCallResult {
        tool_use_id: call.id.clone(),
        content,
        is_error: false,
        kind: aura_core_types::ToolResultKind::Ok,
        stop_loop: false,
        file_changes: Vec::new(),
        image: None,
    };
    update_cache(
        cache,
        std::slice::from_ref(&call),
        std::slice::from_ref(&result),
    );
    result
}

mod range_cache;
mod result_cache;
mod transcript;
