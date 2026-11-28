//! Approvals and permission modes.

use super::*;

/// Wait for the approval prompt addressed to `agent_id` and return its id.
async fn approval_for(commands: &mut mpsc::Receiver<UiCommand>, agent_id: AgentId) -> String {
    let hex = agent_id.to_hex();
    let wait = async {
        loop {
            match commands.recv().await.expect("command channel closed") {
                UiCommand::ForAgent { agent_id, command } if agent_id == hex => {
                    if let UiCommand::RequestApproval { id, .. } = *command {
                        return id;
                    }
                }
                _ => {}
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(10), wait)
        .await
        .expect("no approval was asked for")
}

#[tokio::test]
async fn a_command_waits_for_approval_and_runs_once_allowed() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);

    handle_ui_event(&mut state, message(first, "run ls", true), &links).await;
    let id = approval_for(&mut commands, first).await;
    assert!(state.hub.is_running(first));
    assert!(done.try_recv().is_err(), "the turn waits for the answer");

    handle_ui_event(&mut state, UiEvent::Approve(id), &links).await;
    let finished = next_done(&mut done).await;
    handle_work_done(&mut state, finished).await;
    assert_eq!(tool_results(&state, first), [false]);
}

#[tokio::test]
async fn cancel_denies_the_waiting_call_and_ends_the_turn() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);

    handle_ui_event(&mut state, message(first, "run ls", true), &links).await;
    let id = approval_for(&mut commands, first).await;
    handle_ui_event(&mut state, UiEvent::Cancel(first.to_hex()), &links).await;

    let finished = next_done(&mut done).await;
    handle_work_done(&mut state, finished).await;
    assert!(!state.approvals.is_waiting(first));
    assert!(!state.hub.is_running(first));
    let closed = drain(&mut commands)
        .into_iter()
        .any(|(_, c)| matches!(c, UiCommand::CloseApproval(ref c) if *c == id));
    assert!(closed, "the prompt is taken down");
}

#[tokio::test]
async fn a_background_agent_asks_in_its_own_chat() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let (first, second) = (fixture.first, fixture.second);
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    assert_eq!(state.active, first);

    handle_ui_event(&mut state, message(second, "run ls", true), &links).await;
    let id = approval_for(&mut commands, second).await;
    assert!(id.starts_with(&second.to_hex()), "{id}");

    handle_ui_event(&mut state, UiEvent::Deny(id), &links).await;
    let finished = next_done(&mut done).await;
    assert_eq!(finished.agent_id, second);
    handle_work_done(&mut state, finished).await;
    assert_eq!(tool_results(&state, second), [true], "the call was denied");
}

#[tokio::test]
async fn a_mode_switch_is_recorded_before_it_takes_effect() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut state = loop_state(&fixture, &mut b);
    let set_plan = UiEvent::SetMode {
        agent_id: first.to_hex(),
        mode: PermissionMode::Plan,
    };

    handle_ui_event(&mut state, set_plan, &links).await;
    let kernel = Arc::clone(&state.idle[&first].kernel);
    assert_eq!(kernel.policy().mode_label(), "plan");
    assert_eq!(state.hub.mode_of(first), PermissionMode::Plan);
    let records = kernel.store().scan_record(first, 0, 100).unwrap();
    let switch = records
        .iter()
        .find_map(|e| e.tx.mode_change_payload())
        .expect("a mode_change record for the switch");
    assert_eq!(switch.from_permissions.as_deref(), Some("agent/ask"));
    assert_eq!(switch.to_permissions.as_deref(), Some("plan/ask"));
    assert_eq!(switch.to, aura_core_types::AgentMode::Plan);

    handle_ui_event(&mut state, UiEvent::CycleApproval(first.to_hex()), &links).await;
    assert_eq!(
        state.hub.permissions_of(first).approval,
        ApprovalLevel::Strict
    );
    assert_eq!(state.hub.mode_of(first), PermissionMode::Plan);
    let records = kernel.store().scan_record(first, 0, 100).unwrap();
    let last = records
        .iter()
        .filter_map(|e| e.tx.mode_change_payload())
        .last()
        .unwrap();
    assert_eq!(last.to_permissions.as_deref(), Some("plan/strict"));
    assert_eq!(kernel.policy().approval(), ApprovalLevel::Strict);
    assert_mode_then_approval_shown(&mut commands, first);
}

