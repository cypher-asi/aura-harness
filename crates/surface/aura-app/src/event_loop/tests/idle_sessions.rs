//! The event loop keeps a bounded number of idle sessions and lets go of
//! archived agents' (see `event_loop::idle_sessions`).

use super::fixture::{borrowed, fixture, lock_registry_agent, loop_state};
use super::*;
use crate::event_loop::idle_sessions::{self, IDLE_SESSIONS_KEPT};
use crate::event_loop::turns::{Prompt, Request, Work};

/// `count` more agents registered with `fixture`.
fn register(fixture: &Fixture, count: usize) -> Vec<AgentId> {
    let mut registry = subagents::lock(&fixture.hub.registry);
    (0..count)
        .map(|i| {
            registry
                .create(Some(format!("Idle {i}")))
                .unwrap()
                .agent_id()
        })
        .collect()
}

/// Open a session for each of `agents` and leave it idle, oldest first.
async fn leave_idle(fixture: &Fixture, state: &mut LoopState<'_>, agents: &[AgentId]) {
    for &agent in agents {
        let registered = lock_registry_agent(fixture, agent);
        let session = fixture
            .hub
            .factory
            .open(&registered, Vec::new(), None)
            .unwrap();
        state.idle.insert(agent, session);
        idle_sessions::left_idle(state, agent).await;
    }
}

#[tokio::test]
async fn only_the_sessions_most_recently_left_idle_are_kept() {
    let (fixture, _commands) = fixture();
    let agents = register(&fixture, IDLE_SESSIONS_KEPT + 4);
    let mut b = borrowed(&fixture);
    let mut state = loop_state(&fixture, &mut b);
    leave_idle(&fixture, &mut state, &agents).await;
    assert_eq!(state.idle.len(), IDLE_SESSIONS_KEPT);
    assert!(
        state.idle.contains_key(&fixture.first),
        "the agent on screen stays"
    );
    let newest = &agents[agents.len() - (IDLE_SESSIONS_KEPT - 1)..];
    assert!(newest.iter().all(|agent| state.idle.contains_key(agent)));
    assert!(
        !state.idle.contains_key(&agents[0]),
        "the oldest went first"
    );
}

#[tokio::test]
async fn an_agent_with_queued_work_keeps_its_session() {
    let (fixture, _commands) = fixture();
    let agents = register(&fixture, IDLE_SESSIONS_KEPT + 2);
    let mut b = borrowed(&fixture);
    let mut state = loop_state(&fixture, &mut b);
    let work = Work::Prompt {
        prompt: Prompt::inbox("later".to_string(), Request::New),
        shown: false,
    };
    state.queued.push_back(agents[0], work);
    leave_idle(&fixture, &mut state, &agents).await;
    assert!(state.idle.contains_key(&agents[0]), "work waits for it");
    assert!(!state.idle.contains_key(&agents[1]));
}

#[tokio::test]
async fn a_session_let_go_of_reopens_with_its_conversation_and_todos() {
    let (fixture, _commands) = fixture();
    let agents = register(&fixture, 1);
    let mut b = borrowed(&fixture);
    let mut state = loop_state(&fixture, &mut b);
    let registered = lock_registry_agent(&fixture, agents[0]);
    let mut session = fixture
        .hub
        .factory
        .open(&registered, Vec::new(), None)
        .unwrap();
    session.messages = vec![
        aura_model::Message::user("fix the parser"),
        aura_model::Message::assistant("Fixed the escapes."),
    ];
    session.todos = vec![aura_exec_tools::todo::TodoItem {
        id: "1".to_string(),
        content: "Write the escape tests".to_string(),
        status: aura_exec_tools::todo::TodoStatus::Pending,
        assignee: None,
    }];
    let texts = |messages: &[aura_model::Message]| -> Vec<String> {
        messages
            .iter()
            .map(aura_model::Message::text_content)
            .collect()
    };
    let (messages, todos) = (texts(&session.messages), session.todos.clone());
    state.idle.insert(agents[0], session);
    idle_sessions::release_sessions(&mut state, &agents).await;
    assert!(!state.idle.contains_key(&agents[0]), "archived: let go of");

    let reopened = fixture
        .hub
        .factory
        .open(&registered, Vec::new(), None)
        .unwrap();
    assert_eq!(texts(&reopened.messages), messages);
    assert_eq!(reopened.todos, todos);
}

#[tokio::test]
async fn an_agent_that_owes_a_report_keeps_its_session() {
    use super::inbox::{assign_footer, channel_with_member, code};
    let (fixture, _commands) = fixture();
    let coordinator = channel_with_member(&fixture).await;
    let member = fixture.second;
    let mut b = borrowed(&fixture);
    let mut state = loop_state(&fixture, &mut b);
    assign_footer(&fixture, coordinator, &code(&fixture, member)).await;
    assert_eq!(state.hub.inbox.owed_by(member), 1);
    // The member is left idle first, so it is the oldest of the lot.
    let mut agents = vec![member];
    agents.extend(register(&fixture, IDLE_SESSIONS_KEPT + 2));
    leave_idle(&fixture, &mut state, &agents).await;
    assert!(state.idle.contains_key(&member), "its report is owed");
    assert!(!state.idle.contains_key(&agents[1]));
}
