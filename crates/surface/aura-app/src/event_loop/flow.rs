//! Feeds the chat panel's GRAPH tab: the flow between an agent and the
//! agents it spawned, as the kernels record it.
//!
//! [`FlowFeed`] is a record observer on every kernel. Once a UI opens the
//! tab it hands each committed entry to one task, which loads the flow of
//! the agent that was asked for from the store, sends it, and from then on
//! sends what each new entry adds or changes. Kernels never wait on it,
//! and the store is read off the async threads.

use super::flow_builder::{spawned_child, FlowBuilder};
use super::subagents::{lock, AgentHub};
use crate::record_loader::scan_tail;
use aura_agent_kernel::RecordObserver;
use aura_core_types::{AgentId, RecordEntry};
use aura_store_record::ReadStore;
use aura_ui_terminal::UiCommand;
use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use tokio::sync::mpsc;
use tracing::warn;

/// Entries read from the end of each agent's record when a flow loads.
const TAIL_ENTRIES: usize = 1500;
/// Agents in one flow. Of an agent with more below it, the ones that
/// worked last are kept.
const MAX_AGENTS: usize = 48;
/// Flows kept up to date at once: one per window tab showing a graph.
const MAX_WATCHES: usize = 4;
/// Entries taken in before the UI is told what changed.
const BATCH: usize = 256;
/// Levels of spawning looked through to find out whose agent wrote an
/// entry.
const MAX_DEPTH: usize = 16;

enum FlowMsg {
    Open(AgentId),
    Entry(Box<RecordEntry>),
}

/// Record observer behind the GRAPH tab. Costs nothing until a UI opens
/// the tab.
#[derive(Debug)]
pub struct FlowFeed {
    watching: AtomicBool,
    inbox: mpsc::UnboundedSender<FlowMsg>,
}

/// What a [`FlowFeed`] collected, for the task that serves it.
pub struct FlowInbox(mpsc::UnboundedReceiver<FlowMsg>);

impl FlowFeed {
    /// The observer to install on the kernels, and its inbox to
    /// [`FlowInbox::serve`].
    #[must_use]
    pub fn new() -> (Arc<Self>, FlowInbox) {
        let (inbox, entries) = mpsc::unbounded_channel();
        let feed = Arc::new(Self {
            watching: AtomicBool::new(false),
            inbox,
        });
        (feed, FlowInbox(entries))
    }

    /// Send the UI the flow of `agent` and keep it up to date.
    pub(crate) fn open(&self, agent: AgentId) {
        // Entries start queueing before the flow is read from the store,
        // so none falls between the two.
        self.watching.store(true, Ordering::Release);
        let _ = self.inbox.send(FlowMsg::Open(agent));
    }
}

impl RecordObserver for FlowFeed {
    fn on_entry(&self, entry: &RecordEntry) {
        if self.watching.load(Ordering::Acquire) {
            let _ = self.inbox.send(FlowMsg::Entry(Box::new(entry.clone())));
        }
    }
}

impl FlowInbox {
    /// Serve the feed on its own task until the session ends, sending
    /// flows to `commands`. Must be called inside a Tokio runtime.
    pub fn serve(self, hub: &Arc<AgentHub>, commands: mpsc::Sender<UiCommand>) {
        // The hub's kernels hold the feed; holding the hub here would
        // keep both alive after the session.
        tokio::spawn(run(self.0, Arc::downgrade(hub), commands));
    }
}

/// One flow kept up to date.
struct Watch {
    builder: FlowBuilder,
    /// Agents known to be outside the flow
    outsiders: HashSet<AgentId>,
    /// Agents in the flow that nothing could name yet: an agent is
    /// recorded as spawned before the agent host knows it.
    unnamed: HashSet<AgentId>,
}

async fn run(
    mut inbox: mpsc::UnboundedReceiver<FlowMsg>,
    hub: Weak<AgentHub>,
    commands: mpsc::Sender<UiCommand>,
) {
    // Most recently opened last.
    let mut watches: Vec<Watch> = Vec::new();
    while let Some(first) = inbox.recv().await {
        let mut batch = vec![first];
        while batch.len() < BATCH {
            let Ok(next) = inbox.try_recv() else { break };
            batch.push(next);
        }
        let Some(hub) = hub.upgrade() else { break };
        for message in batch {
            match message {
                FlowMsg::Entry(entry) => {
                    for watch in &mut watches {
                        watch.take(&hub, &entry);
                    }
                }
                FlowMsg::Open(focus) => {
                    let Some(mut watch) = open(&hub, &mut watches, focus).await else {
                        continue;
                    };
                    let (lanes, nodes) = watch.builder.snapshot();
                    watches.push(watch);
                    if watches.len() > MAX_WATCHES {
                        watches.remove(0);
                    }
                    let load = UiCommand::FlowLoad {
                        agent_id: focus.to_hex(),
                        lanes,
                        nodes,
                    };
                    if commands.send(load).await.is_err() {
                        return;
                    }
                }
            }
        }
        for watch in &mut watches {
            let Some((lanes, nodes)) = watch.builder.drain() else {
                continue;
            };
            let update = UiCommand::FlowUpsert {
                agent_id: watch.builder.focus().to_hex(),
                lanes,
                nodes,
            };
            if commands.send(update).await.is_err() {
                return;
            }
        }
    }
}

