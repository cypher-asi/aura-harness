//! Times the agent-list work behind creating and archiving agents, on a
//! copy of a real data directory. Ignored by default; run with
//! `AURA_PERF_DATA=<copy of aura_data> cargo test -p aura-app --test agent_list_perf -- --ignored --nocapture`.

use aura_app::agent_registry::AgentRegistry;
use aura_app::record_loader::{agent_summaries, archived_summaries, project_summaries, LastActive};
use aura_core_types::AgentId;
use aura_store_db::RocksStore;
use aura_store_record::ReadStore;
use std::collections::HashSet;
use std::time::{Duration, Instant};

fn time<T>(label: &str, runs: u32, mut f: impl FnMut() -> T) -> T {
    let mut out = f();
    let start = Instant::now();
    for _ in 0..runs {
        out = f();
    }
    let each = start.elapsed() / runs.max(1);
    println!("{label:<40} {:>10.3} ms", each.as_secs_f64() * 1000.0);
    out
}

/// The timed steps; test code, so they may unwrap.
#[cfg(test)]
mod steps {
    use super::*;

    /// Store-level reads and decodes of the agent rows, when there are any.
    pub(super) fn time_agent_rows(store: &RocksStore) {
        let agents = aura_store_db::AgentStore::for_store(store);
        if agents.is_empty().unwrap() {
            return;
        }
        let rows = time("store load (agent rows)", 3, || {
            agents.load().unwrap().agents
        });
        time("derive id of every row", 3, || {
            for row in &rows {
                std::hint::black_box(aura_store_db::agents::derive_agent_id(&row.zns_id));
            }
        });
        let bytes: Vec<Vec<u8>> = rows
            .iter()
            .map(|r| serde_json::to_vec(r).unwrap())
            .collect();
        time("decode every row", 3, || {
            for b in &bytes {
                std::hint::black_box(
                    serde_json::from_slice::<aura_store_db::AgentRecord>(b).unwrap(),
                );
            }
        });
    }

    /// The registry's in-memory reads. Returns the listed agents.
    pub(super) fn time_registry_reads(registry: &AgentRegistry) -> Vec<AgentId> {
        time("tree", 5, || registry.tree().len());
        time("archived_tree", 5, || registry.archived_tree().len());
        time("active", 5, || registry.active().agent_id());
        time("agent_id of every agent", 5, || {
            registry
                .agents()
                .iter()
                .map(|a| a.agent_id())
                .collect::<Vec<_>>()
        });
        let listed: Vec<_> = registry.tree().iter().map(|(a, _)| a.agent_id()).collect();
        time("find of every listed agent", 2, || {
            listed
                .iter()
                .filter(|&&id| registry.find(id).is_some())
                .count()
        });
        time("under_compare of every listed agent", 2, || {
            listed
                .iter()
                .filter(|&&id| registry.under_compare(id))
                .count()
        });
        time("project_of of every listed agent", 2, || {
            listed
                .iter()
                .filter(|&&id| registry.project_of(id).is_some())
                .count()
        });
        listed
    }

    /// The store reads the list rows make for the listed agents.
    pub(super) fn time_store_heads(store: &RocksStore, listed: &[AgentId]) {
        time("store head+last entry, listed agents", 2, || {
            let mut ts = 0u64;
            for &id in listed {
                let head = store.get_head_seq(id).unwrap_or(0);
                if head > 0 {
                    if let Ok(entry) = store.get_record_entry(id, head) {
                        ts = ts.max(entry.tx.ts_ms);
                    }
                }
            }
            ts
        });
        time("store head only, listed agents", 2, || {
            listed
                .iter()
                .map(|&id| store.get_head_seq(id).unwrap_or(0))
                .sum::<u64>()
        });
    }

    /// The summaries the panels show, and the last-activity cache.
    pub(super) fn time_summaries(registry: &AgentRegistry, store: &RocksStore, active: AgentId) {
        let running = HashSet::new();
        time("project_summaries", 3, || project_summaries(registry).len());
        time("agent_summaries", 2, || {
            agent_summaries(registry, store, active, &running).len()
        });
        time("archived_summaries", 2, || {
            archived_summaries(registry, store).len()
        });

        let mut cache = LastActive::default();
        let ids: Vec<_> = registry
            .tree()
            .iter()
            .chain(registry.archived_tree().iter())
            .map(|(a, _)| a.agent_id())
            .collect();
        time("last activity, every row, warm cache", 3, || {
            ids.iter().map(|&id| cache.of(store, id).0).sum::<u64>()
        });
    }

    /// The registry writes: set active, create, archive, and a subtree walk.
    pub(super) fn time_saves(registry: &mut AgentRegistry, active: AgentId, listed: &[AgentId]) {
        let mut save = Duration::ZERO;
        for _ in 0..5 {
            let start = Instant::now();
            registry.set_active(active).unwrap();
            save += start.elapsed();
        }
        println!(
            "{:<40} {:>10.3} ms",
            "set_active (save)",
            save.as_secs_f64() * 1000.0 / 5.0
        );
        let start = Instant::now();
        let created = registry.create(None).unwrap();
        println!(
            "{:<40} {:>10.3} ms",
            "create (save)",
            start.elapsed().as_secs_f64() * 1000.0
        );
        let start = Instant::now();
        registry.archive(created.agent_id()).unwrap();
        println!(
            "{:<40} {:>10.3} ms",
            "archive (save)",
            start.elapsed().as_secs_f64() * 1000.0
        );
        let start = Instant::now();
        let subtree = registry.subtree(listed[0]);
        println!(
            "{:<40} {:>10.3} ms ({} agents)",
            "subtree of first listed",
            start.elapsed().as_secs_f64() * 1000.0,
            subtree.len()
        );
    }

    /// What `Traces::latest` does for an agent with no stored run.
    pub(super) fn time_bundle_fallback() {
        if let Ok(home) = std::env::var("AURA_PERF_HOME") {
            let home = std::path::PathBuf::from(home);
            time("bundle fallback (no match)", 2, || {
                aura_agent_trace::bundle::list_run_dirs(&home)
                    .into_iter()
                    .rev()
                    .filter_map(|dir| aura_agent_trace::RunBundle::at(dir).read_meta().ok())
                    .filter(|meta| aura_agent_trace::db::owner_of(meta).as_deref() == Some("nope"))
                    .count()
            });
        }
    }
}

use steps::{
    time_agent_rows, time_bundle_fallback, time_registry_reads, time_saves, time_store_heads,
    time_summaries,
};

#[test]
#[ignore = "needs AURA_PERF_DATA"]
fn time_agent_list() {
    let Ok(dir) = std::env::var("AURA_PERF_DATA") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let launch = std::env::current_dir().unwrap();
    let started = Instant::now();
    let store = RocksStore::open(dir.join("db"), false).unwrap();
    println!("open store {:?}", started.elapsed());
    time_agent_rows(&store);
    let mut registry = time("load registry", 3, || {
        AgentRegistry::open(aura_store_db::AgentStore::for_store(&store), &dir, &launch).unwrap()
    });
    println!(
        "agents {} listed {} archived rows {}",
        registry.agents().len(),
        registry.tree().len(),
        registry.archived_tree().len()
    );

    let listed = time_registry_reads(&registry);
    time_store_heads(&store, &listed);
    let active = registry.active().agent_id();
    time_summaries(&registry, &store, active);
    time_saves(&mut registry, active, &listed);
    time_bundle_fallback();
}
