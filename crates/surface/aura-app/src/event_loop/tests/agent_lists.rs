//! Agent lists, archiving, compacting and stopping commands.

use super::*;

/// Archiving is reversible, so it does not refuse an agent with work in
/// flight: it stops that work and archives anyway.
#[tokio::test]
async fn archiving_a_busy_agent_stops_its_turn() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);

    handle_ui_event(&mut state, message(first, "slow job", true), &links).await;
    assert!(state.hub.is_running(first));

    handle_ui_event(&mut state, UiEvent::ArchiveAgent(first.to_hex()), &links).await;
    assert!(
        subagents::lock(&state.hub.registry)
            .find(first)
            .unwrap()
            .archived,
        "archived right away, not blocked on the busy turn"
    );
    let stopped = drain(&mut commands).into_iter().any(|(_, c)| {
        matches!(c, UiCommand::ShowSuccess(text) if text.contains("Archived") && text.contains("stopped"))
    });
    assert!(stopped, "the success message says the turn was stopped");

    let finished = next_done(&mut done).await;
    assert_eq!(finished.agent_id, first);
    handle_work_done(&mut state, finished).await;
    assert!(!state.hub.is_running(first), "the stopped turn ended");
}

/// Index of the first command `pick` matches.
fn position_of(
    commands: &[(Option<String>, UiCommand)],
    pick: impl Fn(&UiCommand) -> bool,
) -> Option<usize> {
    commands.iter().position(|(_, c)| pick(c))
}

/// A new agent is in the list the UI gets before its chat and records,
/// so it shows up without waiting on its history.
#[tokio::test]
async fn a_created_agent_is_listed_before_its_history_is_sent() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut state = loop_state(&fixture, &mut b);
    let _ = drain(&mut commands);

    handle_ui_event(
        &mut state,
        UiEvent::CreateAgent(Some("Fresh".into())),
        &links,
    )
    .await;
    settle(&mut state).await;
    let created = state.active.to_hex();
    let sent = drain(&mut commands);
    let listed = position_of(
        &sent,
        |c| matches!(c, UiCommand::SetAgents(agents) if agents.iter().any(|a| a.id == created)),
    )
    .expect("the new agent is listed");
    let history = position_of(&sent, |c| matches!(c, UiCommand::LoadConversation(_)))
        .expect("its chat is sent");
    let records = position_of(&sent, |c| matches!(c, UiCommand::SetRecords(_)))
        .expect("its records are sent");
    assert!(listed < history && listed < records, "{sent:?}");
    assert!(
        !sent
            .iter()
            .any(|(_, c)| matches!(c, UiCommand::GraphUpsert { .. })),
        "an agent with no record has no stored run to show"
    );
}

/// Archiving the agent on screen takes it off the list before the next
/// agent's chat is read.
#[tokio::test]
async fn archiving_the_active_agent_lists_it_gone_before_opening_the_next() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut state = loop_state(&fixture, &mut b);
    assert_eq!(state.active, first);
    let _ = drain(&mut commands);

    handle_ui_event(&mut state, UiEvent::ArchiveAgent(first.to_hex()), &links).await;
    assert_ne!(state.active, first);
    let gone = first.to_hex();
    let sent = drain(&mut commands);
    let listed = position_of(
        &sent,
        |c| matches!(c, UiCommand::SetAgents(agents) if agents.iter().all(|a| a.id != gone)),
    )
    .expect("the list without it is sent");
    let history = position_of(&sent, |c| matches!(c, UiCommand::LoadConversation(_)))
        .expect("the next agent's chat is sent");
    assert!(listed < history, "{sent:?}");
}

/// An agent in the Archive can be opened; it stays there, and a later
/// list refresh keeps it as the one on screen.
#[tokio::test]
async fn an_archived_agent_opens_and_stays_archived() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let (first, second) = (fixture.first, fixture.second);
    let mut state = loop_state(&fixture, &mut b);
    assert_eq!(state.active, first);

    handle_ui_event(&mut state, UiEvent::ArchiveAgent(second.to_hex()), &links).await;
    handle_ui_event(&mut state, UiEvent::SelectAgent(second.to_hex()), &links).await;
    assert_eq!(state.active, second);
    let _ = drain(&mut commands);

    handle_ui_event(&mut state, UiEvent::RefreshAgents, &links).await;
    let sent = drain(&mut commands);
    let hex = second.to_hex();
    let last_active = sent.iter().rev().find_map(|(_, c)| match c {
        UiCommand::SetActiveAgent(id) => Some(id.clone()),
        _ => None,
    });
    assert_eq!(last_active.as_deref(), Some(hex.as_str()), "{sent:?}");
    let archived_row = sent.iter().rev().find_map(|(_, c)| match c {
        UiCommand::SetArchivedAgents(rows) => rows.iter().find(|row| row.id == hex).cloned(),
        _ => None,
    });
    assert!(
        archived_row.is_some_and(|row| row.is_active),
        "the open agent is marked active in the Archive: {sent:?}"
    );
    assert!(lock_registry_agent(&fixture, second).archived);
}

