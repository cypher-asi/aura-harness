//! A coordinator's `assign`: to a new member, to a member it has, and
//! what it refuses.

use super::super::*;
use super::support::{code_of, new_channel, start, take_all, to_member, to_new};
use crate::event_loop::channels::create_channel;
use crate::event_loop::mail::MailItem;
use crate::event_loop::subagents::tests::{fixture, Fixture};
use aura_store_db::MemberRole;
use tokio::sync::mpsc::UnboundedReceiver;

/// What assigning a review of the footer, for todo 2, to a new member
/// started from `reviewer` left in `#site`, which has no project.
struct NewMember {
    fixture: Fixture,
    channel: ChannelRecord,
    /// The channel's members after
    members: Vec<MemberRecord>,
    member: AgentId,
    /// How many agents the registry had before
    before: usize,
    /// What the coordinator was told
    said: String,
    notices: UnboundedReceiver<Notice>,
    mail: UnboundedReceiver<MailItem>,
}

async fn assign_to_a_new_member() -> NewMember {
    let mut fixture = fixture("unused");
    let notices = fixture.hub.take_notices();
    let mail = fixture.hub.open_mail();
    let channel = create_channel(&fixture.hub, new_channel("site", None))
        .await
        .unwrap();
    let before = lock(&fixture.hub.registry).agents().len();

    let args = AssignArgs {
        todo_id: Some("2".to_string()),
        ..to_new("reviewer", "Review the footer", "Check the footer links.")
    };
    let said = assign(&fixture.hub, channel.coordinator, args)
        .await
        .unwrap();
    fixture.drain_commands();

    let store = fixture.hub.channels.as_ref().unwrap();
    let members = store.members(&channel.id).unwrap();
    let member = members
        .iter()
        .find(|m| m.agent_id != channel.coordinator)
        .unwrap()
        .agent_id;
    NewMember {
        fixture,
        channel,
        members,
        member,
        before,
        said,
        notices,
        mail,
    }
}

#[tokio::test]
async fn assigning_to_a_new_member_starts_a_top_level_agent_of_the_channel() {
    let assigned = assign_to_a_new_member().await;
    let (fixture, coordinator, member) = (
        &assigned.fixture,
        assigned.channel.coordinator,
        assigned.member,
    );
    assert_eq!(assigned.members.len(), 2);
    let joined = assigned
        .members
        .iter()
        .find(|m| m.agent_id != coordinator)
        .unwrap();
    assert_eq!(joined.role, MemberRole::Agent);
    assert_eq!(joined.parent, Some(coordinator));
    {
        let registry = lock(&fixture.hub.registry);
        assert_eq!(registry.agents().len(), assigned.before + 1);
        let agent = registry.find(member).unwrap();
        let lead = registry.find(coordinator).unwrap();
        assert_eq!(agent.parent, None, "a member is nobody's subagent");
        assert!(registry.is_top_level_agent(member));
        assert_eq!(agent.definition.as_deref(), Some("reviewer"));
        assert_eq!(agent.project_id, lead.project_id);
        assert_eq!(agent.folder, lead.folder, "it works where the channel does");
        let rows = registry.store().load().unwrap().agents;
        let row = rows.iter().find(|row| row.agent_id == member).unwrap();
        assert_eq!(row.parent, None);
        assert_eq!(row.code, agent.code);
        assert_eq!(
            assigned.said,
            format!(
                "Assigned to {} (new member). Its report will arrive in your inbox.",
                Named::of(agent).full()
            )
        );
    }
    assert!(lock(&fixture.hub.channel_members).contains(&member));
    assert!(
        lock(&fixture.hub.hidden_members).contains(&member),
        "a channel without a project hides its members from the Projects tree"
    );
}

