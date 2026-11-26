//! Where the time goes when the session loads an agent's chat, measured on
//! a temp `RocksDB` store holding an agent with a realistic large record:
//! the scan, the chat rebuild, the Record panel summaries, the plan path,
//! the model history, the single read a first open makes for the chat,
//! plan path and model history together, and the agent/channel list rows.
//!
//! Ignored by default:
//!
//! ```text
//! cargo test --release -p aura-app --test chat_load_perf -- --ignored --nocapture
//! ```
//!
//! `AURA_PERF_ENTRIES` sets how many record entries the agent has (default
//! `1000`). Everything here is the public entry point the loaded path uses;
//! where the path calls something `pub(crate)` (`scan_tail`, `ListRows`,
//! `send_channels_with`, `member_rows`) the nearest public entry is timed
//! and the line says so.

// Integration-test crate: test code may unwrap/expect.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use aura_app::agent_registry::AgentRegistry;
use aura_app::record_loader::{
    agent_summaries, archived_summaries, conversation_from, conversation_history, latest_plan_path,
    latest_plan_path_in, model_history, model_history_from, recent_record_summaries, ByHead,
    RecordTail,
};
use aura_core_types::{
    ActionId, AgentId, Effect, EffectKind, EffectStatus, PolicyDecision, PolicyRule, PolicySource,
    PromptPayload, RecordEntry, ToolExecution, ToolGateVerdict, ToolProposal, ToolResult,
    Transaction, TransactionType,
};
use aura_store_db::{RocksStore, WriteStore};
use aura_store_record::ReadStore;
use std::collections::HashSet;
use std::time::{Duration, Instant};
use tempfile::TempDir;

fn env_num(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(default)
}

fn print_row(stage: &str, d: Duration) {
    println!("{stage:<58} {:>10.3} ms", d.as_secs_f64() * 1000.0);
}

/// `bytes` in kilobytes, for the report lines.
#[allow(clippy::cast_precision_loss)] // a record is far below 2^52 bytes
fn kb(bytes: usize) -> f64 {
    bytes as f64 / 1024.0
}

/// `count` as a number, for the report lines.
#[allow(clippy::cast_precision_loss)] // a record count is far below 2^52
fn num(count: usize) -> f64 {
    count as f64
}

/// Run `f` once, then time it `runs` more times and report each.
fn time<T>(stage: &str, runs: usize, mut f: impl FnMut() -> T) -> T {
    let mut out = f();
    let start = Instant::now();
    for _ in 0..runs {
        out = f();
    }
    let each = start.elapsed() / u32::try_from(runs).unwrap_or(1).max(1);
    print_row(stage, each);
    out
}

/// A long assistant reply: prose, three fenced code blocks and two tables.
fn reply(turn: usize) -> String {
    let mut text = format!(
        "## Turn {turn}\n\n\
         The loader is off the hot path now. What each stage costs:\n\n\
         | stage | before | after |\n|---|---|---|\n\
         | scan | 41 ms | 12 ms |\n\
         | markdown | 180 ms | 22 ms |\n\n"
    );
    for block in 0..3 {
        text.push_str("```rust\n");
        for line in 0..10 {
            text.push_str(&format!(
                "fn stage_{block}_{line}(input: &[u8]) -> Result<usize, Error> {{\n    \
                 let parsed = parse(input).context(\"stage read\")?;\n    \
                 Ok(parsed.iter().filter(|row| row.is_live()).count())\n}}\n"
            ));
        }
        text.push_str("```\n\n| test | time |\n|---|---|\n| replays | 0.4 ms |\n\n");
    }
    text.push_str(&"This paragraph is repeated so one reply runs past four kilobytes, which a long session multiplies by hundreds. ".repeat(5));
    text
}

/// A tool result of a few kilobytes, shaped like a diff and a test log.
fn tool_output(turn: usize) -> String {
    let mut text = String::from(
        "--- a/src/record_loader.rs\n+++ b/src/record_loader.rs\n@@ -880,7 +880,7 @@\n",
    );
    for line in 0..24 {
        text.push_str(&format!("-    let from_seq = head_saturating(window, {line});\n+    let from_seq = head_saturating(window, {line}, {turn});\n"));
    }
    for line in 0..20 {
        text.push_str(&format!("test record_loader::case_{turn}_{line} ... ok\n"));
    }
    text.push_str("test result: ok. 44 passed; 0 failed; 0 ignored\n");
    text
}

