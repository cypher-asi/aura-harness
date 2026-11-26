//! Who reaches whom: coordinators, members, their project's agents and
//! subagents.

use super::super::peers::circle_of;
use super::super::*;
use super::support::{code_of, new_channel, start};
use crate::event_loop::channels::create_channel;
use crate::event_loop::subagents::tests::{fixture, request, Fixture};
use std::path::Path;
use tempfile::TempDir;

fn peers(circle: &Circle) -> Vec<(AgentId, Relation)> {
    circle
        .peers
        .iter()
        .map(|peer| (peer.agent.agent_id, peer.relation))
        .collect()
}

fn open_project(fixture: &Fixture, dir: &Path) -> (String, AgentId) {
    let mut registry = lock(&fixture.hub.registry);
    let (agent, _) = registry.open_project(dir).unwrap();
    (agent.project_id.clone().unwrap(), agent.agent_id())
}

/// `#site`, in a project with an agent of its own, run by `coordinator`
/// with the members `first` and `second`, and a `subagent` of `first`.
struct Site {
    _dir: TempDir,
    fixture: Fixture,
    project_agent: AgentId,
    coordinator: AgentId,
    first: AgentId,
    second: AgentId,
    subagent: AgentId,
}

async fn site_in_a_project() -> Site {
    let mut fixture = fixture("unused");
    let dir = TempDir::new().unwrap();
    let (project_id, project_agent) = open_project(&fixture, dir.path());
    let channel = create_channel(&fixture.hub, new_channel("site", Some(project_id)))
        .await
        .unwrap();
    let coordinator = channel.coordinator;
    let first = start(&mut fixture, coordinator, "reviewer").await;
    let second = start(&mut fixture, coordinator, "tests").await;
    let subagent = fixture
        .dispatcher
        .dispatch(request(first, "explorer", "Look around"))
        .await
        .unwrap()
        .child_agent_id
        .unwrap();
    fixture.drain_commands();
    Site {
        _dir: dir,
        fixture,
        project_agent,
        coordinator,
        first,
        second,
        subagent,
    }
}

#[tokio::test]
async fn coordinators_reach_their_members() {
    let site = site_in_a_project().await;
    let lead = circle_of(&site.fixture.hub, site.coordinator).unwrap();
    assert!(lead.coordinates());
    assert_eq!(lead.slug(), "site");
    assert_eq!(
        peers(&lead),
        [
            (site.first, Relation::Member),
            (site.second, Relation::Member)
        ],
        "only the channel's own agents, not its project's or subagents"
    );
}

#[tokio::test]
async fn members_reach_their_channel_and_their_project_too() {
    let site = site_in_a_project().await;
    let (coordinator, second) = (site.coordinator, site.second);
    let member = circle_of(&site.fixture.hub, site.first).unwrap();
    assert!(!member.coordinates());
    assert_eq!(
        peers(&member),
        [
            (coordinator, Relation::Coordinator),
            (second, Relation::Member),
            (site.project_agent, Relation::Project),
        ]
    );
    let code = code_of(&site.fixture, second);
    assert_eq!(
        member.find(&code.to_lowercase()).unwrap().agent.agent_id,
        second
    );
    assert_eq!(
        member.find(&second.to_hex()).unwrap().agent.agent_id,
        second
    );
    assert_eq!(
        member.find("@coordinator").unwrap().agent.agent_id,
        coordinator
    );
    assert!(member.find(&site.subagent.to_hex()).is_none());
}

#[tokio::test]
async fn the_project_reaches_a_channels_members_and_subagents_reach_nobody() {
    let site = site_in_a_project().await;
    let outside = circle_of(&site.fixture.hub, site.project_agent).unwrap();
    assert_eq!(outside.channel, None);
    assert!(
        peers(&outside).contains(&(site.first, Relation::Project)),
        "members of a channel with a project are agents of the project"
    );
    assert!(!peers(&outside)
        .iter()
        .any(|(id, _)| *id == site.coordinator));
    assert!(
        circle_of(&site.fixture.hub, site.subagent).is_none(),
        "subagents have no peers"
    );
}

#[tokio::test]
async fn members_of_a_channel_without_a_project_keep_to_the_channel() {
    let mut fixture = fixture("unused");
    let channel = create_channel(&fixture.hub, new_channel("site", None))
        .await
        .unwrap();
    let member = start(&mut fixture, channel.coordinator, "reviewer").await;
    let outsider = lock(&fixture.hub.registry).active().agent_id();

    let circle = circle_of(&fixture.hub, member).unwrap();
    assert_eq!(
        peers(&circle),
        [(channel.coordinator, Relation::Coordinator)]
    );
    let outside = circle_of(&fixture.hub, outsider).unwrap();
    assert!(peers(&outside).iter().all(|(id, _)| *id != member));
}
