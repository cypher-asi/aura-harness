//! Agents cloud agents spawned on their machines, listed here as a spawn
//! here is.

use super::spawns::adopt_machine_spawns;
use super::*;
use crate::event_loop::channels::{add_agent, create_channel};
use crate::event_loop::subagents::tests::{fixture, Fixture};
use crate::event_loop::tests::support::open_project;
use aura_agent_kernel::{ChildAgentSpec, KernelSpawnHook, SpawnHook};
use aura_core_types::{AgentPermissions, RecordEntry, Transaction};
use aura_store_db::{MemberRole, NewChannel, WriteStore};

/// Copy here the record of an agent `parent`'s machine spawned as `name`
/// in `role` and asked `prompt`, as the hub copies it; returns its id.
async fn spawned_on_machine(
    fixture: &Fixture,
    parent: AgentId,
    spawned_as: (&str, &str),
    prompt: &str,
) -> AgentId {
    spawned_described(fixture, parent, spawned_as, None, prompt).await
}

/// [`spawned_on_machine`], its `task` call having described it as
/// `description`.
async fn spawned_described(
    fixture: &Fixture,
    parent: AgentId,
    (name, role): (&str, &str),
    description: Option<&str>,
    prompt: &str,
) -> AgentId {
    let child = AgentId::generate();
    KernelSpawnHook::new(fixture.records.clone())
        .spawn_child(
            &parent,
            None,
            ChildAgentSpec {
                name: name.into(),
                role: role.into(),
                permissions: AgentPermissions::full_access(),
                tool_permissions: None,
                parent_tool_permissions: None,
                system_prompt_override: None,
                preassigned_agent_id: Some(child),
                initial_prompt: None,
                description: description.map(str::to_string),
            },
        )
        .await
        .unwrap();
    let asked = RecordEntry::builder(2, Transaction::user_prompt(child, prompt)).build();
    fixture
        .records
        .append_entry_direct(child, 2, &asked)
        .unwrap();
    child
}

/// A member of a new channel working in a new project, as a cloud
/// channel's member is before it moves to its machine; returns the
/// member, the channel's id and the project's.
async fn channel_member(fixture: &Fixture, dir: &Path) -> (AgentId, String, String) {
    let (project_id, _) = open_project(&fixture.hub, dir);
    let channel = create_channel(
        &fixture.hub,
        NewChannel {
            slug: "site".into(),
            title: "Website relaunch".into(),
            description: "Ship the new marketing site.".into(),
            project_id: Some(project_id.clone()),
            ..NewChannel::default()
        },
    )
    .await
    .unwrap();
    let member = add_agent(&fixture.hub, channel.coordinator, "reviewer")
        .unwrap()
        .agent_id();
    (member, channel.id, project_id)
}

/// A channel member whose machine spawned an explorer, which spawned a
/// scout, both registered from their copied records.
struct MachineTree {
    fixture: Fixture,
    _dir: tempfile::TempDir,
    member: AgentId,
    channel_id: String,
    project_id: String,
    explorer: AgentId,
    scout: AgentId,
}

async fn machine_tree() -> MachineTree {
    let fixture = fixture("unused");
    let dir = tempfile::TempDir::new().unwrap();
    let (member, channel_id, project_id) = channel_member(&fixture, dir.path()).await;
    let explorer = spawned_on_machine(
        &fixture,
        member,
        ("explore", "subagent:explore"),
        "Implement the engine core\nwith tests",
    )
    .await;
    let scout = spawned_on_machine(&fixture, explorer, ("scout", "scout"), "Look around").await;
    // The scout comes first, before the agent that spawned it is listed.
    assert!(adopt_machine_spawns(
        &fixture.hub,
        &[(member, scout), (member, explorer)]
    ));
    MachineTree {
        fixture,
        _dir: dir,
        member,
        channel_id,
        project_id,
        explorer,
        scout,
    }
}

#[tokio::test]
async fn agents_spawned_on_a_members_machine_are_its_subagents_here() {
    let tree = machine_tree().await;
    let registry = lock(&tree.fixture.hub.registry);
    let member_zns = registry.find(tree.member).unwrap().zns_id.clone();
    let explore = registry.find(tree.explorer).unwrap();
    assert_eq!(explore.zns_id, tree.explorer.spawn_zns(), "keeps its id");
    assert_eq!(explore.parent.as_deref(), Some(member_zns.as_str()));
    assert_eq!(
        explore.project_id.as_deref(),
        Some(tree.project_id.as_str())
    );
    assert_eq!(explore.definition.as_deref(), Some("explore"));
    assert_eq!(explore.name, "explore #1");
    let code = explore.code.clone().unwrap();
    assert_eq!(explore.display_name(), format!("{code}: Engine core"));
    let scout = registry.find(tree.scout).unwrap();
    assert_eq!(scout.name, "scout");
    assert_eq!(scout.parent.as_deref(), Some(explore.zns_id.as_str()));
    drop(registry);

    let spawns = [(tree.member, tree.explorer), (tree.member, tree.scout)];
    assert!(
        !adopt_machine_spawns(&tree.fixture.hub, &spawns),
        "a later copy lists nothing again"
    );
}

