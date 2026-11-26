//! Links between calls, subtasks, spawns and messages.

use super::*;

#[test]
fn a_task_links_the_call_to_the_subtask_and_the_reply_to_the_result() {
    let (lead, coder) = (agent(1), agent(2));
    let mut records = Records::default();
    let mut flow = FlowBuilder::new(lead, "Lead");
    flow.add_lane(coder, "coder #1", Some(lead));
    push_greeting_task(&mut records, &mut flow, lead, coder);

    let (lanes, nodes) = flow.snapshot();
    assert_eq!(
        lanes.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(),
        ["Lead", "coder #1"]
    );
    assert_eq!(lanes[1].parent, Some(lead.to_hex()));
    assert_eq!(
        shown(&nodes),
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

/// `lead` asks `coder` for `greet()` with a `task` call and passes on its reply.
fn push_greeting_task(
    records: &mut Records,
    flow: &mut FlowBuilder,
    lead: AgentId,
    coder: AgentId,
) {
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

/// `scout` reports back to `lead`, gets a message and answers it.
fn push_scout_exchange(
    records: &mut Records,
    flow: &mut FlowBuilder,
    lead: AgentId,
    scout: AgentId,
) {
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
    push_scout_exchange(&mut records, &mut flow, lead, scout);

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