#[tokio::test]
async fn a_new_members_assignment_reaches_it_and_the_inbox_tab() {
    let mut assigned = assign_to_a_new_member().await;
    let (coordinator, member) = (assigned.channel.coordinator, assigned.member);
    let lead = code_of(&assigned.fixture, coordinator);
    let [notice] = take_all(&mut assigned.notices).try_into().unwrap();
    assert_eq!(notice.to, member);
    assert_eq!(notice.inbox, Some(InboxKind::Assignment));
    assert_eq!(notice.shown, format!("✉ {lead}: Review the footer"));
    assert!(
        notice
            .prompt
            .starts_with(&format!("{lead}: Review the footer\n(inbox assignment ")),
        "{}",
        notice.prompt
    );
    assert!(notice.prompt.contains("\n\nCheck the footer links.\n\n"));
    assert!(
        notice.prompt.ends_with(&format!(
            "When it is done, end your turn with a short report: it goes back to {lead} on its own."
        )),
        "{}",
        notice.prompt
    );
    assert_eq!(assigned.fixture.hub.inbox.owed_by(member), 1);

    let item = assigned.mail.try_recv().unwrap();
    assert_eq!(item.view.subject, "Review the footer");
    assert_eq!(item.view.from_id, coordinator.to_hex());
    assert_eq!(item.view.to_id, member.to_hex());
    assert!(item.view.thread_id.starts_with("inbox:"));
    assert!(!item.view.reply);
}

#[tokio::test]
async fn assigning_to_a_member_by_code_reuses_it() {
    let mut fixture = fixture("unused");
    let mut notices = fixture.hub.take_notices();
    let channel = create_channel(&fixture.hub, new_channel("site", None))
        .await
        .unwrap();
    let coordinator = channel.coordinator;
    let member = start(&mut fixture, coordinator, "reviewer").await;
    take_all(&mut notices);
    let before = lock(&fixture.hub.registry).agents().len();

    let code = code_of(&fixture, member);
    let said = assign(
        &fixture.hub,
        coordinator,
        to_member(&format!("@{}", code.to_lowercase()), "Now the header."),
    )
    .await
    .unwrap();
    assert!(said.contains("(starting now)"), "{said}");
    assert_eq!(lock(&fixture.hub.registry).agents().len(), before);
    let [notice] = take_all(&mut notices).try_into().unwrap();
    assert_eq!(notice.to, member);
    assert!(
        notice.prompt.starts_with(&format!(
            "{}: Now the header.\n",
            code_of(&fixture, coordinator)
        )),
        "the subject defaults to the brief's first line: {}",
        notice.prompt
    );
    assert_eq!(fixture.hub.inbox.owed_by(member), 2);

    let error = assign(&fixture.hub, coordinator, to_member("NOPE9", "Anything."))
        .await
        .unwrap_err();
    assert!(error.contains("#site has no member `NOPE9`"), "{error}");
    assert!(error.contains(&code), "the members are listed: {error}");
}

#[tokio::test]
async fn only_a_typed_prompt_titles_an_untitled_member() {
    let fixture = fixture("unused");
    let _notices = fixture.hub.take_notices();
    let channel = create_channel(&fixture.hub, new_channel("site", None))
        .await
        .unwrap();
    let coordinator = channel.coordinator;
    let untitled = channels::add_agent(&fixture.hub, coordinator, "reviewer")
        .unwrap()
        .agent_id();
    assert!(
        fixture.hub.titles_turn(untitled, true),
        "a typed prompt titles an untitled member"
    );
    assert!(
        !fixture.hub.titles_turn(untitled, false),
        "a notice doesn't"
    );
    assert!(!fixture.hub.titles_turn(coordinator, true));
}

