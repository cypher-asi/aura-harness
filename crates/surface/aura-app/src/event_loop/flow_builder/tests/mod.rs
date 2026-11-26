use super::*;
use aura_core_types::{
    ActionId, ArmSpec, BenchSource, CompareArm, CompareCopy, CompareRun, CompareStatus,
    OutcomePayload, OutcomeSignal, PolicyRule, PolicySource, ProcessId, PromptCategory,
    PromptPayload, Route, RouteReason, SystemKind, Transaction,
};
use serde_json::json;

/// Writes records the way kernels do, one sequence per agent.
#[derive(Default)]
struct Records {
    seqs: HashMap<AgentId, u64>,
    clock: u64,
}

impl Records {
    fn entry(&mut self, agent: AgentId, tx_type: TransactionType, payload: Vec<u8>) -> RecordEntry {
        let seq = self.seqs.entry(agent).or_default();
        *seq += 1;
        self.clock += 10;
        let hash = Hash::from_content(format!("{agent}:{seq}").as_bytes());
        let tx = Transaction::new(hash, agent, self.clock, tx_type, payload);
        RecordEntry::builder(*seq, tx).build()
    }

    fn json(&mut self, agent: AgentId, tx_type: TransactionType, payload: &Value) -> RecordEntry {
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

/// The kind and label of every node, in order.
fn shown(nodes: &[FlowNode]) -> Vec<(FlowKind, &str)> {
    nodes.iter().map(|n| (n.kind, n.label.as_str())).collect()
}

/// The lane of `agent` among `lanes`, if it is there.
fn lane_of(lanes: &[FlowLane], agent: AgentId) -> Option<FlowLane> {
    lanes
        .iter()
        .find(|lane| lane.agent_id == agent.to_hex())
        .cloned()
}

/// A `run_command` decision of `verdict` for `tool_use_id`.
fn decision(
    tool_use_id: &str,
    verdict: ToolGateVerdict,
    reason: Option<&str>,
    decided_by: PolicyRule,
    mode: &str,
    action: Option<ActionId>,
) -> PolicyDecision {
    PolicyDecision {
        tool_use_id: tool_use_id.to_string(),
        tool: "run_command".to_string(),
        verdict,
        reason: reason.map(str::to_string),
        decided_by,
        trace: Vec::new(),
        approval: None,
        mode: mode.to_string(),
        policy_hash: String::new(),
        action_id: action.map(|action| action.to_hex()),
    }
}

/// A finished `sonnet` turn for `prompt_tx`.
fn outcome(
    prompt_tx: Hash,
    category: PromptCategory,
    signal: OutcomeSignal,
    cost_usd: f64,
    latency_ms: u64,
) -> OutcomePayload {
    OutcomePayload {
        prompt_tx,
        category,
        model: "sonnet".to_string(),
        success: Some(false),
        signal,
        cost_usd,
        subtask_cost_usd: 0.0,
        latency_ms,
    }
}

/// A benchmark run of `arm` by `agent_id` on `task`.
fn bench_run(arm: CompareArm, agent_id: AgentId, check: bool, task: &str) -> CompareRun {
    CompareRun {
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
    }
}

/// A running gate-against-opus benchmark asked for by `requested_by`.
fn bench_payload(requested_by: AgentId, runs: Vec<CompareRun>) -> ComparePayload {
    ComparePayload {
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
        classifier: String::new(),
        classify_tokens: Vec::new(),
        requested_by,
        apply_root: "/repo".into(),
        started_ms: 0,
        runs,
        judgments: Vec::new(),
        notes: Vec::new(),
        status: CompareStatus::Running,
    }
}

mod limits;
mod links;
mod updates;
