//! Tests for the recorded tool-call audit trail: every tool call is a
//! `ToolProposal` entry, then a `PolicyDecision` entry naming the rule that
//! decided, then (approved calls only) a `ToolExecution` entry.

use super::*;
use crate::{ExecuteContext, Executor, ExecutorError, ExecutorRouter, ReplayConsumer};
use aura_core_modes::AgentMode;
use aura_core_types::{
    Action, AgentToolPermissions, Effect, EffectKind, EffectStatus, PolicyApprover,
    PolicyCheckOutcome, PolicySource, ToolExecution, ToolGateVerdict, ToolProposal, Transaction,
    TransactionType, UserToolDefaults,
};
use aura_model_reasoner::{MockProvider, ModelProvider};
use aura_store_db::{RocksStore, Store};
use aura_store_snapshot::NoopSnapshotStore;
use std::sync::Mutex as StdMutex;
use tempfile::TempDir;

struct OkExecutor;

#[async_trait]
impl Executor for OkExecutor {
    async fn execute(
        &self,
        _ctx: &ExecuteContext,
        action: &Action,
    ) -> Result<Effect, ExecutorError> {
        let result = aura_core_types::ToolResult::success("read_file", "ok");
        Ok(Effect::new(
            action.action_id,
            EffectKind::Agreement,
            EffectStatus::Committed,
            serde_json::to_vec(&result).unwrap(),
        ))
    }

    fn can_handle(&self, _action: &Action) -> bool {
        true
    }

    fn name(&self) -> &'static str {
        "ok"
    }
}

#[derive(Debug, Default)]
struct RecordingObserver {
    entries: StdMutex<Vec<RecordEntry>>,
}

impl ToolAuditObserver for RecordingObserver {
    fn on_tool_entry(&self, entry: &RecordEntry) {
        self.entries.lock().unwrap().push(entry.clone());
    }
}

#[derive(Debug)]
struct FixedPrompter(ToolApprovalResponse);

#[async_trait]
impl ToolApprovalPrompter for FixedPrompter {
    async fn prompt(
        &self,
        _prompt: PendingToolPrompt,
    ) -> Result<ToolApprovalResponse, ToolApprovalError> {
        Ok(self.0)
    }
}

struct Harness {
    kernel: Kernel,
    store: Arc<dyn Store>,
    observer: Arc<RecordingObserver>,
    _db: TempDir,
    _ws: TempDir,
}

fn harness(configure: impl FnOnce(&mut KernelConfig)) -> Harness {
    let db = TempDir::new().unwrap();
    let ws = TempDir::new().unwrap();
    let store: Arc<dyn Store> = Arc::new(RocksStore::open(db.path(), false).unwrap());
    let provider: Arc<dyn ModelProvider + Send + Sync> =
        Arc::new(MockProvider::simple_response("unused"));
    let observer = Arc::new(RecordingObserver::default());
    let mut config = KernelConfig {
        workspace_base: ws.path().to_path_buf(),
        tool_audit_observer: Some(observer.clone()),
        ..KernelConfig::default()
    };
    configure(&mut config);
    let executor = ExecutorRouter::with_executors(vec![Arc::new(OkExecutor)]);
    let kernel = Kernel::new(
        store.clone(),
        provider,
        executor,
        config,
        AgentId::generate(),
    )
    .unwrap();
    Harness {
        kernel,
        store,
        observer,
        _db: db,
        _ws: ws,
    }
}

fn proposal_tx(kernel: &Kernel, id: &str, tool: &str) -> Transaction {
    let proposal = ToolProposal::new(id, tool, serde_json::json!({ "path": "a.txt" }));
    Transaction::tool_proposal(kernel.agent_id, &proposal).unwrap()
}

fn all_entries(h: &Harness) -> Vec<RecordEntry> {
    h.store.scan_record(h.kernel.agent_id, 1, 100).unwrap()
}

fn decision_of(entry: &RecordEntry) -> PolicyDecision {
    assert_eq!(entry.tx.tx_type, TransactionType::PolicyDecision);
    serde_json::from_slice(&entry.tx.payload).unwrap()
}