/// The watch of `focus`: the one already kept up to date, else a new one
/// read from the store.
async fn open(hub: &Arc<AgentHub>, watches: &mut Vec<Watch>, focus: AgentId) -> Option<Watch> {
    if let Some(at) = watches.iter().position(|w| w.builder.focus() == focus) {
        return Some(watches.remove(at));
    }
    let loader = Arc::clone(hub);
    match tokio::task::spawn_blocking(move || load(&loader, focus)).await {
        Ok(builder) => Some(Watch {
            builder,
            outsiders: HashSet::new(),
            unnamed: HashSet::new(),
        }),
        Err(e) => {
            warn!(agent = %focus, error = %e, "Loading the agent flow failed");
            None
        }
    }
}

impl Watch {
    /// Take `entry` into the flow if its agent is, or now turns out to
    /// be, part of it.
    fn take(&mut self, hub: &AgentHub, entry: &RecordEntry) {
        let agent = entry.tx.agent_id;
        if !self.builder.has_lane(agent) && !self.admit(hub, agent) {
            return;
        }
        if self.unnamed.contains(&agent) {
            if let Some(name) = known_name(hub, agent) {
                self.builder.name_lane(agent, &name);
                self.unnamed.remove(&agent);
            }
        }
        if let Some(child) = spawned_child(entry) {
            self.add_lane(hub, child, agent);
        }
        self.builder.push(entry);
    }

    fn add_lane(&mut self, hub: &AgentHub, agent: AgentId, parent: AgentId) {
        let name = known_name(hub, agent);
        if name.is_none() {
            self.unnamed.insert(agent);
        }
        let name = name.unwrap_or_else(|| placeholder_name(agent));
        self.builder.add_lane(agent, &name, Some(parent));
        self.outsiders.remove(&agent);
    }

    /// Give `agent` a lane when it works for an agent of the flow.
    fn admit(&mut self, hub: &AgentHub, agent: AgentId) -> bool {
        if self.outsiders.contains(&agent) {
            return false;
        }
        let chain = lineage(hub, agent);
        let Some(known) = chain.iter().position(|a| self.builder.has_lane(*a)) else {
            self.outsiders.insert(agent);
            return false;
        };
        for at in (0..known).rev() {
            self.add_lane(hub, chain[at], chain[at + 1]);
        }
        true
    }
}

/// Read the flow of `focus` from the store: its record and those of the
/// agents below it, in the order things happened. Blocks on the store.
fn load(hub: &AgentHub, focus: AgentId) -> FlowBuilder {
    let store = hub.factory.store.as_ref();
    let mut builder = FlowBuilder::new(focus, &agent_name(hub, focus));
    let family = {
        let registry = lock(&hub.registry);
        registry
            .descendants(focus)
            .into_iter()
            .map(|agent| (agent, registry.ancestry(agent).get(1).copied()))
            .collect::<Vec<_>>()
    };
    let mut queue = VecDeque::from([focus]);
    for (agent, parent) in latest(store, family, MAX_AGENTS - 1) {
        builder.add_lane(agent, &agent_name(hub, agent), parent);
        queue.push_back(agent);
    }
    let mut tails = Vec::new();
    while let Some(agent) = queue.pop_front() {
        let tail = scan_tail(store, agent, TAIL_ENTRIES).unwrap_or_else(|e| {
            warn!(%agent, error = %e, "Could not read a record for the agent flow");
            Vec::new()
        });
        // Agents started with `spawn_agent` are only named by the entry
        // that started them.
        for child in tail.iter().filter_map(spawned_child) {
            if builder.lane_count() < MAX_AGENTS
                && builder.add_lane(child, &agent_name(hub, child), Some(agent))
            {
                queue.push_back(child);
            }
        }
        tails.push(tail);
    }
    for entry in in_time_order(tails) {
        builder.push(&entry);
    }
    builder
}

