use super::{GateLive, RECENT_DECISIONS};
use crate::record::SCAN_BATCH;
use aura_agent_kernel::RecordObserver;
use aura_config::GateConfig;
use aura_core_auth::ProviderKind;
use aura_core_types::{
    AgentId, Hash, OutcomePayload, OutcomeSignal, PromptCategory, PromptPayload, RecordEntry,
    Route, RouteLevel, RouteReason, Transaction,
};
use aura_gate_route::stats::GateStats;
use aura_gate_route::{candidates, SplitMix64, Work};
use aura_model::model_catalog::catalog;
use aura_store_db::{ReadStore, RocksStore, WriteStore};
use std::sync::Arc;
use tempfile::TempDir;

const HAIKU: &str = "claude/claude-haiku-4-5";

/// Commit `tx` to `agent_id`'s record in `store`, as a kernel would,
/// returning the entry.
fn commit(store: &RocksStore, agent_id: AgentId, tx: Transaction) -> RecordEntry {
    let seq = store.get_head_seq(agent_id).unwrap() + 1;
    let entry = RecordEntry::builder(seq, tx).build();
    store.append_entry_direct(agent_id, seq, &entry).unwrap();
    entry
}

fn prompt(agent_id: AgentId, text: &str) -> Transaction {
    let mut payload = PromptPayload::new(text);
    payload.category = Some(PromptCategory::Question);
    payload.route = Route::turn(HAIKU, RouteReason::Default);
    Transaction::user_prompt_with(agent_id, &payload)
}

fn outcome(agent_id: AgentId, prompt_tx: Hash, signal: OutcomeSignal) -> Transaction {
    Transaction::outcome(
        agent_id,
        &OutcomePayload {
            prompt_tx,
            category: PromptCategory::Question,
            model: HAIKU.to_string(),
            success: signal.success(),
            signal,
            cost_usd: 0.01,
            subtask_cost_usd: 0.0,
            latency_ms: 5,
        },
    )
}

#[test]
fn rebuilding_from_the_log_matches_the_live_updates() {
    let db = TempDir::new().unwrap();
    let store = RocksStore::open(db.path(), false).unwrap();
    let agents = [AgentId::generate(), AgentId::generate()];
    let live = GateLive::default();
    for (i, &agent_id) in agents.iter().enumerate() {
        for n in 0..(SCAN_BATCH + 3) / 2 {
            let text = format!("question {i} {n}");
            let p = commit(&store, agent_id, prompt(agent_id, &text));
            live.on_entry(&p);
            let signal = if n % 4 == 0 {
                OutcomeSignal::ChecksFailed
            } else {
                OutcomeSignal::ChecksPassed
            };
            live.on_entry(&commit(
                &store,
                agent_id,
                outcome(agent_id, p.tx.hash, signal),
            ));
            if n % 7 == 0 {
                let rated = outcome(agent_id, p.tx.hash, OutcomeSignal::JudgedFail);
                live.on_entry(&commit(&store, agent_id, rated));
            }
        }
    }

    let rebuilt = GateLive::default();
    let read = rebuilt.rebuild(&store, &agents);
    assert!(read > SCAN_BATCH, "the scan crosses batches: {read}");
    let (live, rebuilt) = (live.stats(), rebuilt.stats());
    let rows = |stats: &GateStats| -> Vec<(String, u32, u32)> {
        stats
            .rows()
            .into_iter()
            .map(|(key, row)| (key.model.clone(), row.tries, row.successes))
            .collect()
    };
    assert_eq!(rows(&rebuilt), rows(&live));
    let row = live
        .row(RouteLevel::Turn, PromptCategory::Question, HAIKU)
        .unwrap();
    assert_eq!(
        row.tries,
        u32::try_from(2 * ((SCAN_BATCH + 3) / 2)).unwrap()
    );
    let rebuilt_row = rebuilt
        .row(RouteLevel::Turn, PromptCategory::Question, HAIKU)
        .unwrap();
    assert!((row.cost_usd - rebuilt_row.cost_usd).abs() < 1e-9);
}

#[tokio::test]
async fn a_background_rebuild_overlapping_live_entries_counts_each_prompt_once() {
    let db = TempDir::new().unwrap();
    let store = Arc::new(RocksStore::open(db.path(), false).unwrap());
    let agent_id = AgentId::generate();
    let live = Arc::new(GateLive::default());
    let p = commit(&store, agent_id, prompt(agent_id, "what is gate?"));
    let o = commit(
        &store,
        agent_id,
        outcome(agent_id, p.tx.hash, OutcomeSignal::ChecksPassed),
    );
    live.on_entry(&o);
    live.on_entry(&p);
    live.rebuild(store.as_ref(), &[agent_id]);
    let rated = commit(
        &store,
        agent_id,
        outcome(agent_id, p.tx.hash, OutcomeSignal::JudgedFail),
    );
    live.on_entry(&rated);
    live.rebuild(store.as_ref(), &[agent_id]);
    let row = live
        .stats()
        .row(RouteLevel::Turn, PromptCategory::Question, HAIKU)
        .copied()
        .unwrap();
    assert_eq!((row.tries, row.successes), (1, 0));
}