#[tokio::test]
async fn allowed_call_records_proposal_then_decision_then_execution() {
    let h = harness(|_| {});
    let result = h
        .kernel
        .process_direct(proposal_tx(&h.kernel, "tu-1", "read_file"))
        .await
        .unwrap();

    let entries = all_entries(&h);
    let types: Vec<_> = entries.iter().map(|e| e.tx.tx_type).collect();
    assert_eq!(
        types,
        vec![
            TransactionType::ToolProposal,
            TransactionType::PolicyDecision,
            TransactionType::ToolExecution,
        ]
    );
    let (proposal, decision_entry, execution) = (&entries[0], &entries[1], &entries[2]);

    assert!(proposal.effects.is_empty() && proposal.actions.is_empty());
    assert_eq!(decision_entry.tx.reference_tx_hash, Some(proposal.tx.hash));
    assert_eq!(execution.tx.reference_tx_hash, Some(decision_entry.tx.hash));

    let decision = decision_of(decision_entry);
    assert_eq!(decision.verdict, ToolGateVerdict::Approved);
    assert_eq!(decision.decided_by.source, PolicySource::UserDefault);
    assert_eq!(decision.decided_by.rule, "full_access");
    assert_eq!(decision.mode, "agent");
    assert_eq!(decision.policy_hash, h.kernel.policy().snapshot_hash());
    let sources: Vec<_> = decision.trace.iter().map(|c| c.rule.source).collect();
    assert_eq!(
        sources,
        vec![
            PolicySource::ActionKind,
            PolicySource::Mode,
            PolicySource::AgentCapability,
            PolicySource::Integration,
            PolicySource::UserDefault,
        ]
    );

    let action_id = execution.actions[0].action_id;
    assert_eq!(decision_entry.decision.accepted_action_ids, vec![action_id]);
    assert_eq!(decision.action_id, Some(action_id.to_hex()));
    assert_eq!(execution.effects.len(), 1);
    let exec: ToolExecution = serde_json::from_slice(&execution.tx.payload).unwrap();
    assert_eq!(exec.decision, ToolGateVerdict::Approved);
    assert!(!exec.is_error);

    assert_eq!(result.entry.seq, execution.seq);
    let decision_ref = result.policy_decision.unwrap();
    assert_eq!(decision_ref.seq, decision_entry.seq);
    assert_eq!(decision_ref.proposal_tx_hash, proposal.tx.hash);

    let observed: Vec<_> = h
        .observer
        .entries
        .lock()
        .unwrap()
        .iter()
        .map(|e| e.seq)
        .collect();
    assert_eq!(observed, vec![1, 2, 3]);
}

#[tokio::test]
async fn denied_call_stops_at_policy_decision_and_names_override() {
    let h = harness(|config| {
        config.policy = config.policy.clone().with_agent_override(Some(
            AgentToolPermissions::new().with("read_file", ToolState::Deny),
        ));
    });
    let result = h
        .kernel
        .process_direct(proposal_tx(&h.kernel, "tu-1", "read_file"))
        .await
        .unwrap();

    let entries = all_entries(&h);
    assert_eq!(entries.len(), 2);
    let decision = decision_of(&entries[1]);
    assert_eq!(decision.verdict, ToolGateVerdict::Denied);
    assert_eq!(decision.decided_by.source, PolicySource::AgentOverride);
    assert_eq!(decision.decided_by.rule, "per_tool[read_file]=off");
    assert_eq!(decision.action_id, None);
    assert_eq!(entries[1].decision.rejected.len(), 1);
    assert!(matches!(
        result.tool_decision,
        Some(ToolDecision::Denied { .. })
    ));
    assert_eq!(result.entry.seq, entries[1].seq);
}

#[tokio::test]
async fn mode_denial_names_the_mode() {
    let h = harness(|config| {
        config.policy = config.policy.clone().with_mode(AgentMode::Ask);
    });
    h.kernel
        .process_direct(proposal_tx(&h.kernel, "tu-1", "run_command"))
        .await
        .unwrap();

    let decision = decision_of(&all_entries(&h)[1]);
    assert_eq!(decision.verdict, ToolGateVerdict::Denied);
    assert_eq!(decision.decided_by.source, PolicySource::Mode);
    assert_eq!(decision.decided_by.rule, "ask");
    assert_eq!(decision.mode, "ask");
}

