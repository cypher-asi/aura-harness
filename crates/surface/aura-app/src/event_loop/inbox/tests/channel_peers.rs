//! Channel members reaching each other whatever their place in the spawn
//! tree: a member the coordinator assigned is registered top-level, one
//! it spawned is registered under it, and both are its channel's agents.
//! Each lists the other, messages it by code, locally and from a cloud
//! machine, and strangers stay out.

use super::super::peers::circle_of;
use super::super::*;
use super::support::{code_of, new_channel, start, take_all};
use crate::cloud::hub::MailDesk;
use crate::event_loop::channels::{channel_prompt, create_channel};
use crate::event_loop::subagents::tests::{fixture, request, Fixture};
use aura_engine::{MessageKind, RemoteMail, ThreadId};
use aura_store_db::MemberRole;
use tokio::sync::mpsc::UnboundedReceiver;

/// `#site` with a member the coordinator assigned and one it spawned,
/// and `#other` with a member of its own.
struct Site {
    fixture: Fixture,
    notices: UnboundedReceiver<Notice>,
    coordinator: AgentId,
    assigned: AgentId,
    spawned: AgentId,
    stranger: AgentId,
}

async fn site() -> Site {
    let mut fixture = fixture("unused");
    let mut notices = fixture.hub.take_notices();
    let channel = create_channel(&fixture.hub, new_channel("site", None))
        .await
        .unwrap();
    let coordinator = channel.coordinator;
    let assigned = start(&mut fixture, coordinator, "reviewer").await;
    let spawned = fixture
        .dispatcher
        .dispatch(request(coordinator, "tests", "Write the tests"))
        .await
        .unwrap()
        .child_agent_id
        .unwrap();
    let other = create_channel(&fixture.hub, new_channel("other", None))
        .await
        .unwrap();
    let stranger = start(&mut fixture, other.coordinator, "reviewer").await;
    fixture.drain_commands();
    take_all(&mut notices);
    Site {
        fixture,
        notices,
        coordinator,
        assigned,
        spawned,
        stranger,
    }
}

fn link(fixture: &Fixture) -> HubLink {
    let link = HubLink::default();
    link.set(&fixture.hub);
    link
}

fn ids(circle: &Circle) -> Vec<AgentId> {
    circle
        .peers
        .iter()
        .map(|peer| peer.agent.agent_id)
        .collect()
}

fn mail(from: AgentId, to: AgentId, text: &str) -> RemoteMail {
    RemoteMail {
        from,
        from_name: "spawned".to_string(),
        to,
        kind: MessageKind::Request,
        thread: ThreadId("aura-x:t1".to_string()),
        subject: "Drawer".to_string(),
        text: text.to_string(),
    }
}

#[tokio::test]
async fn a_spawned_member_is_a_full_member_though_not_top_level() {
    let site = site().await;
    let hub = &site.fixture.hub;
    let store = hub.channels.as_ref().unwrap();
    let channel = store.channel_of(&site.spawned).unwrap().unwrap();
    let joined = store
        .members(&channel)
        .unwrap()
        .into_iter()
        .find(|member| member.agent_id == site.spawned)
        .unwrap();
    assert_eq!(joined.role, MemberRole::Agent);
    assert!(
        !lock(&hub.registry).is_top_level_agent(site.spawned),
        "registered under the coordinator, the case that was left out"
    );
}

#[tokio::test]
async fn both_kinds_of_member_reach_each_other_and_the_coordinator() {
    let site = site().await;
    let hub = &site.fixture.hub;
    let spawned = circle_of(hub, site.spawned).expect("a circle of its own");
    assert_eq!(spawned.slug(), "site");
    assert!(
        ids(&spawned).contains(&site.assigned),
        "{:?}",
        ids(&spawned)
    );
    assert!(ids(&spawned).contains(&site.coordinator));
    let assigned = circle_of(hub, site.assigned).unwrap();
    assert!(
        ids(&assigned).contains(&site.spawned),
        "{:?}",
        ids(&assigned)
    );
    let lead = circle_of(hub, site.coordinator).unwrap();
    assert!(ids(&lead).contains(&site.spawned) && ids(&lead).contains(&site.assigned));
    let code = code_of(&site.fixture, site.assigned);
    assert_eq!(
        spawned.find(&code.to_lowercase()).unwrap().agent.agent_id,
        site.assigned
    );
    for circle in [&spawned, &assigned, &lead] {
        assert!(
            !ids(circle).contains(&site.stranger),
            "another channel's member"
        );
    }
}

