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
use aura_model::{model_catalog, ModelCallPurpose};
use aura_ui_terminal::events::{
    FlowBench, FlowKind, FlowLane, FlowNode, FlowUsage, TraceNodeStatus,
};
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

    fn prompt(&mut self, entry: &RecordEntry) {
        let agent = entry.tx.agent_id;
        let payload = entry.tx.prompt_payload();
        let text = payload
            .as_ref()
            .map_or_else(|| payload_text(&entry.tx), |prompt| prompt.text.clone());
        let mut node = new_node(entry, FlowKind::Prompt, &text);
        let mut subtask = false;
        if let Some(prompt) = &payload {
            subtask = prompt.route.level == RouteLevel::Subtask;
            node.detail = short(&prompt.route.model, MAX_DETAIL_CHARS);
            if let Some(category) = prompt.category {
                set_meta(&mut node, "category", category.to_string());
            }
            if !prompt.route.model.is_empty() {
                set_meta(
                    &mut node,
                    "route",
                    format!(
                        "{} · {} · {}",
                        prompt.route.level, prompt.route.model, prompt.route.reason
                    ),
                );
            }
            if prompt.route.explored {
                set_meta(
                    &mut node,
                    "explored",
                    format!("propensity {:.3}", prompt.route.propensity),
                );
            }
            if !prompt.classifier.is_empty() {
                set_meta(&mut node, "classifier", prompt.classifier.clone());
            }
            if let Some(kind) = &prompt.kind {
                set_meta(&mut node, "kind", kind.clone());
            }
        }
        set_meta(&mut node, "text", capped(&text));
        let id = node.id.clone();
        let source = self.prompt_source(agent, subtask, &text);
        self.insert(entry, node, None);
        self.open_prompts.entry(agent).or_default().push(id.clone());
        if let Some((from, kind)) = source {
            self.link(&from, kind, &id);
        }
    }

    /// The node the prompt `text` for `agent` came from, and how: the
    /// `task` call that gave it the work, the last thing the agent whose
    /// reply or message it carries did, or the `send_to_agent` call that
    /// sent it.
    fn prompt_source(
        &mut self,
        agent: AgentId,
        subtask: bool,
        text: &str,
    ) -> Option<(String, &'static str)> {
        if let Some(call) = self.bound.remove(&agent) {
            return Some((call, LINK_SPAWN));
        }
        if subtask {
            let parent = self.lanes[*self.lane_of.get(&agent)?].parent;
            if let Some(call) = self.claim_task(parent, agent, text) {
                return Some((call, LINK_SPAWN));
            }
        }
        if let Some((sender, kind)) = self.sender_of(agent, text) {
            return self.last_node.get(&sender).map(|id| (id.clone(), kind));
        }
        self.sends.remove(&agent).map(|id| (id, LINK_MESSAGE))
    }

    /// The `task` call of `parent` that `child`'s prompt `text` belongs
    /// to: the latest one asking for exactly that, else the latest still
    /// waiting (a retry rewrites the prompt).
    fn claim_task(
        &mut self,
        parent: Option<AgentId>,
        child: AgentId,
        text: &str,
    ) -> Option<String> {
        let parent = parent?;
        let wanted = text.trim();
        let calls = || {
            self.tasks
                .iter()
                .enumerate()
                .rev()
                .filter(|(_, call)| call.agent == parent)
        };
        let index = calls()
            .find(|(_, call)| call.prompt.trim() == wanted)
            .or_else(|| calls().find(|(_, call)| !call.returned))
            .map(|(index, _)| index)?;
        let call = &mut self.tasks[index];
        call.children.push(child);
        Some(call.node.clone())
    }

    /// The agent whose reply or message the prompt `text` for `to`
    /// carries, going by how the harness and the agent host word them.
    fn sender_of(&self, to: AgentId, text: &str) -> Option<(AgentId, &'static str)> {
        let text = text
            .strip_prefix(SteerSource::Harness.prefix())
            .unwrap_or(text);
        if let Some(rest) = text.strip_prefix("Agent ") {
            if let Some((who, _)) = rest.split_once(") reported back:") {
                return self.lane_shown_as(who).map(|agent| (agent, LINK_RETURN));
            }
        }
        if let Some(rest) = text.strip_prefix("Message from agent ") {
            if let Some((who, _)) = rest.split_once("):") {
                return self.lane_shown_as(who).map(|agent| (agent, LINK_MESSAGE));
            }
        }
        self.lanes
            .iter()
            .rev()
            .filter(|lane| lane.agent != to)
            .find(|lane| {
                text.strip_prefix(lane.name.as_str())
                    .is_some_and(|rest| TASK_NOTICE_ENDINGS.iter().any(|end| rest.starts_with(end)))
            })
            .map(|lane| (lane.agent, LINK_RETURN))
    }

    /// The lane of the agent written as `name (id`, by id, else by name.
    fn lane_shown_as(&self, who: &str) -> Option<AgentId> {
        let (name, id) = who.rsplit_once(" (").unwrap_or((who, ""));
        self.lane_with_id(id).or_else(|| {
            self.lanes
                .iter()
                .rev()
                .find(|lane| lane.name == name)
                .map(|lane| lane.agent)
        })
    }

    /// The lane whose agent's hex id starts with `id`.
    fn lane_with_id(&self, id: &str) -> Option<AgentId> {
        let id = id.trim();
        if id.is_empty() {
            return None;
        }
        self.lanes
            .iter()
            .find(|lane| lane.hex.starts_with(id))
            .map(|lane| lane.agent)
    }

    fn reply(&mut self, entry: &RecordEntry) {
        let text = payload_text(&entry.tx);
        let mut node = new_node(entry, FlowKind::Reply, &text);
        node.status = TraceNodeStatus::Ok;
        set_meta(&mut node, "text", capped(&text));
        self.insert(entry, node, None);
        for id in self
            .open_prompts
            .remove(&entry.tx.agent_id)
            .unwrap_or_default()
        {
            self.update(&id, |prompt| {
                if prompt.status == TraceNodeStatus::Pending {
                    prompt.status = TraceNodeStatus::Ok;
                }
            });
        }
    }

    fn model_call(&mut self, entry: &RecordEntry) {
        let payload = json_payload(&entry.tx).unwrap_or(Value::Null);
        let text = |key: &str| payload.get(key).and_then(Value::as_str);
        let purpose = payload
            .get("purpose")
            .cloned()
            .and_then(|purpose| serde_json::from_value(purpose).ok());
        let kind = match purpose {
            Some(ModelCallPurpose::Classify) => FlowKind::Gate,
            Some(ModelCallPurpose::Judge) => FlowKind::Judge,
            Some(ModelCallPurpose::Title) => FlowKind::Title,
            None => FlowKind::Model,
        };
        let mut node = new_node(entry, kind, text("model").unwrap_or("model call"));
        node.status = TraceNodeStatus::Ok;
        let duration_ms = payload.get("duration_ms").and_then(Value::as_u64);
        let mut tokens = reasoning_tokens(&payload);
        if let Some((model, usage)) = model_call(&entry.tx) {
            let billed = model_catalog::billed_tokens(&model, &usage);
            let flow_usage = FlowUsage {
                input_tokens: billed.uncached_input + billed.cache_write + billed.cache_read,
                output_tokens: billed.output,
                cost_usd: model_catalog::estimate_cost_usd(&model, &usage),
                duration_ms,
            };
            // The whole prompt, as the stats line counts it: Anthropic
            // reports cached tokens beside `input_tokens`, not in it.
            tokens = Some(format!(
                "{}/{} tok",
                format_tokens(flow_usage.input_tokens),
                format_tokens(flow_usage.output_tokens)
            ));
            if billed.cache_read > 0 || billed.cache_write > 0 {
                set_meta(
                    &mut node,
                    "cache",
                    format!(
                        "{} read · {} written",
                        format_tokens(billed.cache_read),
                        format_tokens(billed.cache_write)
                    ),
                );
            }
            if let Some(cost) = flow_usage.cost_usd {
                set_meta(&mut node, "cost", format!("${cost:.4}"));
            }
            node.usage = Some(flow_usage);
        }
        let mut facts = Vec::new();
        if let Some(tokens) = tokens {
            set_meta(&mut node, "tokens", tokens.clone());
            facts.push(tokens);
        }
        if let Some(ms) = duration_ms {
            set_meta(&mut node, "took", format_duration_ms(ms));
            facts.push(format_duration_ms(ms));
        }
        if let Some(stop) = text("stop_reason") {
            set_meta(&mut node, "stop", stop.to_string());
        }
        if let Some(tools) = reasoning_tools(&payload) {
            set_meta(&mut node, "calls", tools);
        }
        if let Some(thinking) = text("thinking").filter(|thinking| !thinking.is_empty()) {
            set_meta(&mut node, "thinking", capped(thinking));
            if let Some(ms) = payload.get("thinking_ms").and_then(Value::as_u64) {
                set_meta(&mut node, "thought for", format_duration_ms(ms));
            }
        }
        if let Some(error) = reasoning_error(&payload) {
            node.status = TraceNodeStatus::Error;
            facts = vec!["failed".to_string()];
            set_meta(&mut node, "error", capped(error));
            if let Some(stage) = text("stage") {
                set_meta(&mut node, "stage", stage.to_string());
            }
        }
        node.detail = facts.join(" · ");
        self.insert(entry, node, None);
    }

    fn tool_call(&mut self, entry: &RecordEntry) {
        let Ok(proposal) = serde_json::from_slice::<ToolProposal>(&entry.tx.payload) else {
            return self.event(entry, &payload_text(&entry.tx));
        };
        let agent = entry.tx.agent_id;
        let mut node = new_node(
            entry,
            FlowKind::Tool,
            &call_label(&proposal.tool, &proposal.args),
        );
        node.status = TraceNodeStatus::Pending;
        set_call_meta(&mut node, &proposal.tool, &proposal.args);
        if let Some(source) = &proposal.source {
            set_meta(&mut node, "source", source.clone());
        }
        let id = node.id.clone();
        self.insert(entry, node, Some((agent, proposal.tool_use_id)));
        self.set_target(&id, &proposal.tool, &proposal.args);
        match proposal.tool.as_str() {
            TASK_TOOL_NAME => {
                self.tasks.push_back(TaskCall {
                    agent,
                    node: id,
                    prompt: arg(&proposal.args, "prompt").to_string(),
                    children: Vec::new(),
                    returned: false,
                    detached: arg(&proposal.args, "spawn_mode") == "detached",
                });
                if self.tasks.len() > MAX_TASK_CALLS {
                    self.tasks.pop_front();
                }
            }
            SEND_TO_AGENT_TOOL_NAME => {
                if let Some(target) = self.lane_with_id(arg(&proposal.args, "agent_id")) {
                    self.sends.insert(target, id);
                }
            }
            _ => {}
        }
    }

    fn policy(&mut self, entry: &RecordEntry) {
        let Ok(decision) = serde_json::from_slice::<PolicyDecision>(&entry.tx.payload) else {
            return;
        };
        let key = (entry.tx.agent_id, decision.tool_use_id.clone());
        let Some(id) = self.tools.get(&key).cloned() else {
            return;
        };
        self.update(&id, |node| {
            match decision.verdict {
                ToolGateVerdict::Approved => {}
                ToolGateVerdict::Denied => {
                    node.status = TraceNodeStatus::Error;
                    node.detail = "denied".to_string();
                }
                ToolGateVerdict::PendingApproval => {
                    node.status = TraceNodeStatus::Warn;
                    node.detail = "awaiting approval".to_string();
                }
            }
            set_meta(node, "policy", summarize_policy_decision(&decision));
            if let Some(reason) = decision.reason.as_deref().filter(|r| !r.is_empty()) {
                set_meta(node, "policy reason", capped(reason));
            }
            if let Some(approval) = &decision.approval {
                set_meta(node, "approved by", approval_label(approval));
            }
            set_meta(node, "mode", decision.mode.clone());
        });
        if let Some(action) = decision.action_id {
            if let Some(slot) = self.slots.get_mut(&id) {
                slot.action = Some(action.clone());
            }
            self.actions.insert(action, id);
        }
    }

    fn tool_result(&mut self, entry: &RecordEntry) {
        let Some(execution) = tool_execution(entry) else {
            return;
        };
        let agent = entry.tx.agent_id;
        let key = (agent, execution.tool_use_id.clone());
        let status = execution_status(&execution);
        let finish = |node: &mut FlowNode| {
            node.status = status;
            node.detail = execution_detail(&execution, status);
            set_meta(node, "outcome", capped(&tool_outcome(&execution)));
            if let Some(result) = execution.result.as_deref().filter(|r| !r.is_empty()) {
                set_meta(node, "result", capped(result));
            }
        };
        let call = self
            .tools
            .get(&key)
            .filter(|id| self.slots.contains_key(*id))
            .cloned();
        let Some(call) = call else {
            // The call itself is older than what was loaded.
            let mut node = new_node(
                entry,
                FlowKind::Tool,
                &call_label(&execution.tool, &execution.args),
            );
            set_call_meta(&mut node, &execution.tool, &execution.args);
            finish(&mut node);
            let id = node.id.clone();
            self.insert(entry, node, Some(key));
            return self.set_target(&id, &execution.tool, &execution.args);
        };
        self.update(&call, finish);
        if execution.tool == TASK_TOOL_NAME {
            self.task_returned(entry, &call, &execution, status);
        }
    }

    /// The `task` call `call` came back: a result node in the caller's
    /// lane, which the agents that did the work point to. A call no agent
    /// took yet (one running in the background) has nothing to show.
    fn task_returned(
        &mut self,
        entry: &RecordEntry,
        call: &str,
        execution: &ToolExecution,
        status: TraceNodeStatus,
    ) {
        let Some(task) = self.tasks.iter_mut().find(|task| task.node == call) else {
            return;
        };
        task.returned = true;
        let children = task.children.clone();
        if children.is_empty() || task.detached {
            return;
        }
        let names: Vec<&str> = children
            .iter()
            .filter_map(|child| self.lane_of.get(child))
            .map(|lane| self.lanes[*lane].name.as_str())
            .collect();
        let verb = if status == TraceNodeStatus::Error {
            "failed"
        } else {
            "replied"
        };
        let mut node = new_node(
            entry,
            FlowKind::Result,
            &format!("{} {verb}", names.join(", ")),
        );
        node.status = status;
        node.detail = execution_detail(execution, status);
        if let Some(result) = execution.result.as_deref().filter(|r| !r.is_empty()) {
            set_meta(&mut node, "result", capped(result));
        }
        let id = node.id.clone();
        self.insert(entry, node, None);
        self.link(call, LINK_RESULT, &id);
        for child in children {
            if let Some(last) = self.last_node.get(&child).cloned() {
                self.link(&last, LINK_RETURN, &id);
            }
        }
    }

    fn spawn(&mut self, entry: &RecordEntry) {
        let payload = json_payload(&entry.tx).unwrap_or(Value::Null);
        let child = spawned_child(entry);
        if let Some(child) = child {
            if self.bind_task(entry.tx.agent_id, child, &payload) {
                return;
            }
        }
        let name = child
            .and_then(|child| self.lane_of.get(&child))
            .map(|lane| self.lanes[*lane].name.clone())
            .or_else(|| child.map(|child| format!("agent {child}")))
            .unwrap_or_else(|| "agent".to_string());
        let mut node = new_node(entry, FlowKind::Spawn, &spawn_label(&name));
        node.status = TraceNodeStatus::Ok;
        if let Some(mode) = payload.get("spawn_mode").and_then(Value::as_str) {
            node.detail = mode.to_string();
        }
        if let Some(child) = child {
            set_meta(&mut node, "child", child.to_hex());
        }
        let id = node.id.clone();
        self.insert(entry, node, None);
        if let Some(child) = child {
            self.spawns.insert(child, id);
        }
    }

    /// Tie `child` to the `task` call of `parent` its spawn entry
    /// `payload` names by tool-use id: the call's row is what the child's
    /// work wires to, so the spawn needs no box of its own. Returns
    /// whether the call was found.
    fn bind_task(&mut self, parent: AgentId, child: AgentId, payload: &Value) -> bool {
        let tool_use = arg(payload, "tool_use_id");
        if tool_use.is_empty() {
            return false;
        }
        let Some(call) = self.tools.get(&(parent, tool_use.to_string())).cloned() else {
            return false;
        };
        let Some(task) = self.tasks.iter_mut().find(|task| task.node == call) else {
            return false;
        };
        if !task.children.contains(&child) {
            task.children.push(child);
        }
        let agent = arg(payload, "child_name");
        let todo = arg(payload, "todo_id");
        self.update(&call, |node| {
            if !agent.is_empty() {
                set_meta(node, "agent", agent.to_string());
            }
            if !todo.is_empty() {
                set_meta(node, "todo", todo.to_string());
            }
            set_meta(node, "child", child.to_hex());
        });
        if !self.last_node.contains_key(&child) {
            self.bound.insert(child, call);
        }
        true
    }

    fn system(&mut self, entry: &RecordEntry) {
        if let Some(payload) = entry.tx.compare_payload() {
            self.bench(entry.tx.agent_id, &payload);
        }
        let Some(payload) = json_payload(&entry.tx) else {
            return self.event(entry, &payload_text(&entry.tx));
        };
        if payload.get("kind").and_then(Value::as_str) != Some("agent_message") {
            return self.event(entry, &summarize_system(&payload));
        }
        // A message the agent could not take live (see the agent host).
        let from = arg(&payload, "from_name");
        let text = arg(&payload, "text");
        let mut node = new_node(entry, FlowKind::Message, &format!("{from}: {text}"));
        node.status = TraceNodeStatus::Ok;
        set_meta(&mut node, "from", from.to_string());
        set_meta(&mut node, "text", capped(text));
        let id = node.id.clone();
        self.insert(entry, node, None);
        let reply = payload.get("reply").and_then(Value::as_bool) == Some(true);
        let sender = self
            .lane_with_id(arg(&payload, "from_agent_id"))
            .or_else(|| self.lane_shown_as(from));
        if let Some(last) = sender.and_then(|sender| self.last_node.get(&sender).cloned()) {
            self.link(&last, if reply { LINK_RETURN } else { LINK_MESSAGE }, &id);
        }
    }

    fn process(&mut self, entry: &RecordEntry) {
        let result = serde_json::from_slice::<ActionResultPayload>(&entry.tx.payload).ok();
        let label = result
            .as_ref()
            .map_or_else(|| payload_text(&entry.tx), summarize_action_result);
        // The action id names the call that started the process; the
        // reference is only the agent's latest entry when it started,
        // which in a batch of calls can be another call's.
        let started_by = result
            .as_ref()
            .and_then(|result| self.actions.get(&result.action_id.to_hex()))
            .or_else(|| {
                let hash = entry.tx.reference_tx_hash?;
                self.by_tx.get(&hash)
            })
            .cloned();
        let command = started_by
            .as_ref()
            .and_then(|id| self.slots.get(id))
            .and_then(|slot| slot.target.clone());
        let label = match &command {
            Some(command) => format!("{command} · {label}"),
            None => label,
        };
        let mut node = new_node(entry, FlowKind::Process, &label);
        node.status = match &result {
            Some(result) if !result.success => TraceNodeStatus::Error,
            _ => TraceNodeStatus::Ok,
        };
        if let Some(command) = command {
            set_meta(&mut node, "command", capped(&command));
        }
        if let Some(result) = &result {
            node.detail = format_duration_ms(result.duration_ms);
            set_meta(
                &mut node,
                "exit",
                result
                    .exit_code
                    .map_or_else(|| "none".to_string(), |code| code.to_string()),
            );
            set_meta(&mut node, "process", result.process_id.to_hex());
            for (key, bytes) in [("stdout", &result.stdout), ("stderr", &result.stderr)] {
                let text = String::from_utf8_lossy(bytes);
                if !text.trim().is_empty() {
                    set_meta(&mut node, key, capped(&text));
                }
            }
        }
        let id = node.id.clone();
        self.insert(entry, node, None);
        if let Some(from) = started_by {
            self.link(&from, LINK_COMPLETED, &id);
        }
    }

    /// An outcome says how its prompt's turn ended; it has no box.
    fn outcome(&mut self, entry: &RecordEntry) {
        let Some(outcome) = entry.tx.outcome_payload() else {
            return;
        };
        let Some(id) = self.by_tx.get(&outcome.prompt_tx).cloned() else {
            return;
        };
        if let Some(open) = self.open_prompts.get_mut(&entry.tx.agent_id) {
            open.retain(|prompt| *prompt != id);
        }
        self.update(&id, |node| {
            node.status = match outcome.success {
                Some(false) => TraceNodeStatus::Error,
                Some(true) => TraceNodeStatus::Ok,
                None if node.status == TraceNodeStatus::Pending => TraceNodeStatus::Ok,
                None => node.status,
            };
            if !outcome.model.is_empty() {
                node.detail = short(
                    &format!(
                        "{} · {}",
                        outcome.model,
                        format_duration_ms(outcome.latency_ms)
                    ),
                    MAX_DETAIL_CHARS,
                );
            }
            // Later signals (a rewind, the judge, the user's rating) say
            // more about the same work; the first one is not dropped.
            let signal = outcome.signal.to_string();
            let signals = match node.meta.iter().find(|(key, _)| key == "outcome") {
                Some((_, before)) if before.split(" → ").any(|s| s == signal) => before.clone(),
                Some((_, before)) => format!("{before} → {signal}"),
                None => signal,
            };
            set_meta(node, "outcome", signals);
            set_meta(node, "cost", format!("${:.4}", outcome.cost_usd));
            set_meta(node, "took", format_duration_ms(outcome.latency_ms));
        });
    }

    fn event(&mut self, entry: &RecordEntry, label: &str) {
        let mut node = new_node(entry, FlowKind::Event, label);
        node.status = TraceNodeStatus::Ok;
        self.insert(entry, node, None);
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

/// A node for `entry` labelled `label`, cut to one line.
fn new_node(entry: &RecordEntry, kind: FlowKind, label: &str) -> FlowNode {
    let agent = entry.tx.agent_id.to_hex();
    let label = short(label, MAX_LABEL_CHARS);
    FlowNode {
        id: format!(
            "{}:{}",
            &agent[..NODE_ID_AGENT_CHARS.min(agent.len())],
            entry.seq
        ),
        agent_id: agent,
        kind,
        label: if label.is_empty() {
            tx_type_label(entry.tx.tx_type).0.to_lowercase()
        } else {
            label
        },
        seq: entry.seq,
        at_ms: entry.tx.ts_ms,
        meta: vec![("tx".to_string(), entry.tx.hash.to_hex())],
        ..FlowNode::default()
    }
}

fn set_meta(node: &mut FlowNode, key: &str, value: String) {
    match node.meta.iter_mut().find(|(k, _)| k == key) {
        Some(slot) => slot.1 = value,
        None => node.meta.push((key.to_string(), value)),
    }
}

fn set_call_meta(node: &mut FlowNode, tool: &str, args: &Value) {
    set_meta(node, "tool", tool.to_string());
    if !args.is_null() {
        let args = serde_json::to_string_pretty(args).unwrap_or_else(|_| args.to_string());
        set_meta(node, "args", capped(&args));
    }
}

/// Who answered a live approval, and for how long it holds: `user · session`.
fn approval_label(approval: &PolicyApproval) -> String {
    let who = match &approval.approver {
        PolicyApprover::Hook => "hook".to_string(),
        PolicyApprover::User { user_id: Some(id) } => format!("user {id}"),
        PolicyApprover::User { user_id: None } => "user".to_string(),
    };
    match &approval.remember {
        Some(remember) => format!("{who} · {remember}"),
        None => who,
    }
}

fn spawn_label(name: &str) -> String {
    short(&format!("spawn {name}"), MAX_LABEL_CHARS)
}

/// What a tool call does, on one line. A `task` names the agent it asks
/// and what for.
fn call_label(tool: &str, args: &Value) -> String {
    if tool != TASK_TOOL_NAME {
        return summarize_tool_call(tool, args);
    }
    let kind = arg(args, "subagent_type");
    let prompt = arg(args, "prompt");
    match (kind.is_empty(), prompt.is_empty()) {
        (true, true) => tool.to_string(),
        (false, true) => format!("{tool} {kind}"),
        (true, false) => format!("{tool} · {prompt}"),
        (false, false) => format!("{tool} {kind} · {prompt}"),
    }
}

const fn execution_status(execution: &ToolExecution) -> TraceNodeStatus {
    match execution.decision {
        ToolGateVerdict::Approved if !execution.is_error => TraceNodeStatus::Ok,
        ToolGateVerdict::Approved | ToolGateVerdict::Denied => TraceNodeStatus::Error,
        ToolGateVerdict::PendingApproval => TraceNodeStatus::Warn,
    }
}

/// How long a call that worked took, else how it ended.
fn execution_detail(execution: &ToolExecution, status: TraceNodeStatus) -> String {
    match execution.duration_ms {
        Some(ms) if status == TraceNodeStatus::Ok => format_duration_ms(ms),
        _ => short(&tool_outcome(execution), MAX_DETAIL_CHARS),
    }
}

/// The string argument `key` of a call or payload; empty when it has none.
fn arg<'a>(args: &'a Value, key: &str) -> &'a str {
    args.get(key).and_then(Value::as_str).unwrap_or("")
}