/// Every entry of one turn: a prompt, thinking, twelve tool calls with
/// results and policies, two long replies, and every tenth turn a plan
/// write. Sequence numbers continue from `first_seq`.
fn turn(agent: AgentId, turn: usize, first_seq: u64) -> Vec<RecordEntry> {
    let mut out: Vec<RecordEntry> = Vec::new();
    let mut push = |entry: RecordEntry| out.push(entry);
    let mut draft: Vec<(TransactionType, Vec<u8>, Option<Vec<u8>>)> = Vec::new();
    draft.push((
        TransactionType::UserPrompt,
        PromptPayload::new(format!("Load pass {turn}: measure where the time goes.")).to_bytes(),
        None,
    ));
    for thought in 0..3 {
        draft.push((
            TransactionType::Reasoning,
            serde_json::to_vec(&serde_json::json!({
                "model": "claude-sonnet-5",
                "thinking": format!(
                    "The chat is slow to open. Pass {turn}, thought {thought}: the history \
                     is read, then every block is laid out. {}",
                    "Measuring each stage tells them apart. ".repeat(4)
                ),
                "thinking_ms": 4100,
                "input_tokens": 12_000u64,
                "output_tokens": 900u64,
            }))
            .expect("json"),
            None,
        ));
    }
    for call in 0..12 {
        let id = format!("t{turn}-{call}");
        let name = if call % 4 == 3 {
            "write_file"
        } else {
            "read_file"
        };
        let proposal = ToolProposal::new(
            id.clone(),
            name,
            serde_json::json!({
                "path": format!("crates/surface/aura-app/src/gen_{turn}_{call}.rs"),
                "content": "pub fn generated() -> usize { 42 }\n".repeat(40),
            }),
        );
        draft.push((
            TransactionType::ToolProposal,
            serde_json::to_vec(&proposal).expect("proposal"),
            None,
        ));
        let decision = PolicyDecision {
            tool_use_id: id.clone(),
            tool: name.to_string(),
            verdict: ToolGateVerdict::Approved,
            reason: None,
            decided_by: PolicyRule::new(PolicySource::User, "on"),
            trace: Vec::new(),
            approval: None,
            mode: "agent".to_string(),
            policy_hash: "abc123".to_string(),
            action_id: None,
        };
        draft.push((
            TransactionType::PolicyDecision,
            serde_json::to_vec(&decision).expect("decision"),
            None,
        ));
        let execution = ToolExecution {
            tool_use_id: id,
            tool: name.to_string(),
            args: proposal.args.clone(),
            decision: ToolGateVerdict::Approved,
            reason: None,
            result: None,
            is_error: false,
            duration_ms: Some(120),
            parent_agent_id: agent,
            originating_user_id: String::new(),
        };
        // The kernel no longer copies a tool's output into the payload: it
        // lives in the entry's effect, which the loader decodes.
        let result = ToolResult::success(name, tool_output(turn));
        draft.push((
            TransactionType::ToolExecution,
            serde_json::to_vec(&execution).expect("execution"),
            Some(serde_json::to_vec(&result).expect("tool result")),
        ));
    }
    for part in 0..2 {
        draft.push((
            TransactionType::AgentMsg,
            reply(turn * 2 + part).into_bytes(),
            None,
        ));
    }
    // Every tenth turn writes a plan, so `latest_plan_path` finds its tool
    // execution at the very end of the record: the worst case for it.
    if turn % 10 == 9 {
        let plan = ToolExecution {
            tool_use_id: format!("plan-{turn}"),
            tool: "create_plan".to_string(),
            args: serde_json::json!({ "name": "perf" }),
            decision: ToolGateVerdict::Approved,
            reason: None,
            result: Some(format!(
                "Plan saved: .aura/plans/pass-{turn}.plan.md\nCreated plan 'pass {turn}' with 3 todos:\n"
            )),
            is_error: false,
            duration_ms: Some(9),
            parent_agent_id: agent,
            originating_user_id: String::new(),
        };
        draft.push((
            TransactionType::ToolExecution,
            serde_json::to_vec(&plan).expect("plan"),
            None,
        ));
    }
    for (offset, (tx_type, payload, effect)) in draft.into_iter().enumerate() {
        let tx = Transaction::new_chained(agent, tx_type, payload, None);
        let seq = first_seq + offset as u64;
        let mut builder = RecordEntry::builder(seq, tx);
        if let Some(effect) = effect {
            builder = builder.effects(vec![Effect::new(
                ActionId::generate(),
                EffectKind::Agreement,
                EffectStatus::Committed,
                effect,
            )]);
        }
        push(builder.build());
    }
    out
}

/// Write `entries` entries of `agent` into `store`, oldest first. Returns
/// the bytes of payload written.
fn write_agent(store: &RocksStore, agent: AgentId, entries: usize) -> usize {
    let mut written = 0usize;
    let mut turn_no = 0usize;
    let mut bytes = 0usize;
    while written < entries {
        for entry in turn(agent, turn_no, written as u64 + 1) {
            store
                .append_entry_direct(agent, entry.seq, &entry)
                .expect("append entry");
            bytes += entry.tx.payload.len();
            written += 1;
            if written >= entries {
                return bytes;
            }
        }
        turn_no += 1;
    }
    bytes
}

