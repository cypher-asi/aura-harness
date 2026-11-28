//! Startup with many agents: how long `session::start` takes, until the
//! chat the user is in is sent, until the loop answers the UI's first
//! event (the chat is usable), and until startup work is done.
//!
//! Ignored by default:
//!
//! ```text
//! cargo test --release -p aura-app --test it startup_perf:: -- --ignored --nocapture
//! ```
//!
//! `AURA_PERF_AGENTS` lists the agent counts (default `10,50,200`) and
//! `AURA_PERF_TURNS` the turns each agent's record holds (default `40`).
//! Every agent has a record; the active one is opened. The cloud is not
//! configured here, so its connect is not part of the numbers.
//! `AURA_PERF_FIRST_OPEN=1` times the first start after the agents were
//! written, which settles the database first.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use aura_app::agent_registry::AgentRegistry;
use aura_app::session::{self, SessionOptions};
use aura_core_types::{AgentId, PromptPayload, RecordEntry, Transaction, TransactionType};
use aura_store_db::{RocksStore, WriteStore};
use aura_ui::{UiCommand, UiEvent};
use std::path::Path;
use std::time::{Duration, Instant};

fn env_list(name: &str, default: &[usize]) -> Vec<usize> {
    std::env::var(name).ok().map_or_else(
        || default.to_vec(),
        |v| v.split(',').filter_map(|n| n.trim().parse().ok()).collect(),
    )
}

/// `turns` turns of `agent`: a prompt and a reply of about 2 KB each.
fn write_agent(store: &RocksStore, agent: AgentId, turns: usize) {
    let reply = "The loader is off the hot path now; each stage is measured. ".repeat(32);
    let mut seq = 1;
    for turn in 0..turns {
        let drafts = [
            (
                TransactionType::UserPrompt,
                PromptPayload::new(format!("Pass {turn}: where does the time go?")).to_bytes(),
            ),
            (TransactionType::AgentMsg, reply.clone().into_bytes()),
        ];
        for (tx_type, payload) in drafts {
            let tx = Transaction::new_chained(agent, tx_type, payload, None);
            let entry = RecordEntry::builder(seq, tx).build();
            store
                .append_entry_direct(agent, seq, &entry)
                .expect("append");
            seq += 1;
        }
    }
}

/// A data dir in `dir` holding `agents` agents with records.
fn seed(dir: &Path, agents: usize, turns: usize) {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let db = aura_app::session_helpers::resolve_store_path(dir);
    let store = runtime
        .block_on(aura_app::local_store::open(dir, &db))
        .expect("store");
    let launch = std::env::current_dir().unwrap();
    let rows = aura_store_db::AgentStore::for_store(&store);
    let mut registry = AgentRegistry::open(rows, dir, &launch).expect("registry");
    let mut ids = vec![registry.active().agent_id()];
    while ids.len() < agents {
        ids.push(registry.create(None).expect("agent").agent_id());
    }
    for id in ids {
        write_agent(&store, id, turns);
    }
    drop(registry);
    drop(store);
    if std::env::var_os("AURA_PERF_FIRST_OPEN").is_none() {
        // Open once more, as the run before this one did: the first open
        // after writing settles the database, which a later start no
        // longer pays for. `AURA_PERF_FIRST_OPEN` times that open.
        let again = runtime
            .block_on(aura_app::local_store::open(dir, &db))
            .expect("reopen");
        drop(again);
    }
}

/// The times from calling `session::start`.
#[derive(Debug, Default)]
struct Startup {
    started: Duration,
    chat_sent: Duration,
    usable: Duration,
    loaded: Duration,
}

/// Start a session on `dir` and time it.
async fn time_startup(dir: &Path) -> Startup {
    let options = SessionOptions {
        dir: Some(dir.to_path_buf()),
        provider: "mock".to_string(),
        ..SessionOptions::default()
    };
    let begin = Instant::now();
    let mut session = session::start(&options, None).await.expect("session");
    let mut times = Startup {
        started: begin.elapsed(),
        ..Startup::default()
    };
    session.events.send(UiEvent::RefreshAgents).await.unwrap();
    let (mut chat, mut answered, mut done) = (false, false, false);
    let mut lists = 0;
    while !(chat && answered && done) {
        let command = tokio::time::timeout(Duration::from_secs(120), session.commands.recv())
            .await
            .expect("startup stalled")
            .expect("session ended");
        match command {
            UiCommand::ForAgent { command, .. }
                if matches!(*command, UiCommand::LoadConversation(_)) && !chat =>
            {
                chat = true;
                times.chat_sent = begin.elapsed();
            }
            UiCommand::SetAgents(_) => {
                // The opening's list comes first; the one after it
                // answers the probe.
                lists += 1;
                if lists > 1 && chat && !answered {
                    answered = true;
                    times.usable = begin.elapsed();
                }
            }
            UiCommand::StartupDone => {
                done = true;
                times.loaded = begin.elapsed();
            }
            _ => {}
        }
    }
    drop(session.commands);
    let _ = session.task.finish().await;
    times
}

fn ms(d: Duration) -> String {
    format!("{:>9.1}", d.as_secs_f64() * 1000.0)
}

#[test]
#[ignore = "load test; run with --ignored --nocapture"]
fn startup_with_many_agents() {
    let key = tempfile::NamedTempFile::new().unwrap();
    std::env::set_var("AURA_STATE_KEY_FILE", key.path());
    std::fs::remove_file(key.path()).ok();
    let turns = env_list("AURA_PERF_TURNS", &[40])[0];
    println!("agents  start ms  chat ms  usable ms  loaded ms   ({turns} turns each)");
    for agents in env_list("AURA_PERF_AGENTS", &[10, 50, 200]) {
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path(), agents, turns);
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let times = runtime.block_on(time_startup(dir.path()));
        println!(
            "{agents:>6} {} {} {} {}",
            ms(times.started),
            ms(times.chat_sent),
            ms(times.usable),
            ms(times.loaded)
        );
    }
}