/// The `keep` agents of `family` that wrote to their record last, in the
/// order given.
fn latest(
    store: &dyn ReadStore,
    family: Vec<(AgentId, Option<AgentId>)>,
    keep: usize,
) -> Vec<(AgentId, Option<AgentId>)> {
    if family.len() <= keep {
        return family;
    }
    let last_active = |agent: AgentId| {
        let head = store.get_head_seq(agent).unwrap_or(0);
        (head > 0)
            .then(|| store.get_record_entry(agent, head).ok())
            .flatten()
            .map_or(0, |entry| entry.tx.ts_ms)
    };
    let mut ranked: Vec<(u64, usize)> = family
        .iter()
        .enumerate()
        .map(|(at, (agent, _))| (last_active(*agent), at))
        .collect();
    ranked.sort_unstable_by(|a, b| b.cmp(a));
    let kept: HashSet<usize> = ranked.into_iter().take(keep).map(|(_, at)| at).collect();
    family
        .into_iter()
        .enumerate()
        .filter(|(at, _)| kept.contains(at))
        .map(|(_, member)| member)
        .collect()
}

/// The entries of several records as one sequence in the order they were
/// written. Each record keeps its own order even where clocks disagree.
fn in_time_order(tails: Vec<Vec<RecordEntry>>) -> Vec<RecordEntry> {
    let mut keyed: Vec<((u64, usize, u64), RecordEntry)> = Vec::new();
    for (record, tail) in tails.into_iter().enumerate() {
        let mut clock = 0;
        for entry in tail {
            clock = entry.tx.ts_ms.max(clock);
            keyed.push(((clock, record, entry.seq), entry));
        }
    }
    keyed.sort_by_key(|(key, _)| *key);
    keyed.into_iter().map(|(_, entry)| entry).collect()
}

/// `agent`, then the agent that spawned it, and so on up.
fn lineage(hub: &AgentHub, agent: AgentId) -> Vec<AgentId> {
    let mut chain = vec![agent];
    // Agents started with `spawn_agent` are known to the agent host...
    if let Some(agents) = &hub.agents {
        while chain.len() < MAX_DEPTH {
            let parent = chain
                .last()
                .and_then(|last| agents.host().info(*last))
                .and_then(|info| info.parent);
            match parent {
                Some(parent) if !chain.contains(&parent) => chain.push(parent),
                _ => break,
            }
        }
    }
    // ...and those above them to the registry.
    if let Some(top) = chain.last().copied() {
        chain.extend(lock(&hub.registry).ancestry(top).into_iter().skip(1));
    }
    chain
}

/// What the UI calls `agent`.
fn agent_name(hub: &AgentHub, agent: AgentId) -> String {
    known_name(hub, agent).unwrap_or_else(|| placeholder_name(agent))
}

/// The name of `agent` in the registry, else at the agent host.
fn known_name(hub: &AgentHub, agent: AgentId) -> Option<String> {
    if let Some(known) = lock(&hub.registry).find(agent) {
        return Some(known.display_name().to_string());
    }
    hub.agents
        .as_ref()
        .and_then(|agents| agents.host().info(agent))
        .map(|info| info.name)
}

fn placeholder_name(agent: AgentId) -> String {
    let hex = agent.to_hex();
    format!("agent {}", hex.get(..8).unwrap_or(&hex))
}

#[cfg(test)]
mod tests {
    use super::*;
    use aura_core_types::{Hash, Transaction, TransactionType};

    fn entry(agent: AgentId, seq: u64, at_ms: u64) -> RecordEntry {
        let hash = Hash::from_content(format!("{agent}:{seq}").as_bytes());
        let tx = Transaction::new(hash, agent, at_ms, TransactionType::AgentMsg, Vec::new());
        RecordEntry::builder(seq, tx).build()
    }

    #[test]
    fn records_merge_by_time_and_keep_their_own_order() {
        let (a, b) = (AgentId::new([1; 32]), AgentId::new([2; 32]));
        let merged = in_time_order(vec![
            // The clock went back between entries 2 and 3.
            vec![entry(a, 1, 100), entry(a, 2, 300), entry(a, 3, 250)],
            vec![entry(b, 1, 200), entry(b, 2, 300), entry(b, 3, 400)],
        ]);
        let order: Vec<(AgentId, u64)> = merged.iter().map(|e| (e.tx.agent_id, e.seq)).collect();
        assert_eq!(order, [(a, 1), (b, 1), (a, 2), (a, 3), (b, 2), (b, 3)]);
    }

    #[tokio::test]
    async fn the_feed_forwards_nothing_until_a_flow_is_opened() {
        let (feed, FlowInbox(mut inbox)) = FlowFeed::new();
        let agent = AgentId::new([1; 32]);
        feed.on_entry(&entry(agent, 1, 100));
        assert!(inbox.try_recv().is_err());

        feed.open(agent);
        feed.on_entry(&entry(agent, 2, 200));
        assert!(matches!(inbox.try_recv(), Ok(FlowMsg::Open(opened)) if opened == agent));
        assert!(matches!(inbox.try_recv(), Ok(FlowMsg::Entry(e)) if e.seq == 2));
    }
}
