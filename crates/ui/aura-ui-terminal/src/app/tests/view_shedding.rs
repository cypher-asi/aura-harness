//! Chats of agents long off screen are let go of and load again when
//! shown (see `agent_views::shed`).

use super::*;
use crate::app::agent_switch::KEPT_VIEWS;

const AGENTS: usize = KEPT_VIEWS + 3;

fn agent_id(i: usize) -> String {
    format!("a{i}")
}

/// `turns` user prompts and replies, as the session loads a chat.
fn history(turns: usize) -> Vec<MessageData> {
    (0..turns)
        .flat_map(|t| {
            [
                MessageData::new(MessageRole::User, format!("step {t}")),
                MessageData::new(MessageRole::Assistant, format!("did step {t}")),
            ]
        })
        .collect()
}

/// [`AGENTS`] agents, the first on screen, and where the UI's requests
/// to the session go.
fn app_with_many() -> (App, mpsc::Receiver<UiEvent>) {
    let (tx, rx) = mpsc::channel(64);
    let mut app = App::new().with_event_sender(tx);
    let agents = (0..AGENTS)
        .map(|i| agent(&agent_id(i), &format!("Agent {i}")))
        .collect();
    app.process_command(UiCommand::SetAgents(agents));
    app.process_command(UiCommand::SetActiveAgent(agent_id(0)));
    app.process_command(UiCommand::LoadConversation(history(3)));
    (app, rx)
}

/// Put agent `i` on screen as the session does, with its chat.
fn show(app: &mut App, i: usize) {
    app.process_command(UiCommand::SetActiveAgent(agent_id(i)));
    app.process_command(UiCommand::LoadConversation(history(3)));
}

#[test]
fn only_the_agents_most_recently_on_screen_keep_their_chats() {
    let (mut app, _rx) = app_with_many();
    for i in 1..AGENTS {
        show(&mut app, i);
    }
    // On screen now: the last. Off screen, newest first: the four before.
    for i in 0..AGENTS - 1 {
        let kept = i + 1 + KEPT_VIEWS >= AGENTS;
        assert_eq!(app.holds_chat_of(&agent_id(i)), kept, "agent {i}");
    }
}

#[test]
fn a_shed_chat_is_asked_for_again_and_comes_back_whole() {
    let (mut app, mut rx) = app_with_many();
    for i in 1..AGENTS {
        show(&mut app, i);
    }
    while rx.try_recv().is_ok() {}
    assert!(!app.holds_chat_of(&agent_id(0)));
    assert!(app.switch_on_screen(&agent_id(0)));
    let asked = std::iter::from_fn(|| rx.try_recv().ok())
        .any(|event| matches!(&event, UiEvent::SelectAgent(id) if *id == agent_id(0)));
    assert!(asked, "the shed chat is loaded, not focused");
    app.process_command(UiCommand::LoadConversation(history(3)));
    assert_eq!(app.messages.len(), 6, "the whole chat is back");
}

#[test]
fn a_chat_with_a_turn_in_flight_is_kept() {
    let (mut app, _rx) = app_with_many();
    let busy = for_agent(&agent_id(1), UiCommand::TurnStarted { prompt: None });
    app.process_command(busy);
    app.process_command(for_agent(&agent_id(1), UiCommand::StartStreaming));
    app.process_command(for_agent(
        &agent_id(1),
        UiCommand::AppendText("half a reply".to_string()),
    ));
    for i in 2..AGENTS {
        show(&mut app, i);
    }
    assert!(app.holds_chat_of(&agent_id(1)), "its turn still runs");
    app.process_command(for_agent(&agent_id(1), UiCommand::FinishStreaming));
    app.process_command(for_agent(&agent_id(1), UiCommand::Complete));
    assert!(
        app.holds_chat_of(&agent_id(1)),
        "kept as its turn just ended"
    );
    show(&mut app, 0);
    assert!(!app.holds_chat_of(&agent_id(1)), "shed at the next switch");
}
