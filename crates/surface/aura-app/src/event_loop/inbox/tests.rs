use super::*;
use crate::event_loop::channels::create_channel;
use crate::event_loop::subagents::tests::{fixture, request, Fixture};
use aura_store_db::NewChannel;
use std::collections::HashSet;
use std::path::Path;
use tokio::sync::mpsc::UnboundedReceiver;

fn new_channel(slug: &str, project_id: Option<String>) -> NewChannel {
    NewChannel {
        slug: slug.to_string(),
        title: "Website relaunch".to_string(),
        description: "Ship the new marketing site.".to_string(),
        project_id,
    }
}

/// What a coordinator asks for to start a new member from `agent`.
fn to_new(agent: &str, subject: &str, prompt: &str) -> AssignArgs {
    AssignArgs {
        to: None,
        agent: Some(agent.to_string()),
        subject: subject.to_string(),
        prompt: prompt.to_string(),
        todo_id: None,
    }
}

/// What a coordinator asks for to give work to the member `to`.
fn to_member(to: &str, prompt: &str) -> AssignArgs {
    AssignArgs {
        to: Some(to.to_string()),
        agent: None,
        subject: String::new(),
        prompt: prompt.to_string(),
        todo_id: None,
    }
}

fn code_of(fixture: &Fixture, agent: AgentId) -> String {
    lock(&fixture.hub.registry)
        .find(agent)
        .and_then(|a| a.code.clone())
        .unwrap()
}

