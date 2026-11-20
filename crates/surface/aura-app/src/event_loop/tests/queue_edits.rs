//! Removing, editing, moving and sending now entries of a busy agent's
//! queue from the UI.

use super::*;
use aura_ui_terminal::events::QueuedItemView;

/// Queue `texts` behind a slow first turn of the first agent.
async fn busy_with_queue(
    state: &mut LoopState<'_>,
    links: &mpsc::Sender<auth::LinkOutcome>,
    agent_id: AgentId,
    texts: &[&str],
) {
    handle_ui_event(state, message(agent_id, "slow start", true), links).await;
    for text in texts {
        handle_ui_event(state, message(agent_id, text, false), links).await;
    }
}

fn queue_texts(state: &LoopState<'_>, agent_id: AgentId) -> Vec<String> {
    state
        .queued
        .views(agent_id)
        .into_iter()
        .map(|v| v.text)
        .collect()
}

fn id_of(state: &LoopState<'_>, agent_id: AgentId, text: &str) -> u64 {
    state
        .queued
        .views(agent_id)
        .into_iter()
        .find(|v| v.text == text)
        .map(|v| v.id)
        .unwrap()
}

/// The last queue the UI was shown for `agent_id`.
fn last_queue_shown(
    commands: &mut mpsc::Receiver<UiCommand>,
    agent_id: AgentId,
) -> Option<Vec<QueuedItemView>> {
    let hex = agent_id.to_hex();
    drain(commands)
        .into_iter()
        .rev()
        .find_map(|(to, c)| match c {
            UiCommand::SetQueue(items) if to.as_deref() == Some(hex.as_str()) => Some(items),
            _ => None,
        })
}

#[tokio::test]
async fn the_ui_is_shown_every_queued_entry_with_its_id() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut state = loop_state(&fixture, &mut b);

    busy_with_queue(&mut state, &links, first, &["one", "two"]).await;
    let shown = last_queue_shown(&mut commands, first).unwrap();
    let texts: Vec<&str> = shown.iter().map(|v| v.text.as_str()).collect();
    assert_eq!(texts, ["one", "two"]);
    assert!(shown.iter().all(|v| v.editable));
    assert_ne!(shown[0].id, shown[1].id);
    fixture.gate.add_permits(3);
}

#[tokio::test]
async fn remove_queued_drops_the_entry_and_reshows_the_queue() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut state = loop_state(&fixture, &mut b);

    busy_with_queue(&mut state, &links, first, &["one", "two"]).await;
    let id = id_of(&state, first, "one");
    drain(&mut commands);
    let remove = UiEvent::RemoveQueued {
        agent_id: first.to_hex(),
        id,
    };
    handle_ui_event(&mut state, remove, &links).await;
    assert_eq!(queue_texts(&state, first), ["two"]);
    let shown = last_queue_shown(&mut commands, first).unwrap();
    assert_eq!(shown.len(), 1);

    let id = id_of(&state, first, "two");
    let remove = UiEvent::RemoveQueued {
        agent_id: first.to_hex(),
        id,
    };
    handle_ui_event(&mut state, remove, &links).await;
    assert!(state.queued.is_empty(), "the last removal empties the map");
    assert_eq!(last_queue_shown(&mut commands, first), Some(Vec::new()));
    fixture.gate.add_permits(1);
}

