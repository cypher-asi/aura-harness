//! Turns kernel record entries into the boxes and arrows of the GRAPH tab.
//!
//! A [`FlowBuilder`] follows one agent and the agents it spawned. Each
//! entry of their records becomes a node in its agent's lane, or updates
//! the node it is about: a policy decision and a result belong to their
//! tool call, an outcome to its prompt. Links between lanes come from what
//! the records say about each other: a subtask's prompt names the `task`
//! call that asked for it, a spawn names its child, and the notices agents
//! get about each other's replies name who replied.

use super::steering::SteerSource;
use crate::record_loader::{
    collapse_whitespace, format_duration_ms, format_tokens, json_payload, model_call, payload_text,
    reasoning_error, reasoning_tokens, reasoning_tools, summarize_action_result,
    summarize_policy_decision, summarize_system, summarize_tool_call, tool_execution, tool_outcome,
    tool_target, truncate_chars, tx_type_label,
};
use aura_core_types::{
    ActionResultPayload, AgentId, ComparePayload, Hash, PolicyApproval, PolicyApprover,
    PolicyDecision, RecordEntry, RouteLevel, ToolExecution, ToolGateVerdict, ToolProposal,
    TransactionType,
};
use aura_exec_tools::agents::send_to_agent::SEND_TO_AGENT_TOOL_NAME;
use aura_exec_tools::agents::task::TASK_TOOL_NAME;
use aura_model::ModelCallPurpose;
use aura_ui::events::{FlowBench, FlowKind, FlowLane, FlowNode, FlowUsage, TraceNodeStatus};
use serde_json::Value;
use std::collections::{HashMap, HashSet, VecDeque};

/// Nodes kept per flow; the oldest go first.
const MAX_NODES: usize = 16_000;
/// Longest label of a node. The tab cuts it to the width of its box.
const MAX_LABEL_CHARS: usize = 200;
/// Longest text kept in a node's metadata (prompts, replies, results).
/// The record has the rest.
const MAX_META_CHARS: usize = 1200;
/// Longest fact shown beside a node's kind.
const MAX_DETAIL_CHARS: usize = 48;
/// `task` calls remembered so a subtask can be matched to its call.
const MAX_TASK_CALLS: usize = 64;
/// Characters of the agent's hex id in a node id.
const NODE_ID_AGENT_CHARS: usize = 12;

/// Edge kinds, as the tab names them.
const LINK_SPAWN: &str = "spawn";
const LINK_RETURN: &str = "return";
const LINK_MESSAGE: &str = "message";
const LINK_RESULT: &str = "result";
const LINK_COMPLETED: &str = "completed";

/// How the notice a parent gets about a background `task` goes on after
/// the agent's name (see `subagents::task_notice`).
const TASK_NOTICE_ENDINGS: [&str; 2] = [
    " finished the task you gave it in the background",
    " did not finish the task you gave it in the background",
];

struct Lane {
    agent: AgentId,
    hex: String,
    name: String,
    parent: Option<AgentId>,
    bench: Option<FlowBench>,
    bench_arm: bool,
}

/// A node, and what else finds it.
struct Slot {
    node: FlowNode,
    tx: Hash,
    tool: Option<(AgentId, String)>,
    /// Hex id of the action the kernel accepted for the tool call
    action: Option<String>,
    /// What the tool call acts on (the command line of a `run_command`),
    /// for the process it may start
    target: Option<String>,
}

/// A `task` call and the agents that took its work.
struct TaskCall {
    agent: AgentId,
    node: String,
    prompt: String,
    children: Vec<AgentId>,
    returned: bool,
    /// Whether it runs in the background: its result only says so, and
    /// the agent reports back in a notice
    detached: bool,
}

/// The flow of one agent and the agents below it, built entry by entry.
pub(crate) struct FlowBuilder {
    focus: AgentId,
    lanes: Vec<Lane>,
    lane_of: HashMap<AgentId, usize>,
    /// Lanes added or renamed since the last report
    lane_news: Vec<usize>,
    slots: HashMap<String, Slot>,
    /// Node ids, oldest first
    order: VecDeque<String>,
    by_tx: HashMap<Hash, String>,
    tools: HashMap<(AgentId, String), String>,
    /// Tool calls by the hex id of the action the kernel accepted for
    /// them, which a background process it started reports back with
    actions: HashMap<String, String>,
    last_node: HashMap<AgentId, String>,
    /// Prompts with no reply yet, per agent
    open_prompts: HashMap<AgentId, Vec<String>>,
    tasks: VecDeque<TaskCall>,
    /// The `task` call each child's spawn entry says it answers, until
    /// the child's first prompt takes it
    bound: HashMap<AgentId, String>,
    /// Spawn nodes whose child has written nothing yet
    spawns: HashMap<AgentId, String>,
    /// `send_to_agent` calls whose message the target hasn't recorded yet
    sends: HashMap<AgentId, String>,
    /// Last sequence number taken from each agent's record
    seen: HashMap<AgentId, u64>,
    /// Nodes touched since the last report, in the order they were
    changed: Vec<String>,
    dirty: HashSet<String>,
    /// Agents whose record holds a benchmark
    bench_hosts: HashSet<AgentId>,
    /// The runs of those benchmarks, by the agent that ran each
    bench_runs: HashMap<AgentId, FlowBench>,
}