#[tokio::test]
async fn they_are_listed_under_it_in_its_channel_and_work_as_their_machine_says() {
    let MachineTree {
        fixture,
        member,
        channel_id,
        explorer,
        scout,
        ..
    } = machine_tree().await;
    let rows = fixture.hub.agent_summaries();
    let depth = |agent: AgentId| {
        rows.iter()
            .find(|row| row.id == agent.to_hex())
            .map(|row| row.depth)
    };
    assert_eq!(depth(explorer), depth(member).map(|d| d + 1));
    assert_eq!(depth(scout), depth(member).map(|d| d + 2));

    let members = fixture
        .hub
        .channels
        .as_ref()
        .unwrap()
        .members(&channel_id)
        .unwrap();
    let joined = |agent: AgentId| {
        members
            .iter()
            .find(|m| m.agent_id == agent)
            .map(|m| (m.role, m.parent))
    };
    assert_eq!(joined(explorer), Some((MemberRole::Subagent, Some(member))));
    assert_eq!(joined(scout), Some((MemberRole::Subagent, Some(explorer))));

    lock(&fixture.hub.machine_working).insert(scout);
    let running = |agent: AgentId| {
        fixture
            .hub
            .agent_summaries()
            .iter()
            .any(|row| row.id == agent.to_hex() && row.running)
    };
    assert!(running(scout));
    assert!(!running(explorer));
}

#[tokio::test]
async fn a_spawn_waits_for_its_record_and_its_spawner_to_be_here() {
    let fixture = fixture("unused");
    let dir = tempfile::TempDir::new().unwrap();
    let (member, _, _) = channel_member(&fixture, dir.path()).await;
    let orphan =
        spawned_on_machine(&fixture, AgentId::generate(), ("lost", "lost"), "Wander").await;
    let unread = AgentId::generate();

    assert!(!adopt_machine_spawns(
        &fixture.hub,
        &[(member, orphan), (member, unread)]
    ));
    let registry = lock(&fixture.hub.registry);
    assert!(registry.find(orphan).is_none(), "its spawner is not here");
    assert!(registry.find(unread).is_none(), "no record copied yet");
}

#[tokio::test]
async fn an_archived_spawn_stays_archived_when_its_machine_is_copied_again() {
    let fixture = fixture("unused");
    let dir = tempfile::TempDir::new().unwrap();
    let (member, _, _) = channel_member(&fixture, dir.path()).await;
    let explorer = spawned_on_machine(
        &fixture,
        member,
        ("explore", "subagent:explore"),
        "Map the router",
    )
    .await;
    assert!(adopt_machine_spawns(&fixture.hub, &[(member, explorer)]));
    lock(&fixture.hub.registry).archive(explorer).unwrap();

    assert!(!adopt_machine_spawns(&fixture.hub, &[(member, explorer)]));
    assert!(lock(&fixture.hub.registry).find(explorer).unwrap().archived);
}

#[tokio::test]
async fn a_spawn_is_titled_by_its_description_else_by_the_work_its_brief_names() {
    let fixture = fixture("unused");
    let dir = tempfile::TempDir::new().unwrap();
    let (member, _, _) = channel_member(&fixture, dir.path()).await;
    let brief = "In the Rust workspace at /workspace, find where subagent titles are set.";
    let explore = ("explore", "subagent:explore");
    let described = spawned_described(
        &fixture,
        member,
        explore,
        Some("Subagent title source"),
        brief,
    )
    .await;
    let undescribed = spawned_on_machine(&fixture, member, explore, brief).await;
    assert!(adopt_machine_spawns(
        &fixture.hub,
        &[(member, described), (member, undescribed)]
    ));

    let registry = lock(&fixture.hub.registry);
    let subtitle = |agent| registry.find(agent).unwrap().subtitle.clone();
    assert_eq!(
        subtitle(described).as_deref(),
        Some("Subagent title source")
    );
    assert_eq!(
        subtitle(undescribed).as_deref(),
        Some("Find where subagent titles")
    );
    let code = registry.find(described).unwrap().code.clone().unwrap();
    assert_eq!(
        registry.find(described).unwrap().display_name(),
        format!("{code}: Subagent title source")
    );
}