/// The chat of `agent` was shown the Plan mode, then the Strict approval.
fn assert_mode_then_approval_shown(commands: &mut mpsc::Receiver<UiCommand>, agent: AgentId) {
    let shown: Vec<UiCommand> = drain(commands)
        .into_iter()
        .filter(|(to, _)| *to == Some(agent.to_hex()))
        .map(|(_, c)| c)
        .filter(|c| matches!(c, UiCommand::SetMode(_) | UiCommand::SetApproval(_)))
        .collect();
    assert!(
        matches!(
            shown.as_slice(),
            [
                UiCommand::SetMode(PermissionMode::Plan),
                UiCommand::SetApproval(ApprovalLevel::Strict)
            ]
        ),
        "{shown:?}"
    );
}

/// Whether the Agents panel rows say each agent is in plan mode, by hex
/// id: from the lists sent to the UI, then from the hub's summaries.
fn plan_marks(hub: &subagents::AgentHub) -> [HashMap<String, bool>; 2] {
    let marks = |rows: Vec<aura_ui::events::AgentSummary>| {
        rows.into_iter()
            .map(|row| (row.id, row.plan_mode))
            .collect::<HashMap<_, _>>()
    };
    let listed = hub
        .agent_lists()
        .into_iter()
        .find_map(|command| match command {
            UiCommand::SetAgents(rows) => Some(rows),
            _ => None,
        })
        .expect("the lists hold the Agents panel rows");
    [marks(listed), marks(hub.agent_summaries())]
}

#[tokio::test]
async fn a_switch_to_plan_mode_marks_the_agent_in_the_agent_lists() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let (first, second) = (fixture.first.to_hex(), fixture.second.to_hex());
    let mut state = loop_state(&fixture, &mut b);
    let expect = |first_on: bool, second_on: bool| {
        let marks = HashMap::from([(first.clone(), first_on), (second.clone(), second_on)]);
        [marks.clone(), marks]
    };
    assert_eq!(plan_marks(&state.hub), expect(false, false));

    let set_plan = UiEvent::SetMode {
        agent_id: first.clone(),
        mode: PermissionMode::Plan,
    };
    handle_ui_event(&mut state, set_plan, &links).await;
    assert_eq!(plan_marks(&state.hub), expect(true, false));

    // `/plan` again leaves plan mode, and once more enters it.
    handle_ui_event(&mut state, UiEvent::TogglePlan(first.clone()), &links).await;
    assert_eq!(plan_marks(&state.hub), expect(false, false));
    handle_ui_event(&mut state, UiEvent::TogglePlan(first.clone()), &links).await;
    assert_eq!(plan_marks(&state.hub), expect(true, false));
}

#[tokio::test]
async fn a_mode_switch_and_a_request_send_the_agents_policies() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut state = loop_state(&fixture, &mut b);
    let policies_of = |commands: &mut mpsc::Receiver<UiCommand>| -> Vec<PolicyView> {
        drain(commands)
            .into_iter()
            .filter(|(to, _)| *to == Some(first.to_hex()))
            .filter_map(|(_, c)| match c {
                UiCommand::SetPolicies(view) => Some(view),
                _ => None,
            })
            .collect()
    };
    drain(&mut commands);

    let set_plan = UiEvent::SetMode {
        agent_id: first.to_hex(),
        mode: PermissionMode::Plan,
    };
    handle_ui_event(&mut state, set_plan, &links).await;
    let sent = policies_of(&mut commands);
    let [view] = sent.as_slice() else {
        panic!("expected one SetPolicies after the switch, got {sent:?}");
    };
    assert_eq!(view.mode, PermissionMode::Plan);
    assert_eq!(view.approval, ApprovalLevel::Ask);
    assert_eq!(
        view.policy_hash,
        state.idle[&first].kernel.policy().snapshot_hash()
    );
    let write = view
        .tools
        .iter()
        .find(|tool| tool.name == "write_file")
        .expect("write_file is offered");
    assert_eq!(write.state, ToolPolicyState::Off);

    handle_ui_event(&mut state, UiEvent::RequestPolicies(first.to_hex()), &links).await;
    let sent = policies_of(&mut commands);
    assert_eq!(
        sent.as_slice(),
        std::slice::from_ref(view),
        "a request resends the same view"
    );
}