/// The agent a spawn entry started.
pub(crate) fn spawned_child(entry: &RecordEntry) -> Option<AgentId> {
    if entry.tx.tx_type != TransactionType::SubagentSpawn {
        return None;
    }
    let payload = json_payload(&entry.tx)?;
    let child = payload.get("child_agent_id")?.as_str()?;
    AgentId::from_hex(child).ok()
}

impl FlowBuilder {
    /// A flow for `focus`, shown as `name`, with nothing recorded yet.
    pub(crate) fn new(focus: AgentId, name: &str) -> Self {
        let mut builder = Self {
            focus,
            lanes: Vec::new(),
            lane_of: HashMap::new(),
            lane_news: Vec::new(),
            slots: HashMap::new(),
            order: VecDeque::new(),
            by_tx: HashMap::new(),
            tools: HashMap::new(),
            actions: HashMap::new(),
            last_node: HashMap::new(),
            open_prompts: HashMap::new(),
            tasks: VecDeque::new(),
            bound: HashMap::new(),
            spawns: HashMap::new(),
            sends: HashMap::new(),
            seen: HashMap::new(),
            changed: Vec::new(),
            dirty: HashSet::new(),
            bench_hosts: HashSet::new(),
            bench_runs: HashMap::new(),
        };
        builder.add_lane(focus, name, None);
        builder
    }

    /// The agent the flow is about.
    pub(crate) const fn focus(&self) -> AgentId {
        self.focus
    }

    pub(crate) fn has_lane(&self, agent: AgentId) -> bool {
        self.lane_of.contains_key(&agent)
    }

    pub(crate) fn lane_count(&self) -> usize {
        self.lanes.len()
    }

    /// Give `agent` a lane. Returns `false` when it already has one.
    pub(crate) fn add_lane(&mut self, agent: AgentId, name: &str, parent: Option<AgentId>) -> bool {
        if self.has_lane(agent) {
            return false;
        }
        self.lane_of.insert(agent, self.lanes.len());
        self.lane_news.push(self.lanes.len());
        self.lanes.push(Lane {
            agent,
            hex: agent.to_hex(),
            name: name.to_string(),
            parent,
            bench: None,
            bench_arm: false,
        });
        self.mark_bench(self.lanes.len() - 1);
        true
    }

    /// Say of lane `lane` what it is in the benchmarks known so far.
    fn mark_bench(&mut self, lane: usize) {
        let it = &self.lanes[lane];
        let bench = self.bench_runs.get(&it.agent).cloned();
        let arm = bench.is_none() && it.parent.is_some_and(|p| self.bench_hosts.contains(&p));
        if it.bench == bench && it.bench_arm == arm {
            return;
        }
        let it = &mut self.lanes[lane];
        (it.bench, it.bench_arm) = (bench, arm);
        if !self.lane_news.contains(&lane) {
            self.lane_news.push(lane);
        }
    }

    /// The latest state of the benchmark `host` holds: what each of its
    /// runs worked on and how it did.
    fn bench(&mut self, host: AgentId, payload: &ComparePayload) {
        self.bench_hosts.insert(host);
        for run in &payload.runs {
            let n = run.name.rsplit('-').next().unwrap_or(&run.name);
            let bench = FlowBench {
                group: run.task.clone().unwrap_or_else(|| format!("run {n}")),
                run: run.name.clone(),
                arm: run.arm.index(),
                arm_label: payload.spec(run.arm).label().to_string(),
                check_passed: run.check_passed,
                cost_usd: run.cost_usd,
                wall_ms: run.wall_ms,
                error: run.error.clone(),
            };
            self.bench_runs.insert(run.agent_id, bench);
        }
        for lane in 0..self.lanes.len() {
            self.mark_bench(lane);
        }
    }

    /// Call `agent`'s lane `name` from now on, and the spawn that started
    /// it too while that is still the last thing known about it.
    pub(crate) fn name_lane(&mut self, agent: AgentId, name: &str) {
        let Some(&lane) = self.lane_of.get(&agent) else {
            return;
        };
        if self.lanes[lane].name == name {
            return;
        }
        self.lanes[lane].name = name.to_string();
        if !self.lane_news.contains(&lane) {
            self.lane_news.push(lane);
        }
        if let Some(spawn) = self.spawns.get(&agent).cloned() {
            self.update(&spawn, |node| node.label = spawn_label(name));
        }
    }

    /// Every lane and node, oldest first. Nothing is left to [`Self::drain`].
    pub(crate) fn snapshot(&mut self) -> (Vec<FlowLane>, Vec<FlowNode>) {
        self.changed.clear();
        self.dirty.clear();
        self.lane_news.clear();
        let lanes = self.lanes.iter().map(Lane::to_ui).collect();
        let nodes = self
            .order
            .iter()
            .filter_map(|id| self.slots.get(id))
            .map(|slot| slot.node.clone())
            .collect();
        (lanes, nodes)
    }