#[tokio::test]
async fn switching_to_a_busy_agent_keeps_its_live_chat() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let (first, second) = (fixture.first, fixture.second);
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);

    handle_ui_event(&mut state, message(first, "slow job", true), &links).await;
    handle_ui_event(&mut state, UiEvent::SelectAgent(second.to_hex()), &links).await;
    let _ = drain(&mut commands);

    handle_ui_event(&mut state, UiEvent::SelectAgent(first.to_hex()), &links).await;
    let commands = drain(&mut commands);
    assert!(matches!(
        commands.first(),
        Some((None, UiCommand::SetActiveAgent(id))) if *id == first.to_hex()
    ));
    let reloaded = commands
        .iter()
        .any(|(_, c)| matches!(c, UiCommand::LoadConversation(_)));
    assert!(!reloaded, "a busy agent's chat is live, not reloaded");
    assert!(
        commands.iter().any(|(agent, c)| {
            agent.as_deref() == Some(first.to_hex().as_str()) && matches!(c, UiCommand::ChatReady)
        }),
        "the UI is told its live chat is current, so it stops waiting: {commands:?}"
    );

    fixture.gate.add_permits(1);
    let finished = next_done(&mut done).await;
    handle_work_done(&mut state, finished).await;
}

#[tokio::test]
async fn compact_summarizes_the_older_history() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    let history: Vec<aura_model::Message> = std::iter::once(aura_model::Message::user("build it"))
        .chain((0..12).flat_map(|i| {
            [
                aura_model::Message::assistant(format!("step {i}: {}", "x".repeat(400))),
                aura_model::Message::user(format!("ok {i}")),
            ]
        }))
        .collect();
    let before = history.len();
    state.idle.get_mut(&first).unwrap().messages = history;

    let compact = UiEvent::Compact {
        agent_id: first.to_hex(),
        focus: Some("the step numbers".to_string()),
    };
    handle_ui_event(&mut state, compact, &links).await;
    let finished = next_done(&mut done).await;
    handle_work_done(&mut state, finished).await;

    let messages = &state.idle[&first].messages;
    assert!(messages.len() < before, "{} messages left", messages.len());
    assert_eq!(messages[0].text_content(), "build it");
    let commands = drain(&mut commands);
    assert!(commands
        .iter()
        .any(|(_, c)| matches!(c, UiCommand::Compacted(_))));
    assert_eq!(completed(&commands, first), 1);
}

#[tokio::test]
async fn queued_work_is_counted_for_the_status_line() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);

    handle_ui_event(&mut state, message(first, "slow job", true), &links).await;
    let command = UiEvent::RunCommand {
        agent_id: first.to_hex(),
        command: "true".to_string(),
        cols: 80,
    };
    handle_ui_event(&mut state, command, &links).await;
    let queued: Vec<usize> = drain(&mut commands)
        .into_iter()
        .filter_map(|(_, c)| match c {
            UiCommand::SetQueue(items) => Some(items.len()),
            _ => None,
        })
        .collect();
    assert_eq!(queued, [1]);

    fixture.gate.add_permits(1);
    let finished = next_done(&mut done).await;
    handle_work_done(&mut state, finished).await;
    let finished = next_done(&mut done).await;
    handle_work_done(&mut state, finished).await;
    let queued: Vec<usize> = drain(&mut commands)
        .into_iter()
        .filter_map(|(_, c)| match c {
            UiCommand::SetQueue(items) => Some(items.len()),
            _ => None,
        })
        .collect();
    assert_eq!(queued, [0]);
}

#[cfg(unix)]
#[tokio::test]
async fn stop_command_ends_a_running_command_and_reports_it() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);

    let stop = || UiEvent::StopCommand(first.to_hex());
    handle_ui_event(&mut state, stop(), &links).await;
    assert!(drain(&mut commands)
        .iter()
        .any(|(_, c)| matches!(c, UiCommand::SetStatus(s) if s == "No command running")));

    let command = UiEvent::RunCommand {
        agent_id: first.to_hex(),
        command: "echo started; sleep 30".to_string(),
        cols: 80,
    };
    handle_ui_event(&mut state, command, &links).await;
    let started = std::time::Instant::now();
    loop {
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "command never started"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
        handle_ui_event(&mut state, stop(), &links).await;
        if drain(&mut commands)
            .iter()
            .any(|(_, c)| matches!(c, UiCommand::SetStatus(s) if s == "Stopping the command"))
        {
            break;
        }
    }

    let finished = next_done(&mut done).await;
    handle_work_done(&mut state, finished).await;
    assert!(started.elapsed() < Duration::from_secs(10));
    let output = drain(&mut commands)
        .into_iter()
        .find_map(|(_, c)| match c {
            UiCommand::CommandFinished { output, .. } => Some(output),
            _ => None,
        })
        .expect("command finished");
    assert!(output.contains("[STOPPED]"), "{output:?}");
}