#[test]
fn the_table_lent_out_is_the_table_as_it_is_now() {
    let live = GateLive::default();
    let tries = |stats: &GateStats| {
        stats
            .row(RouteLevel::Turn, PromptCategory::Question, HAIKU)
            .map(|row| row.tries)
    };
    assert_eq!(live.with_stats(tries), None);
    live.learn(
        RouteLevel::Turn,
        (PromptCategory::Question, None),
        HAIKU,
        None,
        (3, true),
    );
    assert_eq!(live.with_stats(tries), Some(3));
    assert_eq!(live.with_stats(tries), tries(&live.stats()));
}

#[test]
fn decisions_are_remembered_up_to_the_limit() {
    let live = GateLive::with_rng(GateConfig::defaults(), SplitMix64::seeded(1));
    let candidates = candidates(
        &catalog("local/qwen3:4b", &[]),
        |p| p == ProviderKind::Claude,
        |_| false,
    );
    for _ in 0..RECENT_DECISIONS + 5 {
        live.decide(&Work::turn(PromptCategory::Debugging), &candidates)
            .unwrap();
    }
    let recent = live.recent();
    assert_eq!(recent.len(), RECENT_DECISIONS);
    assert!(recent
        .iter()
        .all(|r| r.category == PromptCategory::Debugging && r.level == RouteLevel::Turn));
    assert!(live
        .decide(&Work::turn(PromptCategory::Debugging), &[])
        .is_none());
    assert_eq!(live.recent().len(), RECENT_DECISIONS);
}

/// How many outcomes `live`'s table holds for Haiku's questions.
fn haiku_tries(live: &GateLive) -> Option<u32> {
    live.with_stats(|stats| {
        stats
            .row(RouteLevel::Turn, PromptCategory::Question, HAIKU)
            .map(|row| row.tries)
    })
}

#[test]
fn a_snapshot_is_detached_from_the_table_it_was_taken_of() {
    let live = GateLive::default();
    let learn = |live: &GateLive, n| {
        live.learn(
            RouteLevel::Turn,
            (PromptCategory::Question, None),
            HAIKU,
            None,
            (n, true),
        );
    };
    learn(&live, 2);
    let snapshot = live.snapshot(7);
    assert_eq!(
        haiku_tries(&snapshot),
        Some(2),
        "it starts as the table was"
    );

    learn(&live, 3);
    let agent_id = AgentId::generate();
    let p = prompt(agent_id, "what is gate?");
    let p = RecordEntry::builder(1, p).build();
    let o = outcome(agent_id, p.tx.hash, OutcomeSignal::ChecksFailed);
    live.on_entry(&p);
    live.on_entry(&RecordEntry::builder(2, o).build());
    assert_eq!(haiku_tries(&live), Some(6));
    assert_eq!(
        haiku_tries(&snapshot),
        Some(2),
        "the original's learning stays out"
    );

    learn(&snapshot, 4);
    snapshot.on_entry(&RecordEntry::builder(3, prompt(agent_id, "and again?")).build());
    assert_eq!(haiku_tries(&snapshot), Some(6));
    assert_eq!(
        haiku_tries(&live),
        Some(6),
        "and the snapshot's stays in it"
    );
}

#[test]
fn snapshots_draw_from_their_own_seeded_generator() {
    let live = GateLive::with_rng(GateConfig::defaults(), SplitMix64::seeded(1));
    let draws = |live: &GateLive| -> Vec<bool> { (0..64).map(|_| live.sampled(0.5)).collect() };
    let (one, same, other) = (live.snapshot(11), live.snapshot(11), live.snapshot(12));
    // Draws on the original or another snapshot move no snapshot's.
    let _ = draws(&live);
    let _ = draws(&other);
    let first = draws(&one);
    assert_eq!(first, draws(&same), "the same seed draws the same");
    assert_ne!(
        first,
        draws(&live.snapshot(12)),
        "another seed draws others"
    );
    assert_ne!(first, draws(&one), "a snapshot's draws go on");
}

#[test]
fn a_snapshot_shares_the_attempt_log_and_what_agents_are_on() {
    let dir = TempDir::new().unwrap();
    let live = GateLive::default();
    live.log_attempts_to(aura_gate_attempts::AttemptLog::in_data_dir(dir.path()));
    let agent_id = AgentId::generate();
    live.note_model(agent_id, HAIKU);
    let snapshot = live.snapshot(3);
    assert_eq!(snapshot.model_of(agent_id).as_deref(), Some(HAIKU));
    assert_eq!(
        snapshot.attempt_log().map(|log| log.path().to_path_buf()),
        live.attempt_log().map(|log| log.path().to_path_buf())
    );
    let candidates = candidates(
        &catalog("local/qwen3:4b", &[]),
        |p| p == ProviderKind::Claude,
        |_| false,
    );
    snapshot
        .decide(&Work::turn(PromptCategory::Debugging), &candidates)
        .unwrap();
    assert_eq!(
        live.recent().len(),
        1,
        "/gate lists the snapshot's decisions"
    );
}
