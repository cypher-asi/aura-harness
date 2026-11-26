use super::super::AgentLoopConfig;
use super::*;
use serde_json::json;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

/// Serialize tests that swap the installed `aura_config` via
/// `install_for_test` so they cannot race each other on the
/// `implement_now_hard_block` toggle.
fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn install_block(enabled: bool) -> aura_config::ConfigGuard {
    let mut cfg = aura_config::current();
    cfg.agent.steering.implement_now_hard_block = enabled;
    aura_config::install_for_test(cfg)
}

fn mk_read_tool(id: &str, path: &str) -> ToolCallInfo {
    ToolCallInfo {
        id: id.to_string(),
        name: "read_file".to_string(),
        input: json!({"path": path}),
    }
}

fn mk_read_result(tool_use_id: &str) -> ToolCallResult {
    ToolCallResult {
        tool_use_id: tool_use_id.to_string(),
        content: "ok".to_string(),
        is_error: false,
        kind: aura_core_types::ToolResultKind::Ok,
        stop_loop: false,
        file_changes: Vec::new(),
        image: None,
    }
}

fn mk_write_tool(id: &str, name: &str, path: &str) -> ToolCallInfo {
    ToolCallInfo {
        id: id.to_string(),
        name: name.to_string(),
        input: json!({"path": path}),
    }
}

fn mk_write_result(tool_use_id: &str, path: &str) -> ToolCallResult {
    ToolCallResult {
        tool_use_id: tool_use_id.to_string(),
        content: "ok".to_string(),
        is_error: false,
        kind: aura_core_types::ToolResultKind::Ok,
        stop_loop: false,
        file_changes: vec![crate::types::FileChange {
            path: path.to_string(),
            kind: crate::types::FileChangeKind::Modify,
            lines_added: 1,
            lines_removed: 0,
        }],
        image: None,
    }
}

#[test]
fn failed_edit_guard_survives_reads_but_resets_after_mutation() {
    let config = AgentLoopConfig::for_agent("claude-test-model");
    let mut state = LoopState::new_for_tests(&config, vec![]);
    let call = mk_write_tool("edit", "edit_file", "test.txt");
    let mut failure = mk_read_result("edit");
    failure.is_error = true;
    failure.content = "No matching text".into();
    let mut warnings = vec![];
    for _ in 0..3 {
        assert!(guard_failed_edits(
            std::slice::from_ref(&call),
            std::slice::from_ref(&failure),
            &mut state,
            &mut warnings
        )
        .is_none());
        assert!(guard_failed_edits(
            &[mk_read_tool("read", "test.txt")],
            &[mk_read_result("read")],
            &mut state,
            &mut warnings
        )
        .is_none());
    }
    assert_eq!(warnings.len(), 1);
    assert!(guard_failed_edits(
        std::slice::from_ref(&call),
        std::slice::from_ref(&failure),
        &mut state,
        &mut warnings
    )
    .is_some());
    assert!(guard_failed_edits(
        std::slice::from_ref(&call),
        &[mk_write_result("edit", "test.txt")],
        &mut state,
        &mut warnings
    )
    .is_none());
    assert!(guard_failed_edits(&[call], &[failure], &mut state, &mut warnings).is_none());
}

#[test]
fn implement_now_block_is_disabled_when_config_opt_out() {
    let _guard = env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let _cfg = install_block(false);

    let config = AgentLoopConfig::for_agent("claude-test-model");
    let mut state = LoopState::new_for_tests(&config, vec![]);
    state.implement_now_injected = true;

    let calls = vec![
        mk_read_tool("toolu_read", "src/lib.rs"),
        ToolCallInfo {
            id: "toolu_search".to_string(),
            name: "search_code".to_string(),
            input: json!({"pattern": "Dedupe"}),
        },
    ];

    let (blocked, remaining) = partition_circling_duplicate_reads(&calls, &state);

    assert!(
        blocked.is_empty(),
        "opt-out: hard block must not fire when implement_now_hard_block=false"
    );
    assert_eq!(remaining.len(), 2);
}

