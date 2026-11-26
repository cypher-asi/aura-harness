//! Later entries updating the node they are about.

use super::*;

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

    let decision = decision(
        "tu_1",
        ToolGateVerdict::Denied,
        Some("outside the workspace"),
        PolicyRule::new(PolicySource::Mode, "plan"),
        "plan",
        None,
    );
    flow.push(&records.json(lead, TransactionType::PolicyDecision, &json!(decision)));
    assert_denied_call(&mut flow, lead);

    flow.push(&records.result(lead, "tu_1", "run_command", "no such file", true));
    let outcome = outcome(
        prompt_tx,
        PromptCategory::SmallChange,
        OutcomeSignal::Error,
        0.0123,
        2500,
    );
    flow.push(&records.entry(lead, TransactionType::Outcome, outcome.to_bytes()));
    assert_failed_call_and_turn(&mut flow, lead);
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
    let (serve, build) = (ActionId::new([1; 16]), ActionId::new([2; 16]));
    let latest = push_batch(&mut records, &mut flow, lead, (serve, build));
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
    let outcome = outcome(
        prompt.tx.hash,
        PromptCategory::Debugging,
        OutcomeSignal::ChecksFailed,
        0.02,
        9000,
    );
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

/// The denied call is the one node that changed.
#[track_caller]
fn assert_denied_call(flow: &mut FlowBuilder, lead: AgentId) {
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
}

/// The failed result changed the call and the outcome its prompt.
#[track_caller]
fn assert_failed_call_and_turn(flow: &mut FlowBuilder, lead: AgentId) {
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
}

/// Two background calls and their decisions, written as one batch; the
/// hash of the last decision.
fn push_batch(
    records: &mut Records,
    flow: &mut FlowBuilder,
    lead: AgentId,
    (serve, build): (ActionId, ActionId),
) -> Hash {
    let decision = |tool_use_id: &str, action: ActionId| {
        let allow = PolicyRule::new(PolicySource::UserDefault, "allow");
        decision(
            tool_use_id,
            ToolGateVerdict::Approved,
            None,
            allow,
            "agent",
            Some(action),
        )
    };
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
    latest
}
