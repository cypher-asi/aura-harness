//! The load harness: the real event loop and the real terminal UI with N
//! simulated agents, timed.
//!
//! A channel's coordinator has N members, half on this computer and half
//! on cloud machines, and as many archived agents sit in the registry
//! as a long-lived data directory has. While it runs:
//!
//! * each local member is assigned work every [`TURN_EVERY`], which its
//!   mock model answers, so turns start and end, reports go back to the
//!   coordinator as inbox mail, and the coordinator's turns post to the
//!   channel;
//! * each cloud member streams one long turn through the real cloud turn
//!   path: text every 20 ms, a tool step now and then;
//! * the user switches agents every second and scrolls;
//! * a probe goes to the loop every 50 ms, to time how long an event
//!   waits.
//!
//! Run it with
//! `AURA_LOAD_AGENTS=10,50,200 cargo test --release -p aura-app --lib -- load:: --ignored --nocapture`
//! (`AURA_LOAD_SECS` sets the measured span, 10 s by default;
//! `AURA_LOAD_UNPACED=1` builds the lists and saves the inbox after every
//! event, as before the pacing, for comparison). It prints
//! a table per N and the process's resident memory before and after, for
//! the memory side to compare.

use super::*;
use crate::cloud::RemoteEvent;
use crate::event_loop::inbox::{self as owed, assign, AssignArgs};
use aura_store_db::NewChannel;
use measure::{header, row, rss_mib, ByKind, Spread};
use std::fmt::Write as _;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

mod budget;
mod measure;
mod ui;

/// How often each local member is given new work.
const TURN_EVERY: Duration = Duration::from_secs(2);
/// How often a cloud member's stream sends text.
const STREAM_EVERY: Duration = Duration::from_millis(20);
/// How often the user switches agents.
const SWITCH_EVERY: Duration = Duration::from_secs(1);
/// How often a probe goes to the loop.
const PROBE_EVERY: Duration = Duration::from_millis(50);
/// How long the run warms up before it measures.
const WARM_UP: Duration = Duration::from_secs(2);

fn env_list(name: &str, default: &[usize]) -> Vec<usize> {
    std::env::var(name)
        .ok()
        .map(|v| v.split(',').filter_map(|n| n.trim().parse().ok()).collect())
        .filter(|v: &Vec<usize>| !v.is_empty())
        .unwrap_or_else(|| default.to_vec())
}

/// The agents of one run.
struct Cast {
    coordinator: AgentId,
    channel: String,
    local: Vec<(AgentId, String)>,
    cloud: Vec<AgentId>,
}

/// A channel with `n` members, half of them cloud members, and `n`
/// archived agents.
async fn cast(fixture: &Fixture, n: usize) -> Cast {
    let new = NewChannel {
        slug: "load".to_string(),
        title: "Load".to_string(),
        description: "The load harness's channel.".to_string(),
        ..NewChannel::default()
    };
    let channel = channels::create_channel(&fixture.hub, new).await.unwrap();
    let mut cast = Cast {
        coordinator: channel.coordinator,
        channel: channel.id.clone(),
        local: Vec::new(),
        cloud: Vec::new(),
    };
    for at in 0..n {
        let agent = subagents::lock(&fixture.hub.registry)
            .create(Some(format!("Member {at}")))
            .unwrap()
            .agent_id();
        let member = lock_registry_agent(fixture, agent);
        channels::join_parent_channel(&fixture.hub, channel.coordinator, &member).unwrap();
        if at % 2 == 0 {
            cast.local.push((agent, member.code.clone().unwrap()));
        } else {
            cast.cloud.push(agent);
        }
        let old = subagents::lock(&fixture.hub.registry)
            .create(Some(format!("Old {at}")))
            .unwrap()
            .agent_id();
        subagents::lock(&fixture.hub.registry).archive(old).unwrap();
    }
    cast
}

/// One cloud member's turn: text every [`STREAM_EVERY`], a tool step
/// every 25 texts, until `stop`, then its end.
async fn stream(tx: mpsc::Sender<RemoteEvent>, stop: Arc<AtomicBool>) {
    let mut at = 0u64;
    while !stop.load(Ordering::Relaxed) {
        at += 1;
        let text = format!("word{at} and some more of the reply ");
        if tx.send(RemoteEvent::Text(text)).await.is_err() {
            return;
        }
        if at % 25 == 0 {
            let id = format!("t{at}");
            let step = [
                RemoteEvent::ToolStart {
                    id: id.clone(),
                    name: "read_file".into(),
                },
                RemoteEvent::ToolInput {
                    id: id.clone(),
                    input: r#"{"path":"src/main.rs"}"#.into(),
                },
                RemoteEvent::ToolResult {
                    id,
                    result: "fn main() {}\n".repeat(20),
                    is_error: false,
                },
                RemoteEvent::ModelCallEnd,
            ];
            for event in step {
                let _ = tx.send(event).await;
            }
        }
        tokio::time::sleep(STREAM_EVERY).await;
    }
    let _ = tx.send(RemoteEvent::Done(Box::default())).await;
}