#[tokio::test]
async fn user_approval_is_recorded_on_the_decision() {
    let h = harness(|config| {
        config.policy = config
            .policy
            .clone()
            .with_user_default(UserToolDefaults::auto_review());
        config.originating_user_id = Some("user-1".into());
        config.tool_approval_prompter = Some(Arc::new(FixedPrompter(ToolApprovalResponse {
            decision: ToolState::Allow,
            remember: ToolApprovalRemember::Session,
        })));
    });
    h.kernel
        .process_direct(proposal_tx(&h.kernel, "tu-1", "read_file"))
        .await
        .unwrap();

    let entries = all_entries(&h);
    assert_eq!(entries.len(), 3);
    let decision = decision_of(&entries[1]);
    assert_eq!(decision.verdict, ToolGateVerdict::Approved);
    assert_eq!(decision.decided_by.source, PolicySource::User);
    assert_eq!(decision.decided_by.rule, "on");
    let approval = decision.approval.unwrap();
    assert_eq!(
        approval.approver,
        PolicyApprover::User {
            user_id: Some("user-1".into())
        }
    );
    assert_eq!(approval.remember.as_deref(), Some("session"));
    let ask = decision
        .trace
        .iter()
        .find(|c| c.rule.source == PolicySource::UserDefault)
        .unwrap();
    assert_eq!(ask.rule.rule, "auto_review");
    assert_eq!(ask.outcome, PolicyCheckOutcome::Ask);

    // The session grant now decides the next call.
    h.kernel
        .process_direct(proposal_tx(&h.kernel, "tu-2", "read_file"))
        .await
        .unwrap();
    let next = decision_of(&all_entries(&h)[4]);
    assert_eq!(next.decided_by.source, PolicySource::SessionGrant);
    assert_eq!(next.decided_by.rule, "read_file=on");
}

#[tokio::test]
async fn ask_without_prompter_records_no_live_session() {
    let h = harness(|config| {
        config.policy = config
            .policy
            .clone()
            .with_user_default(UserToolDefaults::auto_review());
    });
    h.kernel
        .process_direct(proposal_tx(&h.kernel, "tu-1", "read_file"))
        .await
        .unwrap();

    let decision = decision_of(&all_entries(&h)[1]);
    assert_eq!(decision.verdict, ToolGateVerdict::Denied);
    assert_eq!(decision.decided_by.source, PolicySource::NoLiveSession);
}

#[tokio::test]
async fn batch_records_decisions_before_executions_and_replays() {
    let h = harness(|_| {});
    h.kernel
        .process_direct(Transaction::user_prompt(h.kernel.agent_id, "go"))
        .await
        .unwrap();
    let results = h
        .kernel
        .process_tools(
            ["tu-1", "tu-2", "tu-3"]
                .iter()
                .map(|id| ToolProposal::new(*id, "read_file", serde_json::json!({})))
                .collect(),
        )
        .await
        .unwrap();
    assert_eq!(results.len(), 3);

    let types: Vec<_> = all_entries(&h).iter().map(|e| e.tx.tx_type).collect();
    use TransactionType::{PolicyDecision as D, ToolExecution as E, ToolProposal as P};
    assert_eq!(
        types,
        vec![TransactionType::UserPrompt, P, D, P, D, P, D, E, E, E]
    );

    let report = ReplayConsumer::new(
        h.store.clone(),
        Arc::new(NoopSnapshotStore),
        h.kernel.agent_id,
        1,
        KernelConfig::default().record_window_size,
    )
    .run()
    .expect("tool-call entries must replay without context divergence");
    assert_eq!(report.replayed_seqs.len(), 10);
}

#[tokio::test]
async fn policy_decision_transactions_are_kernel_only() {
    let h = harness(|_| {});
    let decision = PolicyDecision {
        tool_use_id: "forged".into(),
        tool: "run_command".into(),
        verdict: ToolGateVerdict::Approved,
        reason: None,
        decided_by: aura_core_types::PolicyRule::new(PolicySource::User, "on"),
        trace: Vec::new(),
        approval: None,
        mode: "agent".into(),
        policy_hash: String::new(),
        action_id: None,
    };
    let tx = Transaction::policy_decision(h.kernel.agent_id, &decision, Hash::default()).unwrap();
    assert!(h.kernel.process_direct(tx).await.is_err());
    assert_eq!(h.store.get_head_seq(h.kernel.agent_id).unwrap(), 0);
}
