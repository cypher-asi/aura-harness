//! Helpers shared by the inbox tests.

use super::super::*;
use crate::event_loop::subagents::tests::Fixture;
use aura_store_db::NewChannel;
use std::collections::HashSet;
use tokio::sync::mpsc::UnboundedReceiver;

pub(super) fn new_channel(slug: &str, project_id: Option<String>) -> NewChannel {
    NewChannel {
        slug: slug.to_string(),
        title: "Website relaunch".to_string(),
        description: "Ship the new marketing site.".to_string(),
        project_id,
        ..NewChannel::default()
    }
}

/// What a coordinator asks for to start a new member from `agent`.
pub(super) fn to_new(agent: &str, subject: &str, prompt: &str) -> AssignArgs {
    AssignArgs {
        to: None,
        agent: Some(agent.to_string()),
        subject: subject.to_string(),
        prompt: prompt.to_string(),
        todo_id: None,
    }
}

/// What a coordinator asks for to give work to the member `to`.
pub(super) fn to_member(to: &str, prompt: &str) -> AssignArgs {
    AssignArgs {
        to: Some(to.to_string()),
        agent: None,
        subject: String::new(),
        prompt: prompt.to_string(),
        todo_id: None,
    }
}

pub(super) fn code_of(fixture: &Fixture, agent: AgentId) -> String {
    lock(&fixture.hub.registry)
        .find(agent)
        .and_then(|a| a.code.clone())
        .unwrap()
}

/// Start a new member of the channel `coordinator` runs from `agent`;
/// returns it. Drains the UI commands it sent, so the fixture's small
/// command channel never fills.
pub(super) async fn start(fixture: &mut Fixture, coordinator: AgentId, agent: &str) -> AgentId {
    let before: HashSet<AgentId> = lock(&fixture.hub.channel_members).clone();
    let said = assign(
        &fixture.hub,
        coordinator,
        to_new(agent, "Kick-off", "Get to know the code."),
    )
    .await
    .unwrap();
    assert!(said.contains("(new member)"), "{said}");
    fixture.drain_commands();
    let after = lock(&fixture.hub.channel_members).clone();
    let [started] = after
        .difference(&before)
        .copied()
        .collect::<Vec<_>>()
        .try_into()
        .unwrap();
    started
}

/// Every notice sent so far.
pub(super) fn take_all(notices: &mut UnboundedReceiver<Notice>) -> Vec<Notice> {
    let mut out = Vec::new();
    while let Ok(notice) = notices.try_recv() {
        out.push(notice);
    }
    out
}