#[test]
fn implement_now_blocks_exploration_until_a_write_lands() {
    let _guard = env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let _cfg = install_block(true);

    let config = AgentLoopConfig::for_agent("claude-test-model");
    let mut state = LoopState::new_for_tests(&config, vec![]);
    state.implement_now_injected = true;

    let calls = vec![
        mk_read_tool("toolu_read", "src/lib.rs"),
        ToolCallInfo {
            id: "toolu_search".to_string(),
            name: "search_code".to_string(),
            input: json!({"pattern": "Dedupe"}),
        },
        mk_write_tool("toolu_write", "edit_file", "src/lib.rs"),
        ToolCallInfo {
            id: "toolu_done".to_string(),
            name: "task_done".to_string(),
            input: json!({"notes": "done"}),
        },
    ];

    let (blocked, remaining) = partition_circling_duplicate_reads(&calls, &state);

    assert_eq!(blocked.len(), 2);
    assert!(blocked.iter().all(|r| r.is_error));
    assert!(blocked
        .iter()
        .all(|r| r.content.contains("implement_now has already fired")));
    assert_eq!(
        remaining
            .iter()
            .map(|t| t.name.as_str())
            .collect::<Vec<_>>(),
        vec!["edit_file", "task_done"]
    );
}

#[test]
fn implement_now_block_stops_after_first_successful_file_write() {
    let _guard = env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let _cfg = install_block(true);

    let config = AgentLoopConfig::for_agent("claude-test-model");
    let mut state = LoopState::new_for_tests(&config, vec![]);
    state.implement_now_injected = true;
    state.had_any_file_write = true;

    let calls = vec![mk_read_tool("toolu_read", "src/lib.rs")];
    let (blocked, remaining) = partition_circling_duplicate_reads(&calls, &state);

    assert!(blocked.is_empty());
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].name, "read_file");
}

/// Pin that `track_tool_effects` increments the exploration
/// telemetry counter on every successful exploration call. The
/// counter survives for telemetry / future re-introduction of a
/// real cap.
#[test]
fn exploration_count_is_incremented_by_track_tool_effects() {
    const CALLS: usize = 12;

    let config = AgentLoopConfig::for_agent("claude-test-model");
    let mut state = LoopState::new_for_tests(&config, vec![]);

    for i in 0..CALLS {
        let tool_id = format!("toolu_explore_{i}");
        let path = format!("src/file_{i}.rs");
        let to_execute = vec![mk_read_tool(&tool_id, &path)];
        let executed = vec![mk_read_result(&tool_id)];
        track_tool_effects(&to_execute, &executed, &mut state);
    }

    assert_eq!(state.exploration_state.count, CALLS);
    assert!(!state.had_any_write, "no writes were issued in this test");
    assert!(
        state.turn_diff.writes.is_empty(),
        "read_file calls must not count as writes in the turn diff"
    );
    assert_eq!(state.turn_diff.read_paths.len(), CALLS);
}

#[test]
fn successful_write_removes_path_from_session_read_cache() {
    let config = AgentLoopConfig::for_agent("claude-test-model");
    let mut state = LoopState::new_for_tests(&config, vec![]);
    state
        .session_read_paths
        .insert(PathBuf::from("src/inbox.rs"));

    let to_execute = vec![mk_write_tool("toolu_write", "edit_file", "src/inbox.rs")];
    let executed = vec![mk_write_result("toolu_write", "src/inbox.rs")];

    let any_success = track_tool_effects(&to_execute, &executed, &mut state);

    assert!(any_success);
    assert!(state.had_any_write);
    assert!(
        !state
            .session_read_paths
            .contains(&PathBuf::from("src/inbox.rs")),
        "writing a file must allow one fresh re-read of that path"
    );
    assert_eq!(
        state
            .read_after_write_allowances
            .get(&PathBuf::from("src/inbox.rs")),
        Some(&aura_config::READS_AFTER_WRITE_ALLOWANCE)
    );
}