fn main_bench(entries: usize) {
    let dir = TempDir::new().expect("tempdir");
    let data = dir.path().join("data");
    std::fs::create_dir_all(&data).expect("data dir");
    let launch = std::env::current_dir().expect("cwd");

    let start = Instant::now();
    let store = RocksStore::open(dir.path().join("db"), false).expect("open store");
    print_row("RocksStore::open (empty store)", start.elapsed());

    // The registry, with a few dozen agents so the list row work is real.
    let mut registry =
        AgentRegistry::open(aura_store_db::AgentStore::for_store(&store), &data, &launch)
            .expect("registry");
    let mut ids = vec![registry.active().agent_id()];
    for _ in 0..48 {
        ids.push(registry.create(None).expect("agent").agent_id());
    }
    let active = ids[0];
    let start = Instant::now();
    let bytes = write_agent(&store, active, entries);
    let took = start.elapsed();
    println!(
        "\nwrote {entries} entries for {} ({:.1} KB, {:.3} ms per entry)",
        active.to_hex(),
        kb(bytes),
        took.as_secs_f64() * 1000.0 / num(entries)
    );

    let store_read: &dyn ReadStore = &store;

    // The scan `scan_tail` does, straight on the store. `scan_tail` itself is
    // `pub(crate)`; this is the public read it makes.
    let head = store_read.get_head_seq(active).expect("head");
    let from = head.saturating_sub(999).max(1);
    time("store.scan_record (what scan_tail reads)", 5, || {
        store_read
            .scan_record(active, from, 1000)
            .expect("scan")
            .len()
    });

    let history = time("conversation_history (chat rebuild)", 3, || {
        conversation_history(store_read, active).expect("history")
    });
    println!(
        "    {} messages, {} tool/thinking steps",
        history.len(),
        history.iter().filter(|m| m.activity.is_some()).count()
    );

    // `shown_chats` is a `ByHead`, public: this is exactly what a switch
    // back to an agent already read costs.
    let mut shown: ByHead<Vec<aura_ui::events::MessageData>> = ByHead::default();
    print_row("shown_chats.get_or_read, cold (reads + decodes)", {
        let start = Instant::now();
        shown
            .get_or_read(store_read, active, || {
                conversation_history(store_read, active)
            })
            .expect("cold");
        start.elapsed()
    });
    time("shown_chats.get_or_read, warm (clones)", 5, || {
        shown
            .get_or_read(store_read, active, || {
                conversation_history(store_read, active)
            })
            .expect("warm")
            .len()
    });

    let records = time("recent_record_summaries (Record panel)", 3, || {
        recent_record_summaries(store_read, active).expect("records")
    });
    println!(
        "    {} rows, {:.1} KB of panel text",
        records.len(),
        kb(records
            .iter()
            .map(|row| row.message.len() + row.info.len() + row.detail.len())
            .sum::<usize>())
    );

    let plan = time("latest_plan_path", 5, || {
        latest_plan_path(store_read, active)
    });
    println!("    -> {plan:?}");

    let model = time("model_history (session restore)", 3, || {
        model_history(store_read, active).expect("model history")
    });
    println!("    {} model messages", model.len());

    // A first open reads the newest entries once, then rebuilds the chat
    // from them while the session opens with the plan path and the model
    // history from the same entries.
    let tail = time("RecordTail::read (the one read of a first open)", 5, || {
        RecordTail::read(store_read, active).expect("tail")
    });
    time("conversation_from (chat, from the tail)", 3, || {
        conversation_from(tail.entries()).len()
    });
    time("latest_plan_path_in + model_history_from (open)", 3, || {
        (
            latest_plan_path_in(tail.entries()),
            model_history_from(tail.entries()).len(),
        )
    });

    // The list the session sends on every switch.
    time("registry.tree() (rows to build)", 5, || {
        registry.tree().len()
    });
    let running = HashSet::new();
    time("agent_summaries (Agents panel rows)", 3, || {
        agent_summaries(&registry, store_read, active, &running).len()
    });
    time("archived_summaries (Archive rows)", 3, || {
        archived_summaries(&registry, store_read).len()
    });
    // `ListRows`, `send_agent_list` and `send_channels_with`/`member_rows` are
    // `pub(crate)`: `agent_summaries` above is the public path closest to
    // them and does the same per-row store reads (`LastActive::of`).
    time("registry.set_active (persists)", 3, || {
        registry.set_active(active).expect("set active");
    });
}

#[test]
#[ignore = "load test; run with --ignored --nocapture"]
fn chat_load_perf_backend() {
    let entries = env_num("AURA_PERF_ENTRIES", 1000);
    main_bench(entries);
}
