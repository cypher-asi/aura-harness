use super::*;

/// Cached read hits now reinsert the cache content verbatim — the
/// silent cache-result-shaping path was removed because it was
/// rewriting `read_file` results without telling the model. This test
/// pins the new contract: the same long content goes back into the
/// transcript byte-for-byte.
#[test]
fn cached_read_hits_reinsert_content_verbatim() {
    let call = ToolCallInfo {
        id: "tool_1".to_string(),
        name: "read_file".to_string(),
        input: serde_json::json!({"path": "src/lib.rs"}),
    };
    let mut cache = ToolResultCache::default();
    let long_content = "a".repeat(9_000);
    cache.exact.insert(
        tool_result_cache_key(&call.name, &call.input),
        long_content.clone(),
    );

    let (cached, uncached) = split_cached(&[call], &cache);

    assert!(uncached.is_empty());
    assert_eq!(cached.len(), 1);
    assert_eq!(cached[0].content, long_content);
}

#[test]
fn fuzzy_cache_hits_after_alternation_reorder() {
    // Seed the cache by running update_cache with a successful
    // search_code result for pattern "pub fn generate|NeuralKey".
    let seed = ToolCallInfo {
        id: "tool_seed".to_string(),
        name: "search_code".to_string(),
        input: serde_json::json!({"pattern": "pub fn generate|NeuralKey"}),
    };
    let seed_result = ToolCallResult {
        tool_use_id: "tool_seed".to_string(),
        content: "seed-hits".to_string(),
        is_error: false,
        kind: aura_core_types::ToolResultKind::Ok,
        stop_loop: false,
        file_changes: Vec::new(),
        image: None,
    };
    let mut cache = ToolResultCache::default();
    update_cache(
        &mut cache,
        std::slice::from_ref(&seed),
        std::slice::from_ref(&seed_result),
    );
    assert!(!cache.exact.is_empty(), "exact cache should be populated");
    assert!(!cache.fuzzy.is_empty(), "fuzzy cache should be populated");

    // Now a later call with the alternation terms in a different order
    // — it should MISS the exact cache but HIT the fuzzy cache.
    let reordered = ToolCallInfo {
        id: "tool_query".to_string(),
        name: "search_code".to_string(),
        input: serde_json::json!({"pattern": "NeuralKey|pub fn generate"}),
    };

    assert!(
        !cache
            .exact
            .contains_key(&tool_result_cache_key(&reordered.name, &reordered.input)),
        "exact key should not match the reordered alternation"
    );

    let (cached, uncached) = split_cached(&[reordered], &cache);
    assert!(
        uncached.is_empty(),
        "fuzzy cache should satisfy the reordered query without executing"
    );
    assert_eq!(cached.len(), 1);
    assert_eq!(cached[0].tool_use_id, "tool_query");
    assert!(!cached[0].is_error);
}

#[test]
fn a_cache_over_its_budget_starts_over() {
    let calls: Vec<ToolCallInfo> = (0..5)
        .map(|i| ToolCallInfo {
            id: format!("read_{i}"),
            name: "read_file".to_string(),
            input: serde_json::json!({"path": format!("src/big_{i}.rs")}),
        })
        .collect();
    let results: Vec<ToolCallResult> = calls
        .iter()
        .map(|call| ToolCallResult {
            tool_use_id: call.id.clone(),
            content: "x".repeat(super::super::cache::MAX_CACHE_BYTES / 8),
            is_error: false,
            kind: aura_core_types::ToolResultKind::Ok,
            stop_loop: false,
            file_changes: Vec::new(),
            image: None,
        })
        .collect();
    let mut cache = ToolResultCache::default();
    update_cache(&mut cache, &calls[..1], &results[..1]);
    assert!(!cache.exact.is_empty(), "one big read fits");
    update_cache(&mut cache, &calls, &results);
    assert!(cache.exact.is_empty(), "five copies twice over don't");
    assert!(cache.bytes() <= super::super::cache::MAX_CACHE_BYTES);
}

#[test]
fn write_clears_both_caches() {
    // Seed both caches with a search_code result.
    let seed = ToolCallInfo {
        id: "tool_s".to_string(),
        name: "search_code".to_string(),
        input: serde_json::json!({"pattern": "NeuralKey"}),
    };
    let seed_result = ToolCallResult {
        tool_use_id: "tool_s".to_string(),
        content: "hits".to_string(),
        is_error: false,
        kind: aura_core_types::ToolResultKind::Ok,
        stop_loop: false,
        file_changes: Vec::new(),
        image: None,
    };
    let mut cache = ToolResultCache::default();
    update_cache(
        &mut cache,
        std::slice::from_ref(&seed),
        std::slice::from_ref(&seed_result),
    );
    assert!(!cache.exact.is_empty());
    assert!(!cache.fuzzy.is_empty());

    // A successful write_file goes through update_cache. The write
    // itself is not cacheable, but the workspace-global slice of the
    // exact cache (`search_code`/`find_files`) and the fuzzy cache
    // must both be cleared. Path-scoped entries (`read_file`,
    // `list_files`, `stat_file`) only invalidate when they overlap
    // the written path — covered by
    // `write_invalidates_only_overlapping_path`.
    let write_call = ToolCallInfo {
        id: "tool_w".to_string(),
        name: "write_file".to_string(),
        input: serde_json::json!({"path": "x.txt", "content": "hello"}),
    };
    let write_result = ToolCallResult {
        tool_use_id: "tool_w".to_string(),
        content: "Wrote 5 bytes to x.txt".to_string(),
        is_error: false,
        kind: aura_core_types::ToolResultKind::Ok,
        stop_loop: false,
        file_changes: Vec::new(),
        image: None,
    };
    update_cache(
        &mut cache,
        std::slice::from_ref(&write_call),
        std::slice::from_ref(&write_result),
    );

    assert!(
        cache.exact.is_empty(),
        "search_code exact entries must be cleared by any successful write"
    );
    assert!(
        cache.fuzzy.is_empty(),
        "fuzzy cache must be cleared alongside the exact search_code slice"
    );
}

