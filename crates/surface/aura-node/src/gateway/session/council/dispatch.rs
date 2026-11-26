//! Parallel dispatch of council members as council-tagged subagents.

use std::sync::Arc;

use aura_core_protocol::{CouncilMember, CouncilMemberRole};
use aura_core_types::{SubagentDispatchRequest, SubagentResult};
use aura_exec_tools::SubagentDispatchHook;
use tokio_util::sync::CancellationToken;
use tracing::info;

use super::super::chat_run::{spawn_event_forwarder, ChatRunHandle};
use super::super::subagent_stream::RuntimeSubagentObservabilityHook;
use super::prompts::build_reference_advisor_prompt;
use super::{
    CouncilDispatchParams, CouncilRunKind, COUNCIL_MEMBER_KIND,
    SECOND_OPINION_REFERENCE_ADVISOR_PROMPT,
};

/// One council member's resolved outcome, carried into the synthesis
/// prompt in `council_index` order.
pub(super) struct MemberAnswer {
    pub(super) index: usize,
    pub(super) model_label: String,
    pub(super) outcome: Result<SubagentResult, String>,
}

/// Dispatch every council member in parallel through the shared
/// observability hook. Each member emits a `SubagentSpawned` (with
/// `council_index`, `model`, and the shared `council_parent_tool_use_id`)
/// on the parent stream immediately, streams live on its own child run,
/// and resolves to a [`SubagentResult`]. Returns the answers in
/// `council_index` order for synthesis.
pub(super) async fn dispatch_members(
    handle: &Arc<ChatRunHandle>,
    members: &[CouncilMember],
    run_kind: CouncilRunKind,
    query: &str,
    run_id: &str,
    shutdown: &CancellationToken,
    params: &CouncilDispatchParams,
) -> Vec<MemberAnswer> {
    let hook = build_member_hook(handle, run_id, shutdown, params);

    // One synthetic grouping id every member shares so the UI folds them
    // into a single council panel (N columns). Distinct from each
    // member's `tool_call_id` (left `None`) so the `(parent,
    // tool_call_id)` dedupe never collapses two members into one child.
    let group_id = format!("council-{run_id}");

    info!(
        run_id = %run_id,
        member_count = members.len(),
        run_kind = ?run_kind,
        "AURA Council: dispatching members"
    );

    let dispatches = members
        .iter()
        .enumerate()
        .filter(|(index, member)| should_dispatch_member(run_kind, *index, member))
        .map(|(index, member)| {
            let hook = hook.clone();
            let model_label = member.model.id.clone().unwrap_or_default();
            let request = member_request(params, run_kind, query, index, member, &group_id);
            async move {
                let outcome = hook.dispatch(request).await;
                MemberAnswer {
                    index,
                    model_label,
                    outcome,
                }
            }
        });

    let mut answers = futures_util::future::join_all(dispatches).await;
    answers.sort_by_key(|a| a.index);
    answers
}

/// Emit member spawn/status frames into the SAME replay channel the
/// parent run streams over, via a forwarder onto its event channel.
fn build_member_hook(
    handle: &Arc<ChatRunHandle>,
    run_id: &str,
    shutdown: &CancellationToken,
    params: &CouncilDispatchParams,
) -> Arc<RuntimeSubagentObservabilityHook> {
    let parent_outbound = spawn_event_forwarder(handle.events.clone());
    Arc::new(
        RuntimeSubagentObservabilityHook::new(
            Arc::new(params.dispatcher.clone()),
            parent_outbound,
            params.chat_runs.clone(),
            Some(shutdown.clone()),
            Some(run_id.to_string()),
        )
        .with_council_mechanism(Some(params.mechanism.as_wire().to_string())),
    )
}

/// Build one member's [`SubagentDispatchRequest`] off the parent snapshot.
fn member_request(
    params: &CouncilDispatchParams,
    run_kind: CouncilRunKind,
    query: &str,
    index: usize,
    member: &CouncilMember,
    group_id: &str,
) -> SubagentDispatchRequest {
    let (reasoning_effort, budget) = member_execution_profile(&member.model);
    SubagentDispatchRequest {
        parent_agent_id: params.parent_agent_id,
        subagent_type: COUNCIL_MEMBER_KIND.to_string(),
        prompt: member_prompt(run_kind, query),
        originating_user_id: Some(params.originating_user_id.clone()),
        // Members hang directly off the parent (depth 1).
        parent_chain: vec![params.parent_agent_id],
        model_override: member.model.id.clone(),
        reasoning_effort_override: reasoning_effort,
        system_prompt_addendum: member_system_prompt_addendum(run_kind),
        parent_permissions: params.parent_permissions.clone(),
        parent_tool_permissions: params.parent_tool_permissions.clone(),
        user_tool_defaults: params.user_tool_defaults.clone(),
        // No dedupe key: each member is a distinct dispatch.
        tool_call_id: None,
        parent_mode: None,
        parent_kernel_mode: None,
        parent_model_id: Some(params.parent_model_id.clone()),
        override_mode: None,
        override_permissions: None,
        override_tool_subset: member_tool_subset(run_kind),
        override_isolation_id: None,
        override_budget: budget,
        // Wait: block until the member finishes; the hook still emits
        // the spawn frame up-front so all columns appear immediately.
        spawn_mode: None,
        council_index: Some(u32::try_from(index).unwrap_or(u32::MAX)),
        council_parent_tool_use_id: Some(group_id.to_string()),
        check: None,
        todo_id: None,
    }
}

pub(super) fn member_execution_profile(
    model: &aura_core_protocol::ModelSelection,
) -> (Option<String>, Option<aura_core_types::SubagentBudget>) {
    let effort = model
        .reasoning_effort
        .map(|effort| effort.as_wire().to_string());
    let budget = model
        .max_turns
        .is_some()
        .then(|| aura_core_types::SubagentBudget {
            max_iterations: model.max_turns.unwrap_or(aura_core_types::MAX_TURNS),
            ..Default::default()
        });
    (effort, budget)
}

pub(super) fn should_dispatch_member(
    run_kind: CouncilRunKind,
    index: usize,
    member: &CouncilMember,
) -> bool {
    match run_kind {
        CouncilRunKind::Council => true,
        CouncilRunKind::SecondOpinion => {
            index > 0 && member.role == Some(CouncilMemberRole::Reference)
        }
    }
}

pub(super) fn member_prompt(run_kind: CouncilRunKind, query: &str) -> String {
    match run_kind {
        CouncilRunKind::Council => query.to_string(),
        CouncilRunKind::SecondOpinion => build_reference_advisor_prompt(query),
    }
}

pub(super) fn member_system_prompt_addendum(run_kind: CouncilRunKind) -> Option<String> {
    match run_kind {
        CouncilRunKind::Council => None,
        CouncilRunKind::SecondOpinion => Some(SECOND_OPINION_REFERENCE_ADVISOR_PROMPT.to_string()),
    }
}

pub(super) fn member_tool_subset(run_kind: CouncilRunKind) -> Option<Vec<String>> {
    match run_kind {
        CouncilRunKind::Council => None,
        CouncilRunKind::SecondOpinion => Some(Vec::new()),
    }
}
