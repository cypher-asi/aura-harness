use super::*;
use aura_core_types::{OutcomePayload, OutcomeSignal, PromptPayload, Route, Transaction};
use aura_store_db::RocksStore;
use tempfile::TempDir;

const HAIKU: &str = "claude/claude-haiku-4-5";

/// Commit `tx` to `agent_id`'s record in `store`, as a kernel would,
/// returning the entry.
fn commit(store: &RocksStore, agent_id: AgentId, tx: Transaction) -> RecordEntry {
    use aura_store_db::WriteStore;
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

fn outcome(
    agent_id: AgentId,
    prompt_tx: aura_core_types::Hash,
    signal: OutcomeSignal,
) -> Transaction {
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
fn decisions_are_remembered_up_to_the_limit() {
    let live = GateLive::with_rng(GateConfig::defaults(), SplitMix64::seeded(1));
    let candidates = aura_gate::route::candidates(
        &aura_model::model_catalog::catalog("local/qwen3:4b", &[]),
        |p| p == aura_auth::ProviderKind::Claude,
        |_| false,
    );
    for _ in 0..RECENT_DECISIONS + 5 {
        live.decide(
            &aura_gate::route::Work::turn(PromptCategory::Debugging),
            &candidates,
        )
        .unwrap();
    }
    let recent = live.recent();
    assert_eq!(recent.len(), RECENT_DECISIONS);
    assert!(recent
        .iter()
        .all(|r| r.category == PromptCategory::Debugging && r.level == RouteLevel::Turn));
    assert!(live
        .decide(
            &aura_gate::route::Work::turn(PromptCategory::Debugging),
            &[]
        )
        .is_none());
    assert_eq!(live.recent().len(), RECENT_DECISIONS);
}