#[tokio::test]
async fn each_assignment_titles_its_member_with_what_it_is_on() {
    let mut fixture = fixture("unused");
    let _notices = fixture.hub.take_notices();
    let channel = create_channel(&fixture.hub, new_channel("site", None))
        .await
        .unwrap();
    let coordinator = channel.coordinator;
    let first = start(&mut fixture, coordinator, "reviewer").await;
    let second = start(&mut fixture, coordinator, "reviewer").await;
    let title = |agent: AgentId| {
        let registry = lock(&fixture.hub.registry);
        let agent = registry.find(agent).unwrap();
        (agent.title.clone(), agent.display_name().into_owned())
    };
    let kick_off = (Some("Kick-off".to_string()), "Kick-off".to_string());
    assert_eq!(title(first), kick_off, "the subject titles a new member");
    assert_eq!(title(second), kick_off);
    assert_ne!(first, second, "each `agent` assignment starts an instance");

    let code = code_of(&fixture, first);
    let args = AssignArgs {
        subject: "  \"Review the header.\" ".to_string(),
        ..to_member(&code, "Check the header links.")
    };
    assign(&fixture.hub, coordinator, args).await.unwrap();
    let header = "Review the header".to_string();
    assert_eq!(title(first), (Some(header.clone()), header));
    assert_eq!(title(second), kick_off, "instances keep their own titles");

    assign(
        &fixture.hub,
        coordinator,
        to_member(&code, "\n  Now the   footer links\nand the icons."),
    )
    .await
    .unwrap();
    let footer = "Now the footer links".to_string();
    assert_eq!(
        title(first),
        (Some(footer.clone()), footer),
        "without a subject, the brief's first line titles it"
    );
    fixture.drain_commands();
    let registry = lock(&fixture.hub.registry);
    assert_eq!(registry.find(coordinator).unwrap().title, None);
    assert_eq!(
        registry.find(first).unwrap().definition.as_deref(),
        Some("reviewer"),
        "the title is the instance's, not the permanent agent's"
    );
}

#[tokio::test]
async fn only_a_coordinator_assigns_and_only_work_to_real_members() {
    let mut fixture = fixture("unused");
    let channel = create_channel(&fixture.hub, new_channel("site", None))
        .await
        .unwrap();
    let coordinator = channel.coordinator;
    let member = start(&mut fixture, coordinator, "reviewer").await;
    let outsider = lock(&fixture.hub.registry).active().agent_id();

    for caller in [outsider, member] {
        let error = assign(&fixture.hub, caller, to_new("reviewer", "", "Do it."))
            .await
            .unwrap_err();
        assert!(error.contains("only a channel's coordinator"), "{error}");
    }
    let refusals = [
        (
            to_new("coordinator", "", "Run it."),
            "coordinators only run channels",
        ),
        (
            to_new("nobody", "", "Do it."),
            "no permanent agent `nobody`",
        ),
        (to_new("reviewer", "", "   "), "the prompt is empty"),
        (
            to_member("coordinator", "Do it yourself."),
            "has no member `coordinator`",
        ),
        (
            AssignArgs {
                agent: None,
                ..to_new("reviewer", "", "Do it.")
            },
            "name a member in `to`",
        ),
    ];
    let before = lock(&fixture.hub.registry).agents().len();
    for (args, expected) in refusals {
        let error = assign(&fixture.hub, coordinator, args).await.unwrap_err();
        assert!(error.contains(expected), "{expected}: {error}");
    }
    assert_eq!(lock(&fixture.hub.registry).agents().len(), before);
}

#[tokio::test]
async fn new_members_of_a_cloud_channel_wait_to_start_in_the_cloud() {
    use aura_store_db::{ChannelPlacement, NewChannel};
    let mut fixture = fixture("unused");
    let in_cloud = NewChannel {
        placement: ChannelPlacement::Cloud,
        ..new_channel("site", None)
    };
    let cloud = create_channel(&fixture.hub, in_cloud).await.unwrap();
    let local = create_channel(&fixture.hub, new_channel("docs", None))
        .await
        .unwrap();
    assert!(
        lock(&fixture.hub.cloud_starts).is_empty(),
        "coordinators stay here"
    );

    let first = start(&mut fixture, cloud.coordinator, "reviewer").await;
    let second = start(&mut fixture, cloud.coordinator, "reviewer").await;
    let here = start(&mut fixture, local.coordinator, "reviewer").await;
    let waiting = lock(&fixture.hub.cloud_starts).clone();
    assert_eq!(waiting, [first, second].into_iter().collect());
    assert!(!waiting.contains(&here));

    // Work for a member it has starts it nowhere new.
    let code = code_of(&fixture, first);
    lock(&fixture.hub.cloud_starts).clear();
    assign(
        &fixture.hub,
        cloud.coordinator,
        to_member(&code, "Then the header."),
    )
    .await
    .unwrap();
    fixture.drain_commands();
    assert!(lock(&fixture.hub.cloud_starts).is_empty());
}