/// What drives a run besides its cloud streams.
struct Driver {
    hub: Arc<AgentHub>,
    events: mpsc::Sender<UiEvent>,
    cast: Cast,
    stop: Arc<AtomicBool>,
}

impl Driver {
    /// Assign work, switch agents and probe the loop until `stop`.
    async fn run(self) {
        let started = Instant::now();
        let mut probes = tokio::time::interval(PROBE_EVERY);
        let mut next_switch = Instant::now();
        let mut next_turn = vec![Instant::now(); self.cast.local.len()];
        let spread = TURN_EVERY / u32::try_from(self.cast.local.len().max(1)).unwrap_or(1);
        for (at, next) in next_turn.iter_mut().enumerate() {
            *next += spread * u32::try_from(at).unwrap_or(0);
        }
        let everyone: Vec<AgentId> = (self.cast.local.iter().map(|(a, _)| *a))
            .chain(self.cast.cloud.iter().copied())
            .chain([self.cast.coordinator])
            .collect();
        let mut switch = 0usize;
        while !self.stop.load(Ordering::Relaxed) {
            probes.tick().await;
            self.hub.loop_work.timings.probe_sent();
            let _ = self.events.send(UiEvent::ShowHelp).await;
            if Instant::now() >= next_switch {
                next_switch += SWITCH_EVERY;
                switch += 1;
                let agent = everyone[switch % everyone.len()];
                let _ = self.events.send(UiEvent::SelectAgent(agent.to_hex())).await;
            }
            for (at, (_, code)) in self.cast.local.iter().enumerate() {
                if Instant::now() >= next_turn[at] {
                    next_turn[at] += TURN_EVERY;
                    self.assign(code, started.elapsed()).await;
                }
            }
        }
    }

    async fn assign(&self, code: &str, at: Duration) {
        let args = AssignArgs {
            to: Some(code.to_lowercase()),
            agent: None,
            subject: format!("Task at {}", at.as_millis()),
            prompt: "Read the footer and say what it links to.".to_string(),
            todo_id: None,
            withdraw: false,
        };
        let _ = assign(&self.hub, self.cast.coordinator, args).await;
    }
}

/// What one run measured.
struct Measured {
    n: usize,
    loop_events: ByKind,
    waits: Vec<Duration>,
    ui: ui::UiStats,
    rss: (Option<f64>, Option<f64>),
    span: Duration,
}

/// What the loop listens to, for `fixture`'s hub, `b` taking the UI's
/// events from `events`.
pub(super) fn loop_inbox(
    fixture: &Fixture,
    b: &mut super::fixture::Borrowed,
    events: mpsc::Receiver<UiEvent>,
    quit: &CancellationToken,
) -> Inbox {
    b.events = events;
    let done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let (link_tx, link_rx) = mpsc::channel(4);
    Inbox {
        done_rx: done,
        process_completions: mpsc::channel(1).1,
        notice_rx: fixture.hub.take_notices(),
        mail_rx: fixture.hub.open_mail(),
        link_rx,
        link_tx,
        quit: quit.clone(),
    }
}

/// The clock of a run: warm up, measure for `span`, then stop it all.
/// Returns what the loop kept meanwhile.
async fn clock(
    hub: Arc<AgentHub>,
    (stop, measuring): (Arc<AtomicBool>, Arc<AtomicBool>),
    quit: CancellationToken,
    span: Duration,
) -> crate::event_loop::timing::Kept {
    tokio::time::sleep(WARM_UP).await;
    hub.loop_work.timings.enable();
    hub.loop_work.timings.take();
    measuring.store(true, Ordering::Relaxed);
    tokio::time::sleep(span).await;
    measuring.store(false, Ordering::Relaxed);
    let kept = hub.loop_work.timings.take();
    stop.store(true, Ordering::Relaxed);
    quit.cancel();
    kept
}

/// Pace `hub`'s lists and saves as the app paces them, unless asked to
/// run as before the pacing (`AURA_LOAD_UNPACED`).
fn pace(hub: &AgentHub) {
    if std::env::var_os("AURA_LOAD_UNPACED").is_none() {
        let work = &hub.loop_work;
        work.lists.set_every(crate::event_loop::pacing::LIST_EVERY);
        work.saves.set_every(crate::event_loop::pacing::SAVE_EVERY);
    }
}