#[tokio::test]
async fn members_list_and_message_each_other_by_code() {
    let mut site = site().await;
    let hooks = PeerHooks::new(link(&site.fixture), site.spawned, None);
    let listed = hooks.list_agents(None).await.unwrap().to_string();
    let assigned_code = code_of(&site.fixture, site.assigned);
    assert!(listed.contains(&assigned_code), "{listed}");
    assert!(
        !listed.contains(&code_of(&site.fixture, site.stranger)),
        "{listed}"
    );

    hooks
        .deliver_message_in_thread(
            AgentMessage::new(&assigned_code.to_lowercase(), "Which drawer?"),
            Some("Drawer"),
        )
        .await
        .unwrap();
    let [told] = take_all(&mut site.notices).try_into().unwrap();
    assert_eq!(told.to, site.assigned);

    // And back, from the assigned member to the spawned one.
    let back = PeerHooks::new(link(&site.fixture), site.assigned, None);
    back.deliver_message(AgentMessage::new(
        &code_of(&site.fixture, site.spawned),
        "The tasks one.",
    ))
    .await
    .unwrap();
    let [answered] = take_all(&mut site.notices).try_into().unwrap();
    assert_eq!(answered.to, site.spawned);
}

#[tokio::test]
async fn a_cloud_member_reaches_its_channel_and_its_mail_is_filed() {
    let mut site = site().await;
    let hub = &site.fixture.hub;
    let reach = reach_of(hub, site.spawned).await.expect("a channel member");
    let reached: Vec<AgentId> = reach.iter().map(|peer| peer.agent_id).collect();
    assert!(reached.contains(&site.assigned) && reached.contains(&site.coordinator));
    assert!(!reached.contains(&site.stranger));
    let code = code_of(&site.fixture, site.assigned);
    assert!(
        reach
            .iter()
            .any(|peer| peer.name.starts_with(&format!("{code} ("))),
        "named by code first, which machines resolve: {reach:?}"
    );

    let desk = MachineMail(Arc::downgrade(hub));
    desk.file(mail(site.spawned, site.assigned, "Which drawer?"))
        .await
        .expect("filed in the inbox");
    let [told] = take_all(&mut site.notices).try_into().unwrap();
    assert_eq!(told.to, site.assigned);
}

#[tokio::test]
async fn strangers_are_refused_with_the_codes_one_can_reach() {
    let site = site().await;
    let hub = &site.fixture.hub;
    let stranger_code = code_of(&site.fixture, site.stranger);
    let assigned_code = code_of(&site.fixture, site.assigned);

    let desk = MachineMail(Arc::downgrade(hub));
    let refused = desk
        .file(mail(site.spawned, site.stranger, "Psst"))
        .await
        .unwrap_err();
    assert!(refused.contains("unknown agent"), "{refused}");
    assert!(refused.contains(&assigned_code), "{refused}");

    let hooks = PeerHooks::new(link(&site.fixture), site.spawned, None);
    for target in [stranger_code.as_str(), "RUS999"] {
        let error = hooks
            .deliver_message(AgentMessage::new(target, "Psst"))
            .await
            .unwrap_err();
        assert!(
            error.contains(&format!("unknown agent '{target}'")),
            "{error}"
        );
        assert!(
            error.contains("the members of #site are:") && error.contains(&assigned_code),
            "{error}"
        );
        if target != stranger_code {
            assert!(!error.contains(&stranger_code), "{error}");
        }
    }
    // The user's agent outside every channel is no peer either.
    let outsider = lock(&hub.registry).active().agent_id();
    assert!(circle_of(hub, site.spawned)
        .unwrap()
        .find(&outsider.to_hex())
        .is_none());
}

#[tokio::test]
async fn a_spawned_member_gets_its_channel_block() {
    let site = site().await;
    let block = channel_prompt(&site.fixture.hub, site.spawned).expect("its channel's block");
    assert!(block.contains("a member of the channel #site"), "{block}");
    assert!(
        block.contains(&code_of(&site.fixture, site.assigned)),
        "{block}"
    );
    let lead = channel_prompt(&site.fixture.hub, site.coordinator).unwrap();
    assert!(
        lead.contains(&code_of(&site.fixture, site.spawned)),
        "{lead}"
    );
}