    /// Lanes added and nodes added or changed since the last report, or
    /// `None` when there are none.
    pub(crate) fn drain(&mut self) -> Option<(Vec<FlowLane>, Vec<FlowNode>)> {
        if self.changed.is_empty() && self.lane_news.is_empty() {
            return None;
        }
        let lanes = std::mem::take(&mut self.lane_news)
            .into_iter()
            .map(|lane| self.lanes[lane].to_ui())
            .collect();
        self.dirty.clear();
        let nodes = std::mem::take(&mut self.changed)
            .iter()
            .filter_map(|id| self.slots.get(id))
            .map(|slot| slot.node.clone())
            .collect();
        Some((lanes, nodes))
    }

    /// Take `entry` into the flow. Entries of agents without a lane, and
    /// entries already taken, change nothing.
    pub(crate) fn push(&mut self, entry: &RecordEntry) {
        let agent = entry.tx.agent_id;
        if !self.has_lane(agent) {
            return;
        }
        let seen = self.seen.entry(agent).or_default();
        if entry.seq <= *seen {
            return;
        }
        *seen = entry.seq;
        match entry.tx.tx_type {
            TransactionType::UserPrompt => self.prompt(entry),
            TransactionType::AgentMsg => self.reply(entry),
            TransactionType::Reasoning => self.model_call(entry),
            TransactionType::ToolProposal => self.tool_call(entry),
            TransactionType::PolicyDecision => self.policy(entry),
            TransactionType::ToolExecution => self.tool_result(entry),
            TransactionType::SubagentSpawn => self.spawn(entry),
            TransactionType::System => self.system(entry),
            TransactionType::ActionResult | TransactionType::ProcessComplete => self.process(entry),
            TransactionType::Outcome => self.outcome(entry),
            TransactionType::SessionStart => self.event(entry, "new session"),
            TransactionType::Trigger => self.event(entry, &payload_text(&entry.tx)),
        }
    }

    /// Add `node`, made from `entry`. An agent's first node is where the
    /// spawn that started it points.
    fn insert(&mut self, entry: &RecordEntry, node: FlowNode, tool: Option<(AgentId, String)>) {
        let agent = entry.tx.agent_id;
        let id = node.id.clone();
        if let Some(key) = &tool {
            self.tools.insert(key.clone(), id.clone());
        }
        self.by_tx.insert(entry.tx.hash, id.clone());
        self.last_node.insert(agent, id.clone());
        self.slots.insert(
            id.clone(),
            Slot {
                node,
                tx: entry.tx.hash,
                tool,
                action: None,
                target: None,
            },
        );
        self.order.push_back(id.clone());
        self.touch(&id);
        if let Some(spawn) = self.spawns.remove(&agent) {
            self.link(&spawn, LINK_SPAWN, &id);
        }
        self.trim();
    }

    /// Drop the oldest nodes once there are too many.
    fn trim(&mut self) {
        while self.order.len() > MAX_NODES {
            let Some(id) = self.order.pop_front() else {
                break;
            };
            let Some(slot) = self.slots.remove(&id) else {
                continue;
            };
            if self.by_tx.get(&slot.tx) == Some(&id) {
                self.by_tx.remove(&slot.tx);
            }
            if let Some(key) = slot.tool {
                if self.tools.get(&key) == Some(&id) {
                    self.tools.remove(&key);
                }
            }
            if let Some(action) = slot.action {
                if self.actions.get(&action) == Some(&id) {
                    self.actions.remove(&action);
                }
            }
        }
    }

    /// Remember what the tool call `id` acts on.
    fn set_target(&mut self, id: &str, tool: &str, args: &Value) {
        let target = tool_target(tool, args);
        if let Some(slot) = self.slots.get_mut(id).filter(|_| !target.is_empty()) {
            slot.target = Some(short(&target, MAX_LABEL_CHARS));
        }
    }

    fn update(&mut self, id: &str, change: impl FnOnce(&mut FlowNode)) {
        if let Some(slot) = self.slots.get_mut(id) {
            change(&mut slot.node);
            self.touch(id);
        }
    }

    fn link(&mut self, from: &str, kind: &str, to: &str) {
        self.update(from, |node| {
            let link = (kind.to_string(), to.to_string());
            if !node.links.contains(&link) {
                node.links.push(link);
            }
        });
    }

    fn touch(&mut self, id: &str) {
        if self.dirty.insert(id.to_string()) {
            self.changed.push(id.to_string());
        }
    }
}

impl Lane {
    fn to_ui(&self) -> FlowLane {
        FlowLane {
            agent_id: self.hex.clone(),
            name: self.name.clone(),
            parent: self.parent.map(|parent| parent.to_hex()),
            bench: self.bench.clone(),
            bench_arm: self.bench_arm,
        }
    }
}

mod calls;
mod labels;
mod lifecycle;
mod prompt;
#[cfg(test)]
mod tests;

use labels::{
    approval_label, arg, call_label, capped, execution_detail, execution_status, new_node,
    set_call_meta, set_meta, short, spawn_label,
};
