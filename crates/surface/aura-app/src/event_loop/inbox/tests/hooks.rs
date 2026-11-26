//! An agent's peer hooks without background agents behind them: what
//! they send, list and show, and the strangers they refuse.

use super::super::*;
use super::support::{code_of, new_channel, start, take_all};
use crate::event_loop::channels::create_channel;
use crate::event_loop::subagents::tests::{fixture, Fixture};
use tokio::sync::mpsc::UnboundedReceiver;

/// A link to `fixture`'s hub.
fn link(fixture: &Fixture) -> HubLink {
    let link = HubLink::default();
    link.set(&fixture.hub);
    link
}

/// `#site`'s coordinator and a member, with the member's hooks, which
/// fall back to no background agents, and the notices sent from then on.
struct Hooked {
    fixture: Fixture,
    notices: UnboundedReceiver<Notice>,
    coordinator: AgentId,
    member: AgentId,
    hooks: PeerHooks,
}

async fn member_hooks() -> Hooked {
    let mut fixture = fixture("unused");
    let mut notices = fixture.hub.take_notices();
    let channel = create_channel(&fixture.hub, new_channel("site", None))
        .await
        .unwrap();
    let coordinator = channel.coordinator;
    let member = start(&mut fixture, coordinator, "reviewer").await;
    take_all(&mut notices);
    let hooks = PeerHooks::new(link(&fixture), member, None);
    Hooked {
        fixture,
        notices,
        coordinator,
        member,
        hooks,
    }
}

#[tokio::test]
async fn peer_hooks_reach_peers_and_go_on_with_the_last_subject() {
    let mut hooked = member_hooks().await;
    let (coordinator, member) = (hooked.coordinator, hooked.member);
    hooked
        .hooks
        .deliver_message_in_thread(
            AgentMessage::new("coordinator", "The footer is done."),
            Some("Footer"),
        )
        .await
        .unwrap();
    let [told] = take_all(&mut hooked.notices).try_into().unwrap();
    assert_eq!(told.to, coordinator);
    assert!(told.prompt.contains(": Footer\n"), "{}", told.prompt);

    // Without a subject a message goes on with the last one, either way.
    PeerHooks::new(link(&hooked.fixture), coordinator, None)
        .deliver_message(AgentMessage::new(
            &code_of(&hooked.fixture, member),
            "Thanks.",
        ))
        .await
        .unwrap();
    let [answered] = take_all(&mut hooked.notices).try_into().unwrap();
    assert_eq!(answered.to, member);
    assert!(
        answered.prompt.starts_with(&format!(
            "{}: Footer\n",
            code_of(&hooked.fixture, coordinator)
        )),
        "{}",
        answered.prompt
    );
}

#[tokio::test]
async fn peer_hooks_refuse_strangers_without_a_host() {
    let hooked = member_hooks().await;
    let hooks = &hooked.hooks;
    let error = hooks
        .deliver_message(AgentMessage::new("NOPE9", "Hello"))
        .await
        .unwrap_err();
    assert!(error.contains("unknown agent 'NOPE9'"), "{error}");
    assert!(hooks
        .lifecycle("coordinator", None, None, "pause")
        .await
        .is_err());
    assert!(hooks.snapshot("NOPE9").await.is_err());
}

#[tokio::test]
async fn peer_hooks_list_the_agent_and_its_peers_and_show_a_peer() {
    let hooked = member_hooks().await;
    let (hooks, coordinator) = (&hooked.hooks, hooked.coordinator);
    let listed = hooks.list_agents(None).await.unwrap();
    let rows = listed.as_array().unwrap();
    assert_eq!(rows[0]["is_you"], true);
    assert_eq!(rows[0]["agent_id"], hooked.member.to_hex());
    assert_eq!(rows[1]["agent_id"], coordinator.to_hex());
    assert_eq!(rows[1]["relation"], "coordinator");
    assert_eq!(rows[1]["working"], false);

    let state = hooks
        .snapshot(&code_of(&hooked.fixture, coordinator))
        .await
        .unwrap();
    assert_eq!(state["relation"], "coordinator");
}

#[tokio::test]
async fn a_cloud_members_requests_from_its_machine_are_filed_as_its_own_sends() {
    use crate::cloud::hub::MailDesk;
    use aura_engine::{MessageKind, RemoteMail, ThreadId};
    let mut hooked = member_hooks().await;
    let (coordinator, member) = (hooked.coordinator, hooked.member);
    let hub = Arc::clone(&hooked.fixture.hub);
    let other = start(&mut hooked.fixture, coordinator, "reviewer").await;
    take_all(&mut hooked.notices);

    // Its machine lists its peers by the names they go by here.
    let reach = reach_of(&hub, member).await.unwrap();
    let named = |agent: AgentId| {
        let found = reach.iter().find(|peer| peer.agent_id == agent);
        found.map(|peer| peer.name.clone())
    };
    let other_code = code_of(&hooked.fixture, other);
    assert!(named(other).is_some_and(|name| name.starts_with(&format!("{other_code} ("))));
    assert!(named(coordinator).is_some());
    assert_eq!(named(member), None, "not itself");
    assert_eq!(
        reach_of(&hub, lock(&hub.registry).active().agent_id()).await,
        None,
        "an agent in no channel mails its machine's project as before"
    );

    let desk = MachineMail(Arc::downgrade(&hub));
    let request = |to: AgentId, text: &str| RemoteMail {
        from: member,
        from_name: "its name on the machine".into(),
        to,
        kind: MessageKind::Request,
        thread: ThreadId("aura-a:t1".into()),
        subject: "Footer".into(),
        text: text.into(),
    };
    let owed = hub.inbox.owed_by(other);
    desk.file(request(other, "Which footer links are broken?"))
        .await
        .unwrap();
    let [told] = take_all(&mut hooked.notices).try_into().unwrap();
    assert_eq!(told.to, other);
    let member_code = code_of(&hooked.fixture, member);
    assert!(
        told.prompt.starts_with(&format!("{member_code}: Footer\n")),
        "{}",
        told.prompt
    );
    assert!(told.prompt.contains("Which footer links are broken?"));
    assert_eq!(
        hub.inbox.owed_by(other),
        owed + 1,
        "it owes the member an answer"
    );

    let stranger = AgentId::generate();
    let refused = desk.file(request(stranger, "Hello")).await.unwrap_err();
    assert!(refused.contains("unknown agent"), "{refused}");
}