/// Start a streaming cloud turn for each of `cloud`, until `stop`.
async fn start_streams(state: &LoopState<'_>, cloud: &[AgentId], stop: &Arc<AtomicBool>) {
    for agent in cloud {
        let (tx, rx) = mpsc::channel(256);
        crate::event_loop::cloud::follow::stream_events(state, *agent, rx).await;
        tokio::spawn(stream(tx, Arc::clone(stop)));
    }
}

fn by_kind(events: Vec<(&'static str, Duration)>) -> ByKind {
    let mut by_kind = ByKind::default();
    for (kind, took) in events {
        by_kind.add(kind, took);
    }
    by_kind
}

/// Run the app with `n` members for `span`, measuring after a warm-up.
async fn run(n: usize, span: Duration) -> Measured {
    let rss_before = rss_mib();
    let (fixture, commands) = fixture();
    let cast = cast(&fixture, n).await;
    let (events_tx, events_rx) = mpsc::channel::<UiEvent>(100);
    let mut b = borrowed(&fixture);
    let quit = CancellationToken::new();
    let mut inbox = loop_inbox(&fixture, &mut b, events_rx, &quit);
    pace(&fixture.hub);
    let mut state = loop_state(&fixture, &mut b);
    owed::restore_saved(&mut state).await;
    let stop = Arc::new(AtomicBool::new(false));
    let measuring = Arc::new(AtomicBool::new(false));
    start_streams(&state, &cast.cloud, &stop).await;
    let ui = ui::spawn(
        commands,
        events_tx.clone(),
        Arc::clone(&stop),
        Arc::clone(&measuring),
    );
    let _ = events_tx
        .send(UiEvent::OpenChannel(cast.channel.clone()))
        .await;
    let hub = Arc::clone(&fixture.hub);
    let driver = Driver {
        hub: Arc::clone(&hub),
        events: events_tx,
        cast,
        stop: Arc::clone(&stop),
    };
    let flags = (Arc::clone(&stop), Arc::clone(&measuring));
    let mut started = startup::Startup::finished();
    let ((), (), kept) = tokio::join!(
        run::run_loop(&mut state, &mut inbox, &mut started),
        driver.run(),
        clock(Arc::clone(&hub), flags, quit, span)
    );
    hub.cancel_all();
    let ui = ui.join().unwrap();
    Measured {
        n,
        loop_events: by_kind(kept.events),
        waits: kept.waits,
        ui,
        rss: (rss_before, rss_mib()),
        span,
    }
}

/// `measured` as a table.
fn report(measured: &Measured) -> String {
    let mut out = String::new();
    let busy: Duration = measured.loop_events.0.values().flatten().sum();
    let _ = writeln!(
        out,
        "\n=== N = {} members over {:?}: event loop busy {:.0}% ===",
        measured.n,
        measured.span,
        100.0 * busy.as_secs_f64() / measured.span.as_secs_f64()
    );
    header(
        &mut out,
        "event loop: how long a probe waited (latency), ms",
    );
    row(&mut out, "probe wait", &Spread::of(&measured.waits));
    header(&mut out, "event loop: time per event, by kind, ms");
    for (kind, spread) in measured.loop_events.spreads() {
        row(&mut out, kind, &spread);
    }
    header(&mut out, "ui: frame and tick, ms");
    row(&mut out, "render", &Spread::of(&measured.ui.frames));
    row(&mut out, "tick", &Spread::of(&measured.ui.ticks));
    header(&mut out, "ui: time per command, by kind (top 12), ms");
    for (kind, spread) in measured.ui.commands.spreads().into_iter().take(12) {
        row(&mut out, kind, &spread);
    }
    let depths = &measured.ui.depths;
    let mut sorted = depths.clone();
    sorted.sort_unstable();
    let p99 = sorted.get(sorted.len() * 99 / 100).copied().unwrap_or(0);
    let _ = writeln!(
        out,
        "ui: commands waiting p99 {p99}, max {}; longest UI stall {:.1} ms",
        sorted.last().copied().unwrap_or(0),
        measured.ui.longest_gap.as_secs_f64() * 1000.0
    );
    let _ = writeln!(
        out,
        "memory: resident {:.0} MiB before, {:.0} MiB after",
        measured.rss.0.unwrap_or(0.0),
        measured.rss.1.unwrap_or(0.0)
    );
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "the load harness: run it with --ignored --nocapture"]
async fn the_app_with_many_agents() {
    let span = Duration::from_secs(
        std::env::var("AURA_LOAD_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(10),
    );
    for n in env_list("AURA_LOAD_AGENTS", &[10, 50, 200]) {
        let measured = run(n, span).await;
        println!("{}", report(&measured));
    }
}