/// Warnings shown to `agent_id`.
fn warnings_for(commands: &mut mpsc::Receiver<UiCommand>, agent_id: AgentId) -> Vec<String> {
    let hex = agent_id.to_hex();
    drain(commands)
        .into_iter()
        .filter_map(|(to, c)| match c {
            UiCommand::ShowWarning(text) if to.as_deref() == Some(hex.as_str()) => Some(text),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn unknown_ids_are_ignored_but_the_queue_is_reshown() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut state = loop_state(&fixture, &mut b);

    busy_with_queue(&mut state, &links, first, &["one"]).await;
    drain(&mut commands);
    for event in [
        UiEvent::RemoveQueued {
            agent_id: first.to_hex(),
            id: 9_999,
        },
        UiEvent::MoveQueued {
            agent_id: first.to_hex(),
            id: 9_999,
            to: 0,
        },
    ] {
        handle_ui_event(&mut state, event, &links).await;
        let shown = drain(&mut commands);
        let hex = first.to_hex();
        assert!(
            shown
                .iter()
                .any(|(to, c)| matches!(c, UiCommand::SetQueue(_))
                    && to.as_deref() == Some(hex.as_str())),
            "the queue is reshown"
        );
        assert!(
            !shown
                .iter()
                .any(|(_, c)| matches!(c, UiCommand::ShowWarning(_))),
            "a gone entry needs no removing or moving: {shown:?}"
        );
    }
    assert_eq!(queue_texts(&state, first), ["one"]);
    assert!(state.hub.is_running(first), "nothing was cancelled");
    fixture.gate.add_permits(2);
}

#[tokio::test]
async fn editing_or_sending_now_a_gone_entry_warns_the_agents_ui() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut state = loop_state(&fixture, &mut b);

    busy_with_queue(&mut state, &links, first, &["one"]).await;
    drain(&mut commands);
    let edit = UiEvent::EditQueued {
        agent_id: first.to_hex(),
        id: 9_999,
        text: "x".to_string(),
    };
    handle_ui_event(&mut state, edit, &links).await;
    assert_eq!(
        warnings_for(&mut commands, first),
        ["That message already started; your edit wasn't applied"]
    );

    let send = UiEvent::SendQueuedItemNow {
        agent_id: first.to_hex(),
        id: 9_999,
    };
    handle_ui_event(&mut state, send, &links).await;
    let warnings = warnings_for(&mut commands, first);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("nothing was sent now"), "{warnings:?}");

    assert_eq!(queue_texts(&state, first), ["one"]);
    assert!(state.hub.is_running(first), "nothing was cancelled");
    fixture.gate.add_permits(2);
}

#[tokio::test]
async fn move_queued_reorders_and_clamps() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut state = loop_state(&fixture, &mut b);

    busy_with_queue(&mut state, &links, first, &["a", "b", "c"]).await;
    let c = id_of(&state, first, "c");
    let event = UiEvent::MoveQueued {
        agent_id: first.to_hex(),
        id: c,
        to: 0,
    };
    handle_ui_event(&mut state, event, &links).await;
    assert_eq!(queue_texts(&state, first), ["c", "a", "b"]);

    let a = id_of(&state, first, "a");
    let event = UiEvent::MoveQueued {
        agent_id: first.to_hex(),
        id: a,
        to: 100,
    };
    handle_ui_event(&mut state, event, &links).await;
    assert_eq!(queue_texts(&state, first), ["c", "b", "a"]);
    fixture.gate.add_permits(4);
}

#[tokio::test]
async fn edit_queued_changes_a_message_and_leaves_commands_alone() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut state = loop_state(&fixture, &mut b);

    busy_with_queue(&mut state, &links, first, &["typo"]).await;
    let command = UiEvent::RunCommand {
        agent_id: first.to_hex(),
        command: "true".to_string(),
        cols: 80,
    };
    handle_ui_event(&mut state, command, &links).await;

    let id = id_of(&state, first, "typo");
    let edit = UiEvent::EditQueued {
        agent_id: first.to_hex(),
        id,
        text: "fixed".to_string(),
    };
    handle_ui_event(&mut state, edit, &links).await;
    let id = id_of(&state, first, "! true");
    let edit = UiEvent::EditQueued {
        agent_id: first.to_hex(),
        id,
        text: "rm -rf /".to_string(),
    };
    handle_ui_event(&mut state, edit, &links).await;
    assert_eq!(queue_texts(&state, first), ["fixed", "! true"]);
    fixture.gate.add_permits(2);
}

#[tokio::test]
async fn send_queued_item_now_runs_that_item_next() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);

    busy_with_queue(&mut state, &links, first, &["older", "urgent"]).await;
    let id = id_of(&state, first, "urgent");
    let event = UiEvent::SendQueuedItemNow {
        agent_id: first.to_hex(),
        id,
    };
    handle_ui_event(&mut state, event, &links).await;
    // Only the cancellation ends the slow turn; see
    // `send_queued_now_cancels_the_turn_and_runs_the_message_next`.
    let finished = next_done(&mut done).await;
    fixture.gate.add_permits(2);
    handle_work_done(&mut state, finished).await;

    assert!(state.hub.is_running(first), "the chosen message started");
    assert_eq!(queue_texts(&state, first), ["older"], "the old front waits");
    let finished = next_done(&mut done).await;
    handle_work_done(&mut state, finished).await;
    let finished = next_done(&mut done).await;
    handle_work_done(&mut state, finished).await;

    let texts = user_texts(&state, first);
    let position = |text: &str| texts.iter().position(|t| t == text);
    assert!(position("urgent") < position("older"), "{texts:?}");
    assert!(state.queued.is_empty());
}