#[tokio::test]
async fn plan_again_returns_to_the_mode_from_before() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut state = loop_state(&fixture, &mut b);
    let agent = first.to_hex();

    handle_ui_event(
        &mut state,
        UiEvent::SetMode {
            agent_id: agent.clone(),
            mode: PermissionMode::Debug,
        },
        &links,
    )
    .await;
    handle_ui_event(&mut state, UiEvent::TogglePlan(agent.clone()), &links).await;
    assert_eq!(state.hub.mode_of(first), PermissionMode::Plan);
    assert_eq!(state.idle[&first].kernel.policy().mode_label(), "plan");

    handle_ui_event(&mut state, UiEvent::TogglePlan(agent), &links).await;
    assert_eq!(state.hub.mode_of(first), PermissionMode::Debug);
    assert_eq!(state.idle[&first].kernel.policy().mode_label(), "debug");
}

#[tokio::test]
async fn yolo_runs_commands_without_asking() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    let yolo = UiEvent::SetApproval {
        agent_id: first.to_hex(),
        level: ApprovalLevel::Yolo,
    };
    if !crate::event_loop::modes::commands_confinable() {
        return;
    }

    handle_ui_event(&mut state, yolo, &links).await;
    handle_ui_event(&mut state, message(first, "run ls", true), &links).await;
    let finished = next_done(&mut done).await;
    handle_work_done(&mut state, finished).await;
    assert_eq!(tool_results(&state, first), [false]);
    let asked = drain(&mut commands)
        .into_iter()
        .any(|(_, c)| matches!(c, UiCommand::RequestApproval { .. }));
    assert!(!asked);
}

#[tokio::test]
async fn init_asks_the_agent_to_improve_an_existing_agents_md() {
    let (fixture, mut commands) = fixture();
    std::fs::write(fixture._dirs[2].path().join("AGENTS.md"), "Old notes.").unwrap();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);

    let init = UiEvent::InitProject {
        agent_id: first.to_hex(),
    };
    handle_ui_event(&mut state, init, &links).await;
    let finished = next_done(&mut done).await;
    handle_work_done(&mut state, finished).await;

    let prompts = user_texts(&state, first);
    assert_eq!(prompts.len(), 1);
    assert!(
        prompts[0].contains("already has an AGENTS.md"),
        "{prompts:?}"
    );
    let commands = drain(&mut commands);
    assert!(
        commands.iter().any(|(_, c)| matches!(
            c,
            UiCommand::TurnStarted { prompt: Some(p) } if p.contains("improve it")
        )),
        "the chat shows the request"
    );
    assert!(
        commands.iter().any(|(_, c)| matches!(
            c,
            UiCommand::SetInstructions(files)
                if files.iter().any(|f| f.path.ends_with("AGENTS.md"))
        )),
        "the agent's chat learns which instruction files it loads"
    );
}

/// The system notes shown in `agent`'s chat among `sent`.
fn notes_to(sent: &[(Option<String>, UiCommand)], agent: AgentId) -> Vec<String> {
    let hex = agent.to_hex();
    sent.iter()
        .filter(|(to, _)| to.as_deref() == Some(hex.as_str()))
        .filter_map(|(_, command)| match command {
            UiCommand::ShowMessage(message) => Some(message.content.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn plan_mode_asks_the_user_to_build_only_when_the_factory_does_not() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut state = loop_state(&fixture, &mut b);
    let mode = |mode| UiEvent::SetMode {
        agent_id: first.to_hex(),
        mode,
    };

    let _ = drain(&mut commands);
    handle_ui_event(&mut state, mode(PermissionMode::Plan), &links).await;
    let own = notes_to(&drain(&mut commands), first);
    assert!(own.iter().any(|n| n.contains("/build")), "{own:?}");

    handle_ui_event(&mut state, mode(PermissionMode::Agent), &links).await;
    state.hub.factory_flows.assign(first);
    assert!(state.hub.factory_flows.begin(first));
    let _ = drain(&mut commands);
    handle_ui_event(&mut state, mode(PermissionMode::Plan), &links).await;
    let factory = notes_to(&drain(&mut commands), first);
    let note = factory
        .iter()
        .find(|n| n.starts_with("Plan mode:"))
        .expect("a plan mode note");
    assert!(
        !note.contains("/build") && !note.contains("implement\""),
        "{note}"
    );
    assert!(note.contains("on its own"), "{note}");
}