/// `text` on one line of at most `max` characters.
fn short(text: &str, max: usize) -> String {
    truncate_chars(&collapse_whitespace(text), max)
}

/// `text` as kept in a node's metadata.
fn capped(text: &str) -> String {
    truncate_chars(text.trim(), MAX_META_CHARS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aura_core_types::{
        ActionId, OutcomePayload, OutcomeSignal, PolicyRule, PolicySource, ProcessId,
        PromptCategory, PromptPayload, Route, RouteReason, Transaction,
    };
    use serde_json::json;

    /// Writes records the way kernels do, one sequence per agent.
    #[derive(Default)]
    struct Records {
        seqs: HashMap<AgentId, u64>,
        clock: u64,
    }

    impl Records {
        fn entry(
            &mut self,
            agent: AgentId,
            tx_type: TransactionType,
            payload: Vec<u8>,
        ) -> RecordEntry {
            let seq = self.seqs.entry(agent).or_default();
            *seq += 1;
            self.clock += 10;
            let hash = Hash::from_content(format!("{agent}:{seq}").as_bytes());
            let tx = Transaction::new(hash, agent, self.clock, tx_type, payload);
            RecordEntry::builder(*seq, tx).build()
        }

        fn json(
            &mut self,
            agent: AgentId,
            tx_type: TransactionType,
            payload: &Value,
        ) -> RecordEntry {
            self.entry(agent, tx_type, payload.to_string().into_bytes())
        }

        fn prompt(&mut self, agent: AgentId, text: &str) -> RecordEntry {
            self.entry(
                agent,
                TransactionType::UserPrompt,
                PromptPayload::new(text).to_bytes(),
            )
        }

        fn subtask(&mut self, agent: AgentId, kind: &str, text: &str) -> RecordEntry {
            let payload = PromptPayload {
                route: Route::subtask("haiku", RouteReason::Pinned),
                kind: Some(kind.to_string()),
                ..PromptPayload::new(text)
            };
            self.entry(agent, TransactionType::UserPrompt, payload.to_bytes())
        }

        fn reply(&mut self, agent: AgentId, text: &str) -> RecordEntry {
            self.entry(agent, TransactionType::AgentMsg, text.as_bytes().to_vec())
        }

        fn call(&mut self, agent: AgentId, id: &str, tool: &str, args: Value) -> RecordEntry {
            let proposal = ToolProposal::new(id, tool, args);
            self.json(agent, TransactionType::ToolProposal, &json!(proposal))
        }

        fn result(
            &mut self,
            agent: AgentId,
            id: &str,
            tool: &str,
            result: &str,
            is_error: bool,
        ) -> RecordEntry {
            let execution = ToolExecution {
                tool_use_id: id.to_string(),
                tool: tool.to_string(),
                args: json!({}),
                decision: ToolGateVerdict::Approved,
                reason: None,
                result: Some(result.to_string()),
                is_error,
                duration_ms: Some(4200),
                parent_agent_id: agent,
                originating_user_id: String::new(),
            };
            self.json(agent, TransactionType::ToolExecution, &json!(execution))
        }
    }

    fn agent(byte: u8) -> AgentId {
        AgentId::new([byte; 32])
    }

    /// Node id of entry `seq` of `agent`'s record.
    fn id(agent: AgentId, seq: u64) -> String {
        format!("{}:{seq}", &agent.to_hex()[..NODE_ID_AGENT_CHARS])
    }

    fn node<'a>(nodes: &'a [FlowNode], id: &str) -> &'a FlowNode {
        nodes
            .iter()
            .find(|node| node.id == id)
            .unwrap_or_else(|| panic!("no node {id}"))
    }

    fn link(kind: &str, to: String) -> (String, String) {
        (kind.to_string(), to)
    }

    fn meta<'a>(node: &'a FlowNode, key: &str) -> Option<&'a str> {
        node.meta
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, value)| value.as_str())
    }

    #[test]
    fn a_task_links_the_call_to_the_subtask_and_the_reply_to_the_result() {
        let (lead, coder) = (agent(1), agent(2));
        let mut records = Records::default();
        let mut flow = FlowBuilder::new(lead, "Lead");
        flow.add_lane(coder, "coder #1", Some(lead));

        flow.push(&records.prompt(lead, "add a greeting"));
        flow.push(&records.json(
            lead,
            TransactionType::Reasoning,
            &json!({"model": "sonnet", "input_tokens": 1200, "output_tokens": 300, "duration_ms": 900}),
        ));
        flow.push(&records.call(
            lead,
            "tu_1",
            "task",
            json!({"subagent_type": "coder", "prompt": "write greet()"}),
        ));
        flow.push(&records.subtask(coder, "coder", "write greet()"));
        flow.push(&records.reply(coder, "done"));
        flow.push(&records.result(lead, "tu_1", "task", "done", false));
        flow.push(&records.reply(lead, "Added greet()."));

        let (lanes, nodes) = flow.snapshot();
        assert_eq!(
            lanes.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(),
            ["Lead", "coder #1"]
        );
        assert_eq!(lanes[1].parent, Some(lead.to_hex()));
        let shown: Vec<(FlowKind, &str)> =
            nodes.iter().map(|n| (n.kind, n.label.as_str())).collect();
        assert_eq!(
            shown,
            [
                (FlowKind::Prompt, "add a greeting"),
                (FlowKind::Model, "sonnet"),
                (FlowKind::Tool, "task coder · write greet()"),
                (FlowKind::Prompt, "write greet()"),
                (FlowKind::Reply, "done"),
                (FlowKind::Result, "coder #1 replied"),
                (FlowKind::Reply, "Added greet()."),
            ]
        );

        let call = node(&nodes, &id(lead, 3));
        assert_eq!(call.status, TraceNodeStatus::Ok);
        assert_eq!(call.detail, "4.2s");
        assert_eq!(
            call.links,
            [link("spawn", id(coder, 1)), link("result", id(lead, 4)),]
        );
        assert_eq!(
            node(&nodes, &id(coder, 2)).links,
            [link("return", id(lead, 4))]
        );
        assert_eq!(node(&nodes, &id(lead, 2)).detail, "1.2k/300 tok · 900ms");
        // Both prompts were answered.
        assert_eq!(node(&nodes, &id(lead, 1)).status, TraceNodeStatus::Ok);
        assert_eq!(node(&nodes, &id(coder, 1)).status, TraceNodeStatus::Ok);
        assert_eq!(node(&nodes, &id(coder, 1)).detail, "haiku");
        assert!(flow.drain().is_none());
    }

    #[test]
    fn calls_the_harness_makes_are_told_apart_by_their_purpose() {
        let lead = agent(1);
        let mut records = Records::default();
        let mut flow = FlowBuilder::new(lead, "Lead");
        let call = |records: &mut Records, purpose: Option<&str>| {
            let mut payload = json!({"model": "haiku", "input_tokens": 170, "output_tokens": 3});
            if let Some(purpose) = purpose {
                payload["purpose"] = json!(purpose);
            }
            records.json(lead, TransactionType::Reasoning, &payload)
        };
        flow.push(&call(&mut records, Some("classify")));
        flow.push(&records.prompt(lead, "hello"));
        flow.push(&call(&mut records, None));
        flow.push(&call(&mut records, Some("judge")));
        flow.push(&call(&mut records, Some("title")));
        let (_, nodes) = flow.snapshot();
        let kinds: Vec<FlowKind> = nodes.iter().map(|n| n.kind).collect();
        assert_eq!(
            kinds,
            [
                FlowKind::Gate,
                FlowKind::Prompt,
                FlowKind::Model,
                FlowKind::Judge,
                FlowKind::Title
            ]
        );
        let gate = node(&nodes, &id(lead, 1));
        assert_eq!(gate.detail, "170/3 tok");
        assert_eq!(
            gate.usage,
            Some(FlowUsage {
                input_tokens: 170,
                output_tokens: 3,
                cost_usd: None,
                duration_ms: None,
            }),
            "an unpriced model has no cost"
        );
    }

    #[test]
    fn a_background_task_reports_back_in_a_notice() {
        let (lead, coder) = (agent(1), agent(2));
        let mut records = Records::default();
        let mut flow = FlowBuilder::new(lead, "Lead");
        flow.add_lane(coder, "coder #1", Some(lead));

        flow.push(&records.call(
            lead,
            "tu_1",
            "task",
            json!({"subagent_type": "coder", "prompt": "write greet()", "spawn_mode": "detached"}),
        ));
        // The call returns before the agent has recorded its prompt.
        flow.push(&records.result(lead, "tu_1", "task", "started", false));
        flow.push(&records.subtask(coder, "coder", "write greet()"));
        flow.push(&records.reply(coder, "done"));
        flow.push(&records.prompt(
            lead,
            "Harness notice:\ncoder #1 finished the task you gave it in the background. Its reply:\n\ndone",
        ));

        let (_, nodes) = flow.snapshot();
        assert!(nodes.iter().all(|n| n.kind != FlowKind::Result));
        assert_eq!(
            node(&nodes, &id(lead, 1)).links,
            [link("spawn", id(coder, 1))]
        );
        assert_eq!(
            node(&nodes, &id(coder, 2)).links,
            [link("return", id(lead, 3))]
        );
    }

    #[test]
    fn spawned_agents_and_their_messages_are_linked() {
        let (lead, scout) = (agent(1), agent(2));
        let mut records = Records::default();
        let mut flow = FlowBuilder::new(lead, "Lead");

        let spawn = records.json(
            lead,
            TransactionType::SubagentSpawn,
            &json!({"parent_agent_id": lead, "child_agent_id": scout, "spawn_mode": "detached"}),
        );
        assert_eq!(spawned_child(&spawn), Some(scout));
        flow.add_lane(scout, "scout", Some(lead));
        flow.push(&spawn);
        flow.push(&records.prompt(scout, "map the crates"));
        flow.push(&records.reply(scout, "12 crates"));
        flow.push(&records.prompt(
            lead,
            &format!("Agent scout ({scout}) reported back:\n\n12 crates"),
        ));
        flow.push(&records.call(
            lead,
            "tu_2",
            "send_to_agent",
            json!({"agent_id": scout.to_hex(), "content": "and the tests?"}),
        ));
        flow.push(&records.prompt(scout, "and the tests?"));
        flow.push(&records.json(
            lead,
            TransactionType::System,
            &json!({"kind": "agent_message", "from_agent_id": scout, "from_name": "scout", "reply": true, "text": "40 tests"}),
        ));

        let (_, nodes) = flow.snapshot();
        let spawn = node(&nodes, &id(lead, 1));
        assert_eq!(
            (spawn.kind, spawn.label.as_str()),
            (FlowKind::Spawn, "spawn scout")
        );
        assert_eq!(spawn.detail, "detached");
        assert_eq!(spawn.links, [link("spawn", id(scout, 1))]);
        assert_eq!(
            node(&nodes, &id(scout, 2)).links,
            [link("return", id(lead, 2))]
        );
        assert_eq!(
            node(&nodes, &id(lead, 3)).links,
            [link("message", id(scout, 3))]
        );
        let message = node(&nodes, &id(lead, 4));
        assert_eq!(
            (message.kind, message.label.as_str()),
            (FlowKind::Message, "scout: 40 tests")
        );
        assert_eq!(
            node(&nodes, &id(scout, 3)).links,
            [link("return", id(lead, 4))]
        );
    }

    #[test]
    fn a_lane_named_late_renames_the_spawn_that_started_it() {
        let (lead, scout) = (agent(1), agent(2));
        let mut records = Records::default();
        let mut flow = FlowBuilder::new(lead, "Lead");
        flow.add_lane(scout, "agent 0202", Some(lead));
        flow.push(&records.json(
            lead,
            TransactionType::SubagentSpawn,
            &json!({"parent_agent_id": lead, "child_agent_id": scout}),
        ));
        let _ = flow.snapshot();

        flow.name_lane(scout, "scout");
        let (lanes, nodes) = flow.drain().expect("the lane was renamed");
        assert_eq!(lanes.len(), 1);
        assert_eq!(lanes[0].name, "scout");
        assert_eq!(nodes[0].label, "spawn scout");
        flow.name_lane(scout, "scout");
        assert!(flow.drain().is_none());
    }

    #[test]
    fn decisions_results_and_outcomes_update_the_node_they_are_about() {
        let lead = agent(1);
        let mut records = Records::default();
        let mut flow = FlowBuilder::new(lead, "Lead");
        let prompt = records.prompt(lead, "clean up");
        let prompt_tx = prompt.tx.hash;
        flow.push(&prompt);
        flow.push(&records.call(
            lead,
            "tu_1",
            "run_command",
            json!({"command": "rm -rf target"}),
        ));
        let (_, loaded) = flow.snapshot();
        assert_eq!(loaded.len(), 2);
        assert_eq!(node(&loaded, &id(lead, 2)).status, TraceNodeStatus::Pending);

        let decision = PolicyDecision {
            tool_use_id: "tu_1".to_string(),
            tool: "run_command".to_string(),
            verdict: ToolGateVerdict::Denied,
            reason: Some("outside the workspace".to_string()),
            decided_by: PolicyRule::new(PolicySource::Mode, "plan"),
            trace: Vec::new(),
            approval: None,
            mode: "plan".to_string(),
            policy_hash: String::new(),
            action_id: None,
        };
        flow.push(&records.json(lead, TransactionType::PolicyDecision, &json!(decision)));
        let (lanes, changed) = flow.drain().expect("the call changed");
        assert!(lanes.is_empty());
        assert_eq!(changed.len(), 1);
        let call = &changed[0];
        assert_eq!(call.id, id(lead, 2));
        assert_eq!(call.label, "run_command rm -rf target");
        assert_eq!(
            (call.status, call.detail.as_str()),
            (TraceNodeStatus::Error, "denied")
        );
        assert_eq!(meta(call, "policy reason"), Some("outside the workspace"));

        flow.push(&records.result(lead, "tu_1", "run_command", "no such file", true));
        let outcome = OutcomePayload {
            prompt_tx,
            category: PromptCategory::SmallChange,
            model: "sonnet".to_string(),
            success: Some(false),
            signal: OutcomeSignal::Error,
            cost_usd: 0.0123,
            subtask_cost_usd: 0.0,
            latency_ms: 2500,
        };
        flow.push(&records.entry(lead, TransactionType::Outcome, outcome.to_bytes()));
        let (_, changed) = flow.drain().expect("two nodes changed");
        assert_eq!(
            changed.iter().map(|n| n.id.clone()).collect::<Vec<_>>(),
            [id(lead, 2), id(lead, 1)]
        );
        assert_eq!(changed[0].detail, "err: no such file");
        assert_eq!(meta(&changed[0], "result"), Some("no such file"));
        assert_eq!(changed[1].status, TraceNodeStatus::Error);
        assert_eq!(changed[1].detail, "sonnet · 2.5s");
        assert_eq!(meta(&changed[1], "cost"), Some("$0.0123"));
        // Neither the decision, the result nor the outcome got a box.
        assert_eq!(flow.snapshot().1.len(), 2);
    }

    #[test]
    fn a_finished_process_points_back_at_what_started_it() {
        let lead = agent(1);
        let mut records = Records::default();
        let mut flow = FlowBuilder::new(lead, "Lead");
        let call = records.call(
            lead,
            "tu_1",
            "run_command",
            json!({"command": "cargo test"}),
        );
        let started = call.tx.hash;
        flow.push(&call);
        let done = ActionResultPayload::success(
            ActionId::new([1; 16]),
            ProcessId::new([2; 16]),
            Some(0),
            "ok".as_bytes().to_vec(),
            61_000,
        );
        let mut entry = records.json(lead, TransactionType::ProcessComplete, &json!(done));
        entry.tx.reference_tx_hash = Some(started);
        flow.push(&entry);

        let (_, nodes) = flow.snapshot();
        assert_eq!(
            node(&nodes, &id(lead, 1)).links,
            [link("completed", id(lead, 2))]
        );
        let process = node(&nodes, &id(lead, 2));
        assert_eq!(process.kind, FlowKind::Process);
        assert_eq!(process.label, "cargo test · ok · exit 0 · 61.0s");
        assert_eq!(meta(process, "command"), Some("cargo test"));
        assert_eq!(meta(process, "exit"), Some("0"));
        assert_eq!(
            meta(process, "process"),
            Some(ProcessId::new([2; 16]).to_hex().as_str())
        );
        assert_eq!(meta(process, "stdout"), Some("ok"));
    }

    #[test]
    fn a_process_started_in_a_batch_finds_its_call_by_action() {
        let lead = agent(1);
        let mut records = Records::default();
        let mut flow = FlowBuilder::new(lead, "Lead");
        let decision = |tool_use_id: &str, action: ActionId| PolicyDecision {
            tool_use_id: tool_use_id.to_string(),
            tool: "run_command".to_string(),
            verdict: ToolGateVerdict::Approved,
            reason: None,
            decided_by: PolicyRule::new(PolicySource::UserDefault, "allow"),
            trace: Vec::new(),
            approval: None,
            mode: "agent".to_string(),
            policy_hash: String::new(),
            action_id: Some(action.to_hex()),
        };
        let (serve, build) = (ActionId::new([1; 16]), ActionId::new([2; 16]));
        // Every call and decision of a batch is written before any runs.
        for (tool_use_id, command) in [("tu_1", "npm run dev"), ("tu_2", "cargo build")] {
            flow.push(&records.call(
                lead,
                tool_use_id,
                "run_command",
                json!({"command": command, "background": true}),
            ));
        }
        flow.push(&records.json(
            lead,
            TransactionType::PolicyDecision,
            &json!(decision("tu_1", serve)),
        ));
        let last = records.json(
            lead,
            TransactionType::PolicyDecision,
            &json!(decision("tu_2", build)),
        );
        let latest = last.tx.hash;
        flow.push(&last);
        // The server started by the first call ends; the agent's latest
        // entry when it started was the second call's decision.
        let done = ActionResultPayload::failure(
            serve,
            ProcessId::new([3; 16]),
            Some(1),
            "port in use".as_bytes().to_vec(),
            1500,
        );
        let mut entry = records.json(lead, TransactionType::ProcessComplete, &json!(done));
        entry.tx.reference_tx_hash = Some(latest);
        flow.push(&entry);

        let (_, nodes) = flow.snapshot();
        assert_eq!(
            node(&nodes, &id(lead, 1)).links,
            [link("completed", id(lead, 5))]
        );
        assert!(node(&nodes, &id(lead, 2)).links.is_empty());
        let process = node(&nodes, &id(lead, 5));
        assert_eq!(process.status, TraceNodeStatus::Error);
        assert_eq!(process.label, "npm run dev · failed · exit 1 · 1.5s");
        assert_eq!(meta(process, "stderr"), Some("port in use"));
    }

    #[test]
    fn a_call_that_got_no_answer_is_a_failed_call() {
        let lead = agent(1);
        let mut records = Records::default();
        let mut flow = FlowBuilder::new(lead, "Lead");
        flow.push(&records.json(
            lead,
            TransactionType::Reasoning,
            &json!({"stop_reason": "Error", "stage": "streaming_handshake", "error": "HTTP 529 overloaded"}),
        ));
        let (_, nodes) = flow.snapshot();
        let call = &nodes[0];
        assert_eq!(call.status, TraceNodeStatus::Error);
        assert_eq!(call.detail, "failed");
        assert_eq!(meta(call, "error"), Some("HTTP 529 overloaded"));
        assert_eq!(meta(call, "stage"), Some("streaming_handshake"));
    }

    #[test]
    fn a_call_counts_its_whole_prompt_and_says_what_was_cached() {
        let lead = agent(1);
        let mut records = Records::default();
        let mut flow = FlowBuilder::new(lead, "Lead");
        flow.push(&records.json(
            lead,
            TransactionType::Reasoning,
            &json!({
                "model": "claude-sonnet-4-6",
                "input_tokens": 200,
                "cache_read_input_tokens": 48_000,
                "cache_creation_input_tokens": 1_000,
                "output_tokens": 516,
                "duration_ms": 3200,
                "thinking": "look at main.rs first",
                "thinking_ms": 1400,
            }),
        ));
        let (_, nodes) = flow.snapshot();
        let call = &nodes[0];
        let usage = call.usage.expect("the call was answered");
        assert_eq!((usage.input_tokens, usage.output_tokens), (49_200, 516));
        assert_eq!(call.detail, "49.2k/516 tok · 3.2s");
        assert_eq!(meta(call, "tokens"), Some("49.2k/516 tok"));
        assert_eq!(meta(call, "cache"), Some("48.0k read · 1.0k written"));
        assert_eq!(meta(call, "took"), Some("3.2s"));
        assert_eq!(meta(call, "thought for"), Some("1.4s"));
        assert_eq!(
            meta(call, "cost").is_some(),
            usage.cost_usd.is_some(),
            "a priced call shows its cost"
        );
    }

    #[test]
    fn later_signals_about_a_turn_are_added_to_the_first() {
        let lead = agent(1);
        let mut records = Records::default();
        let mut flow = FlowBuilder::new(lead, "Lead");
        let prompt = records.prompt(lead, "fix the test");
        let outcome = OutcomePayload {
            prompt_tx: prompt.tx.hash,
            category: PromptCategory::Debugging,
            model: "sonnet".to_string(),
            success: Some(false),
            signal: OutcomeSignal::ChecksFailed,
            cost_usd: 0.02,
            subtask_cost_usd: 0.0,
            latency_ms: 9000,
        };
        flow.push(&prompt);
        for signal in [
            OutcomeSignal::ChecksFailed,
            OutcomeSignal::JudgedExcused,
            OutcomeSignal::JudgedExcused,
        ] {
            let payload = outcome.with_signal(signal);
            flow.push(&records.entry(lead, TransactionType::Outcome, payload.to_bytes()));
        }
        let (_, nodes) = flow.snapshot();
        assert_eq!(
            meta(&nodes[0], "outcome"),
            Some("checks_failed → judged_excused")
        );
    }

    #[test]
    fn entries_of_other_agents_and_repeats_are_ignored() {
        let (lead, other) = (agent(1), agent(9));
        let mut records = Records::default();
        let mut flow = FlowBuilder::new(lead, "Lead");
        let first = records.prompt(lead, "hello");
        flow.push(&first);
        flow.push(&first);
        flow.push(&records.prompt(other, "elsewhere"));
        assert_eq!(flow.snapshot().1.len(), 1);
        assert_eq!(flow.focus(), lead);
        assert_eq!(flow.lane_count(), 1);
    }

    #[test]
    fn the_oldest_nodes_go_once_there_are_too_many() {
        let lead = agent(1);
        let mut records = Records::default();
        let mut flow = FlowBuilder::new(lead, "Lead");
        for n in 0..MAX_NODES + 5 {
            flow.push(&records.reply(lead, &format!("reply {n}")));
        }
        let (_, nodes) = flow.snapshot();
        assert_eq!(nodes.len(), MAX_NODES);
        assert_eq!(nodes[0].label, "reply 5");
        assert_eq!(flow.by_tx.len(), MAX_NODES);
    }

    #[test]
    fn a_benchmark_says_what_each_run_worked_on_and_how_it_did() {
        use aura_core_types::{
            ArmSpec, BenchSource, CompareArm, CompareCopy, CompareRun, CompareStatus, SystemKind,
        };
        let (lead, host, arm_a, arm_b, a_1, b_1, coder) = (
            agent(1),
            agent(2),
            agent(3),
            agent(4),
            agent(5),
            agent(6),
            agent(7),
        );
        let mut flow = FlowBuilder::new(lead, "Lead");
        flow.add_lane(host, "Compare #1", Some(lead));
        flow.add_lane(arm_a, "A · gate", Some(host));
        flow.add_lane(arm_b, "B · opus", Some(host));
        flow.add_lane(a_1, "a-1", Some(arm_a));
        let _ = flow.snapshot();

        let run = |arm, agent_id, check, task: &str| CompareRun {
            name: format!("{arm}-1"),
            arm,
            agent_id,
            prompt_tx: None,
            model: "sonnet".to_string(),
            copy: CompareCopy {
                root: "/c".into(),
                workdir: "/c".into(),
                git_dir: None,
                baseline: "t".to_string(),
            },
            wall_ms: 61_000,
            cost_usd: 0.25,
            tokens: Vec::new(),
            plan_usage: Vec::new(),
            turns: Vec::new(),
            check_passed: Some(check),
            files_changed: 1,
            error: None,
            task: Some(task.to_string()),
        };
        let payload = ComparePayload {
            system_kind: SystemKind::Compare,
            task: "5 example tasks".to_string(),
            plan: None,
            arm_a: ArmSpec::Gate { delegate: true },
            arm_b: ArmSpec::Model {
                model: "opus".to_string(),
            },
            against: "opus".to_string(),
            prompts: Vec::new(),
            source: BenchSource::Prompt,
            repeats: 1,
            check: None,
            budget_usd: None,
            requested_by: lead,
            apply_root: "/repo".into(),
            started_ms: 0,
            runs: vec![
                run(CompareArm::A, a_1, true, "coding/write-file"),
                run(CompareArm::B, b_1, false, "coding/write-file"),
            ],
            judgments: Vec::new(),
            notes: Vec::new(),
            status: CompareStatus::Running,
        };
        let mut records = Records::default();
        flow.push(&records.entry(host, TransactionType::System, payload.to_bytes()));

        // The lanes known so far are reported again with what they are.
        let (lanes, _) = flow.drain().expect("the lanes changed");
        let bench_of = |lanes: &[FlowLane], agent: AgentId| {
            lanes
                .iter()
                .find(|lane| lane.agent_id == agent.to_hex())
                .cloned()
        };
        let a = bench_of(&lanes, a_1)
            .and_then(|lane| lane.bench)
            .expect("a-1 is a run");
        assert_eq!(a.group, "coding/write-file");
        assert_eq!((a.run.as_str(), a.arm), ("a-1", 0));
        assert_eq!(a.arm_label, "gate");
        assert_eq!(a.check_passed, Some(true));
        assert!(bench_of(&lanes, arm_a).is_some_and(|lane| lane.bench_arm));
        assert!(bench_of(&lanes, host).is_none(), "the host did not change");

        // A run that joins later, and an agent under a run, learn theirs.
        flow.add_lane(b_1, "b-1", Some(arm_b));
        flow.add_lane(coder, "coder #1", Some(a_1));
        let (lanes, _) = flow.snapshot();
        let b = bench_of(&lanes, b_1)
            .and_then(|lane| lane.bench)
            .expect("b-1 is a run");
        assert_eq!((b.arm, b.arm_label.as_str()), (1, "opus"));
        assert_eq!(b.check_passed, Some(false));
        let coder = bench_of(&lanes, coder).expect("a lane");
        assert!(coder.bench.is_none() && !coder.bench_arm);
    }
}