#[test]
fn failed_write_does_not_clear_caches() {
    // Regression guard for the "any_write" check: only *successful*
    // writes should clear the caches. A failed write_file (e.g. the
    // new chunk guard) must leave the caches intact.
    let seed = ToolCallInfo {
        id: "tool_s".to_string(),
        name: "search_code".to_string(),
        input: serde_json::json!({"pattern": "NeuralKey"}),
    };
    let seed_result = ToolCallResult {
        tool_use_id: "tool_s".to_string(),
        content: "hits".to_string(),
        is_error: false,
        kind: aura_core_types::ToolResultKind::Ok,
        stop_loop: false,
        file_changes: Vec::new(),
        image: None,
    };
    let mut cache = ToolResultCache::default();
    update_cache(
        &mut cache,
        std::slice::from_ref(&seed),
        std::slice::from_ref(&seed_result),
    );

    let failed_write = ToolCallInfo {
        id: "tool_w".to_string(),
        name: "write_file".to_string(),
        input: serde_json::json!({"path": "big.rs", "content": "x"}),
    };
    let failed_result = ToolCallResult {
        tool_use_id: "tool_w".to_string(),
        content: "[CHUNK_GUARD] oversized".to_string(),
        is_error: true,
        kind: aura_core_types::ToolResultKind::AgentError,
        stop_loop: false,
        file_changes: Vec::new(),
        image: None,
    };
    update_cache(
        &mut cache,
        std::slice::from_ref(&failed_write),
        std::slice::from_ref(&failed_result),
    );

    assert!(
        !cache.exact.is_empty(),
        "failed write must NOT clear the exact cache"
    );
    assert!(
        !cache.fuzzy.is_empty(),
        "failed write must NOT clear the fuzzy cache"
    );
}

#[test]
fn a_subtask_or_a_command_drops_every_cached_read() {
    let ran = [
        ("task", serde_json::json!({"prompt": "edit src/lib.rs"})),
        ("run_command", serde_json::json!({"command": "make fmt"})),
        ("mcp_some_tool", serde_json::json!({})),
    ];
    for (name, input) in ran {
        let (mut cache, read, search) = cache_with_a_read_and_a_search();
        assert!(split_cached(&[read.clone(), search.clone()], &cache)
            .1
            .is_empty());

        let mut failed = ok_result("t", "exit code 1");
        failed.is_error = true;
        update_cache(&mut cache, &[call("t", name, input)], &[failed]);

        let (cached, uncached) = split_cached(&[read, search], &cache);
        assert!(cached.is_empty(), "{name} left {} cached", cached.len());
        assert_eq!(uncached.len(), 2, "both are read again after {name}");
        assert!(cache.read_file_by_path.is_empty());
    }
}

#[test]
fn a_read_beside_a_subtask_is_not_cached() {
    let mut cache = ToolResultCache::default();
    let read = call("r", "read_file", serde_json::json!({"path": "src/lib.rs"}));
    update_cache(
        &mut cache,
        &[
            read.clone(),
            call("t", "task", serde_json::json!({"prompt": "edit"})),
        ],
        &[ok_result("r", "maybe before"), ok_result("t", "Done.")],
    );
    assert_eq!(split_cached(&[read], &cache).1.len(), 1);
}

#[test]
fn tools_that_change_no_files_keep_the_cache() {
    let (mut cache, read, search) = cache_with_a_read_and_a_search();
    for name in aura_config::FILE_NEUTRAL_TOOLS {
        update_cache(
            &mut cache,
            &[call("n", name, serde_json::json!({}))],
            &[ok_result("n", "ok")],
        );
    }
    let (cached, uncached) = split_cached(&[read, search], &cache);
    assert_eq!((cached.len(), uncached.len()), (2, 0));
}

#[test]
fn split_cached_prefers_exact_over_fuzzy_when_both_match() {
    // Regression guard for the "exact-match stays primary" rule.
    let call = ToolCallInfo {
        id: "tool_1".to_string(),
        name: "search_code".to_string(),
        input: serde_json::json!({"pattern": "NeuralKey"}),
    };
    let mut cache = ToolResultCache::default();
    cache.exact.insert(
        tool_result_cache_key(&call.name, &call.input),
        "exact-hit".to_string(),
    );
    cache.fuzzy.insert(
        normalized_search_key(&call.name, &call.input).unwrap(),
        "fuzzy-hit".to_string(),
    );

    let (cached, uncached) = split_cached(&[call], &cache);
    assert!(uncached.is_empty());
    assert_eq!(cached.len(), 1);
    // The cached content may be summarized, but it should be derived
    // from the exact hit, not the fuzzy one.
    assert!(
        cached[0].content.contains("exact-hit")
            || cached[0].content.contains("Cached result reused"),
        "expected exact-hit to be preferred, got: {}",
        cached[0].content
    );
    assert!(
        !cached[0].content.contains("fuzzy-hit"),
        "fuzzy value should not leak when exact hit exists"
    );
}

#[test]
fn truncate_preview_uses_ascii_marker() {
    let preview = truncate_preview("abcdef", 3);
    assert_eq!(preview, "abc...");
    assert!(!preview.contains('\u{2026}'));
}