/// Start a new member of the channel `coordinator` runs from `agent`;
/// returns it. Drains the UI commands it sent, so the fixture's small
/// command channel never fills.
async fn start(fixture: &mut Fixture, coordinator: AgentId, agent: &str) -> AgentId {
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
fn take_all(notices: &mut UnboundedReceiver<Notice>) -> Vec<Notice> {
    let mut out = Vec::new();
    while let Ok(notice) = notices.try_recv() {
        out.push(notice);
    }
    out
}

/// The conversation of an agent that saw `prompt` and said `said`.
fn conversation(prompt: &str, said: &str) -> Vec<Message> {
    vec![
        Message::user("earlier work"),
        Message::assistant("earlier answer"),
        Message::user(prompt),
        Message::assistant(said),
    ]
}

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

#[tokio::test]
async fn assigning_to_a_new_member_starts_a_top_level_agent_of_the_channel() {
    let mut fixture = fixture("unused");
    let mut notices = fixture.hub.take_notices();
    let mut mail = fixture.hub.open_mail();
    let channel = create_channel(&fixture.hub, new_channel("site", None))
        .await
        .unwrap();
    let coordinator = channel.coordinator;
    let before = lock(&fixture.hub.registry).agents().len();

    let args = AssignArgs {
        todo_id: Some("2".to_string()),
        ..to_new("reviewer", "Review the footer", "Check the footer links.")
    };
    let said = assign(&fixture.hub, coordinator, args).await.unwrap();
    fixture.drain_commands();

    let store = fixture.hub.channels.as_ref().unwrap();
    let members = store.members(&channel.id).unwrap();
    assert_eq!(members.len(), 2);
    let joined = members.iter().find(|m| m.agent_id != coordinator).unwrap();
    let member = joined.agent_id;
    assert_eq!(joined.role, MemberRole::Agent);
    assert_eq!(joined.parent, Some(coordinator));
    {
        let registry = lock(&fixture.hub.registry);
        assert_eq!(registry.agents().len(), before + 1);
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
            said,
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

    let lead = code_of(&fixture, coordinator);
    let [notice] = take_all(&mut notices).try_into().unwrap();
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
    assert_eq!(fixture.hub.inbox.owed_by(member), 1);

    let item = mail.try_recv().unwrap();
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
async fn coordinators_reach_their_members_and_members_reach_their_project_too() {
    let mut fixture = fixture("unused");
    let dir = tempfile::TempDir::new().unwrap();
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

    let lead = circle_of(&fixture.hub, coordinator).unwrap();
    assert!(lead.coordinates());
    assert_eq!(lead.slug(), "site");
    assert_eq!(
        peers(&lead),
        [(first, Relation::Member), (second, Relation::Member)],
        "only the channel's own agents, not its project's or subagents"
    );

    let member = circle_of(&fixture.hub, first).unwrap();
    assert!(!member.coordinates());
    assert_eq!(
        peers(&member),
        [
            (coordinator, Relation::Coordinator),
            (second, Relation::Member),
            (project_agent, Relation::Project),
        ]
    );
    let code = code_of(&fixture, second);
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
    assert!(member.find(&subagent.to_hex()).is_none());

    let outside = circle_of(&fixture.hub, project_agent).unwrap();
    assert_eq!(outside.channel, None);
    assert!(
        peers(&outside).contains(&(first, Relation::Project)),
        "members of a channel with a project are agents of the project"
    );
    assert!(!peers(&outside).iter().any(|(id, _)| *id == coordinator));
    assert!(
        circle_of(&fixture.hub, subagent).is_none(),
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

#[tokio::test]
async fn a_request_is_answered_with_the_last_words_of_the_turn_that_saw_it() {
    let mut fixture = fixture("unused");
    let mut notices = fixture.hub.take_notices();
    let channel = create_channel(&fixture.hub, new_channel("site", None))
        .await
        .unwrap();
    let first = start(&mut fixture, channel.coordinator, "reviewer").await;
    let second = start(&mut fixture, channel.coordinator, "tests").await;
    take_all(&mut notices);
    let asked_before = fixture.hub.inbox.owed_by(second);

    let circle = circle_of(&fixture.hub, first).unwrap();
    let peer = circle.find(&second.to_hex()).unwrap().clone();
    send(
        &fixture.hub,
        &circle,
        InboxKind::Request,
        &peer,
        Some("Parser"),
        "Which parser do the tests use?",
        None,
    )
    .unwrap();
    let [asked] = take_all(&mut notices).try_into().unwrap();
    assert_eq!(asked.inbox, Some(InboxKind::Request));
    assert!(asked.prompt.ends_with(&format!(
        "End your turn with your answer: it goes back to {} on its own.",
        code_of(&fixture, first)
    )));
    assert_eq!(fixture.hub.inbox.owed_by(second), asked_before + 1);

    // A turn that never saw it leaves it owed.
    answer_seen(
        &fixture.hub,
        second,
        &conversation("something else", "Done."),
    );
    assert_eq!(fixture.hub.inbox.owed_by(second), asked_before + 1);
    assert!(take_all(&mut notices).is_empty());

    answer_seen(
        &fixture.hub,
        second,
        &conversation(&asked.prompt, "The LR one, in src/parse.rs."),
    );
    assert_eq!(fixture.hub.inbox.owed_by(second), asked_before);
    let [reply] = take_all(&mut notices).try_into().unwrap();
    assert_eq!(reply.to, first);
    assert_eq!(reply.inbox, Some(InboxKind::Reply));
    let code = code_of(&fixture, second);
    assert!(
        reply
            .prompt
            .starts_with(&format!("{code} replied: Parser\n(inbox reply ")),
        "{}",
        reply.prompt
    );
    assert!(reply.prompt.ends_with("\n\nThe LR one, in src/parse.rs."));
    let row = peer_row(&fixture.hub, &peer);
    assert_eq!(row["last_report"]["text"], "The LR one, in src/parse.rs.");
    assert_eq!(row["last_report"]["finished"], true);

    // Replies are never answered.
    answer_seen(&fixture.hub, first, &conversation(&reply.prompt, "Thanks."));
    assert!(take_all(&mut notices).is_empty());
}

#[tokio::test]
async fn a_finished_assignment_reports_back_and_closes_the_coordinators_todo() {
    let mut fixture = fixture("unused");
    let mut notices = fixture.hub.take_notices();
    let channel = create_channel(&fixture.hub, new_channel("site", None))
        .await
        .unwrap();
    let coordinator = channel.coordinator;
    let member = start(&mut fixture, coordinator, "reviewer").await;
    take_all(&mut notices);
    let args = AssignArgs {
        to: Some(code_of(&fixture, member)),
        agent: None,
        subject: "Footer".to_string(),
        prompt: "Fix the footer links.".to_string(),
        todo_id: Some("4".to_string()),
    };
    assign(&fixture.hub, coordinator, args).await.unwrap();
    let [assigned] = take_all(&mut notices).try_into().unwrap();

    answer_seen(
        &fixture.hub,
        member,
        &conversation(&assigned.prompt, "Fixed: the links were relative."),
    );
    let [report] = take_all(&mut notices).try_into().unwrap();
    assert_eq!(report.to, coordinator);
    assert!(
        report.prompt.contains(" replied: Footer\n"),
        "{}",
        report.prompt
    );
    assert!(
        report.prompt.ends_with(&format!(
            "Fixed: the links were relative.\n{}",
            task_report_line("4", true)
        )),
        "{}",
        report.prompt
    );
    assert_eq!(
        fixture.hub.inbox.owed_by(member),
        1,
        "the kick-off it never saw is still owed"
    );
}

#[tokio::test]
async fn coordinators_owe_no_answers_and_are_told_how_to_give_one() {
    let mut fixture = fixture("unused");
    let mut notices = fixture.hub.take_notices();
    let channel = create_channel(&fixture.hub, new_channel("site", None))
        .await
        .unwrap();
    let coordinator = channel.coordinator;
    let member = start(&mut fixture, coordinator, "reviewer").await;
    take_all(&mut notices);

    let circle = circle_of(&fixture.hub, member).unwrap();
    let lead = circle.find("coordinator").unwrap().clone();
    send(
        &fixture.hub,
        &circle,
        InboxKind::Request,
        &lead,
        Some("Scope"),
        "Should the footer cover the blog too?",
        None,
    )
    .unwrap();
    let [asked] = take_all(&mut notices).try_into().unwrap();
    assert_eq!(asked.to, coordinator);
    assert!(asked.prompt.ends_with(&format!(
        "Answer {} with `send_to_agent` if it needs an answer.",
        code_of(&fixture, member)
    )));
    assert_eq!(fixture.hub.inbox.owed_by(coordinator), 0);
    answer_seen(
        &fixture.hub,
        coordinator,
        &conversation(&asked.prompt, "Yes."),
    );
    assert!(take_all(&mut notices).is_empty());
}

#[tokio::test]
async fn a_thread_carries_a_bounded_number_of_requests() {
    let mut fixture = fixture("unused");
    let mut notices = fixture.hub.take_notices();
    let channel = create_channel(&fixture.hub, new_channel("site", None))
        .await
        .unwrap();
    let first = start(&mut fixture, channel.coordinator, "reviewer").await;
    let second = start(&mut fixture, channel.coordinator, "tests").await;
    let circle = circle_of(&fixture.hub, first).unwrap();
    let peer = circle.find(&second.to_hex()).unwrap().clone();
    let ask = |subject: &str| {
        send(
            &fixture.hub,
            &circle,
            InboxKind::Request,
            &peer,
            Some(subject),
            "And now?",
            None,
        )
    };

    for _ in 0..THREAD_REQUESTS {
        ask("Parser").unwrap();
    }
    let error = ask("Parser").unwrap_err();
    assert!(error.contains("has carried 8 messages"), "{error}");
    assert!(
        error.contains(&format!(
            "take what is still open to {} in a short report",
            code_of(&fixture, channel.coordinator)
        )),
        "members are pointed at their coordinator: {error}"
    );
    ask("Another subject").unwrap();
    assert_eq!(
        take_all(&mut notices).len(),
        2 + THREAD_REQUESTS as usize + 1
    );
}

#[tokio::test]
async fn work_that_stops_tells_its_senders_no_answer_is_coming() {
    let mut fixture = fixture("unused");
    let mut notices = fixture.hub.take_notices();
    let channel = create_channel(&fixture.hub, new_channel("site", None))
        .await
        .unwrap();
    let coordinator = channel.coordinator;
    let member = start(&mut fixture, coordinator, "reviewer").await;
    let [kickoff] = take_all(&mut notices).try_into().unwrap();
    let code = code_of(&fixture, member);
    assign(
        &fixture.hub,
        coordinator,
        AssignArgs {
            todo_id: Some("7".to_string()),
            ..to_member(&code, "Then the header.")
        },
    )
    .await
    .unwrap();
    take_all(&mut notices);
    assert_eq!(fixture.hub.inbox.owed_by(member), 2);

    // With the conversation, only what it saw is dropped.
    give_up_seen(
        &fixture.hub,
        member,
        Some(&conversation(&kickoff.prompt, "")),
        "the model refused",
    );
    let [stopped] = take_all(&mut notices).try_into().unwrap();
    assert_eq!(stopped.to, coordinator);
    assert!(
        stopped
            .prompt
            .ends_with("\n\nStopped before finishing: the model refused"),
        "{}",
        stopped.prompt
    );
    assert_eq!(fixture.hub.inbox.owed_by(member), 1);

    // Without it, the oldest one ran.
    give_up_seen(&fixture.hub, member, None, "cancelled");
    let [stopped] = take_all(&mut notices).try_into().unwrap();
    assert!(
        stopped.prompt.ends_with(&format!(
            "Stopped before finishing: cancelled\n{}",
            task_report_line("7", false)
        )),
        "{}",
        stopped.prompt
    );
    assert_eq!(fixture.hub.inbox.owed_by(member), 0);
}

#[tokio::test]
async fn peer_hooks_reach_peers_and_refuse_strangers_without_a_host() {
    let mut fixture = fixture("unused");
    let mut notices = fixture.hub.take_notices();
    let channel = create_channel(&fixture.hub, new_channel("site", None))
        .await
        .unwrap();
    let coordinator = channel.coordinator;
    let member = start(&mut fixture, coordinator, "reviewer").await;
    take_all(&mut notices);
    let link = HubLink::default();
    link.set(&fixture.hub);
    let hooks = PeerHooks::new(link, member, None);

    hooks
        .deliver_message_in_thread(
            "coordinator",
            None,
            None,
            None,
            "The footer is done.",
            None,
            None,
            Some("Footer"),
        )
        .await
        .unwrap();
    let [told] = take_all(&mut notices).try_into().unwrap();
    assert_eq!(told.to, coordinator);
    assert!(told.prompt.contains(": Footer\n"), "{}", told.prompt);

    // Without a subject a message goes on with the last one, either way.
    let link = HubLink::default();
    link.set(&fixture.hub);
    PeerHooks::new(link, coordinator, None)
        .deliver_message(
            &code_of(&fixture, member),
            None,
            None,
            None,
            "Thanks.",
            None,
            None,
        )
        .await
        .unwrap();
    let [answered] = take_all(&mut notices).try_into().unwrap();
    assert_eq!(answered.to, member);
    assert!(
        answered
            .prompt
            .starts_with(&format!("{}: Footer\n", code_of(&fixture, coordinator))),
        "{}",
        answered.prompt
    );

    let error = hooks
        .deliver_message("NOPE9", None, None, None, "Hello", None, None)
        .await
        .unwrap_err();
    assert!(error.contains("unknown agent 'NOPE9'"), "{error}");
    assert!(hooks
        .lifecycle("coordinator", None, None, "pause")
        .await
        .is_err());

    let listed = hooks.list_agents(None).await.unwrap();
    let rows = listed.as_array().unwrap();
    assert_eq!(rows[0]["is_you"], true);
    assert_eq!(rows[0]["agent_id"], member.to_hex());
    assert_eq!(rows[1]["agent_id"], coordinator.to_hex());
    assert_eq!(rows[1]["relation"], "coordinator");
    assert_eq!(rows[1]["working"], false);

    let state = hooks
        .snapshot(&code_of(&fixture, coordinator))
        .await
        .unwrap();
    assert_eq!(state["relation"], "coordinator");
    assert!(hooks.snapshot("NOPE9").await.is_err());
}

#[test]
fn threads_are_the_same_whoever_writes_and_long_text_is_marked_when_cut() {
    let (a, b) = (AgentId::new([1; 32]), AgentId::new([2; 32]));
    assert_eq!(thread_id(a, b, "Parser").len(), "inbox:".len() + 16);
    assert_eq!(thread_id(a, b, "Parser"), thread_id(b, a, "Parser"));
    assert_ne!(thread_id(a, b, "Parser"), thread_id(a, b, "Lexer"));
    assert_eq!(capped("  short  ", 10), "short");
    assert_eq!(capped("ééééé", 3), "ééé… [cut at 3 characters]");
}
