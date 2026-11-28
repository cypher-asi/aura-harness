//! Restarts that cut work short: after the app reopens, members carry on
//! on their own and the coordinator gets its reports, here and from cloud
//! machines; what is queued comes back whatever it is.

use super::inbox::{assign_footer, assign_footer_asking, channel_with_member, code, SLOW_FOOTER};
use super::inbox_restart::restart;
use super::*;
use crate::cloud::RemoteEvent;
use crate::event_loop::cloud::follow::{follow_events, mark_running};
use crate::event_loop::inbox::{self as owed, HubLink, PeerHooks, THREAD_REQUESTS};
use crate::event_loop::subagents::lock;
use crate::event_loop::turns::{Prompt, Work};
use aura_agent::checkpoint::{SessionCheckpoint, SessionUsage};
use aura_exec_tools::todo::task_report_line;
use aura_exec_tools::{AgentControlHook, AgentMessage};
use aura_model::{ContentBlock, Message, Role};
use aura_ui::events::ImageAttachment;
use tokio::sync::mpsc::UnboundedReceiver;

/// The saved file of `state`'s hub, as text.
fn saved_text(state: &LoopState<'_>) -> String {
    let path = lock(&state.hub.registry).data_dir().join(owed::SAVED_FILE);
    std::fs::read_to_string(path).unwrap_or_default()
}

/// The next notice: `to`'s report, closing todo 3, `ok` or not.
fn assert_report(notices: &mut UnboundedReceiver<notices::Notice>, to: AgentId, ok: bool) {
    let report = notices.try_recv().unwrap();
    assert_eq!(report.to, to, "{}", report.prompt);
    assert!(
        report.prompt.ends_with(&task_report_line("3", ok)),
        "{}",
        report.prompt
    );
}

/// Quit as the app does with `agent` mid-turn: the last save, then its
/// turn cancelled and its end never handled. The new process runs
/// nothing and holds nothing; `agent`'s session reopens from its record.
async fn quit_mid_turn(
    state: &mut LoopState<'_>,
    done: &mut mpsc::Receiver<WorkDone>,
    notices: &mut UnboundedReceiver<notices::Notice>,
    agent: AgentId,
) {
    owed::flush_saved(state).await;
    state.hub.cancel_all();
    let cut = next_done(done).await;
    state.hub.stop_running(agent);
    if let Some(session) = cut.session {
        state.idle.insert(agent, session);
    }
    while notices.try_recv().is_ok() {}
    state.hub.inbox.forget_all();
    state.queued.clear();
}

#[tokio::test]
async fn a_member_cut_short_by_a_quit_carries_on_after_reopening_and_reports() {
    let (fixture, _commands) = fixture();
    let coordinator = channel_with_member(&fixture).await;
    let member = fixture.second;
    let mut notices = fixture.hub.take_notices();
    let mut b = borrowed(&fixture);
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    owed::restore_saved(&mut state).await;
    assign_footer_asking(&fixture, coordinator, &code(&fixture, member), SLOW_FOOTER).await;
    notices::handle_notice(&mut state, notices.try_recv().unwrap()).await;
    assert!(state.hub.is_running(member), "the assignment is under way");
    // Out of the software factory's flow, whose own restart is tested in
    // `factory_restart`: here the turn itself resumes.
    state.hub.factory_flows.leave(member);
    owed::save_now(&state).await;
    assert!(
        saved_text(&state).contains(&format!("\"running\":[{{\"agent\":\"{}\"", member.to_hex())),
        "{}",
        saved_text(&state)
    );

    quit_mid_turn(&mut state, &mut done, &mut notices, member).await;
    // The resumed turn carries on the slow prompt, so the mock holds it.
    fixture.gate.add_permits(1);
    let resume = owed::restore_saved(&mut state).await;
    assert_eq!(
        state.hub.inbox.owed_by(member),
        1,
        "its report is still owed"
    );
    owed::resume_saved(&mut state, resume).await;

    assert!(state.hub.is_running(member), "it carries on on its own");
    let finished = next_done(&mut done).await;
    assert_eq!(finished.agent_id, member);
    handle_work_done(&mut state, finished).await;
    let asked = user_texts(&state, member);
    assert!(
        asked
            .last()
            .is_some_and(|text| text.contains("The app restarted")),
        "{asked:?}"
    );
    assert_eq!(state.hub.inbox.owed_by(member), 0);
    assert_report(&mut notices, coordinator, true);
}

#[tokio::test]
async fn the_users_own_chat_cut_short_is_told_so_not_restarted_and_parked_members_wait() {
    let (fixture, mut commands) = fixture();
    let coordinator = channel_with_member(&fixture).await;
    let (user, member) = (fixture.first, fixture.second);
    let mut notices = fixture.hub.take_notices();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    owed::restore_saved(&mut state).await;
    // The member owes a report but the user stopped it to carry on.
    assign_footer(&fixture, coordinator, &code(&fixture, member)).await;
    state.hub.inbox.park(member);
    handle_ui_event(&mut state, message(user, "slow start", true), &links).await;
    assert!(state.hub.is_running(user));

    quit_mid_turn(&mut state, &mut done, &mut notices, user).await;
    drain(&mut commands);
    let resume = owed::restore_saved(&mut state).await;
    owed::resume_saved(&mut state, resume).await;

    assert!(
        !state.hub.is_running(user),
        "the user's chat waits for the user"
    );
    let said = shown(&mut commands);
    assert!(
        said.iter()
            .any(|text| text.contains("cut short when the app quit")),
        "{said:?}"
    );
    assert!(!state.hub.is_running(member), "parked: left alone");
    assert_eq!(state.hub.inbox.owed_by(member), 1);
    assert!(state.hub.inbox.owing().is_empty(), "still parked");
}

#[tokio::test]
async fn an_idle_member_owing_a_report_in_a_version_1_file_carries_on_at_once() {
    let (fixture, _commands) = fixture();
    let coordinator = channel_with_member(&fixture).await;
    let member = fixture.second;
    let mut notices = fixture.hub.take_notices();
    let mut b = borrowed(&fixture);
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    let path = lock(&state.hub.registry).data_dir().join(owed::SAVED_FILE);
    let v1 = serde_json::json!({
        "version": 1,
        "owed": [{
            "agent": member.to_hex(), "id": "0123456789ab", "kind": "assignment",
            "from": coordinator.to_hex(), "thread": "inbox:0123456789abcdef",
            "subject": "Footer", "todo": "3", "withdrawn": false
        }],
        "queued": [],
        "notices": []
    });
    std::fs::write(&path, v1.to_string()).unwrap();

    let resume = owed::restore_saved(&mut state).await;
    owed::resume_saved(&mut state, resume).await;
    assert!(
        state.hub.is_running(member),
        "at once, not after two sweeps"
    );
    let finished = next_done(&mut done).await;
    handle_work_done(&mut state, finished).await;
    assert_eq!(state.hub.inbox.owed_by(member), 0);
    assert_report(&mut notices, coordinator, true);
    owed::save_now(&state).await;
    assert!(saved_text(&state).contains("\"version\":2"), "saved anew");
}

/// A message with `small` and an image too large to keep, a command, a
/// `/build` and a `/compact`.
fn every_kind(small: &ImageAttachment) -> [Work; 4] {
    let huge = ImageAttachment {
        media_type: "image/png".into(),
        data: "a".repeat(5 * 1024 * 1024),
        bytes: 1,
    };
    [
        Work::Prompt {
            prompt: Prompt::typed("Look at these".into(), vec![small.clone(), huge]),
            shown: true,
        },
        Work::Command {
            command: "echo hi".into(),
            cols: 80,
        },
        Work::Build {
            path: Some("plans/site.md".into()),
        },
        Work::Compact { focus: None },
    ]
}

/// What `commands` showed as messages.
fn shown(commands: &mut mpsc::Receiver<UiCommand>) -> Vec<String> {
    drain(commands)
        .into_iter()
        .filter_map(|(_, command)| match command {
            UiCommand::ShowMessage(message) => Some(message.content),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn every_kind_of_queued_work_comes_back_and_too_large_images_are_said() {
    let (fixture, mut commands) = fixture();
    let agent = fixture.second;
    let mut b = borrowed(&fixture);
    let mut state = loop_state(&fixture, &mut b);
    owed::restore_saved(&mut state).await;
    let small = ImageAttachment {
        media_type: "image/png".into(),
        data: "iVBORw0KGgo=".into(),
        bytes: 8,
    };
    for work in every_kind(&small) {
        state.queued.push_back(agent, work);
    }
    let mut notices = fixture.hub.take_notices();

    let resume = restart(&mut state, &mut notices).await;
    let texts: Vec<String> = state
        .queued
        .views(agent)
        .into_iter()
        .map(|v| v.text)
        .collect();
    assert_eq!(
        texts,
        [
            "Look at these",
            "! echo hi",
            "/build plans/site.md",
            "/compact"
        ]
    );
    let Some((_, Work::Prompt { prompt, .. })) = state.queued.items().into_iter().next() else {
        panic!("the message comes first");
    };
    assert_eq!(prompt.images, [small], "the small image is kept");
    assert!(prompt.mentions, "typed: its mentions attach");

    drain(&mut commands);
    owed::resume_saved(&mut state, resume).await;
    let said = shown(&mut commands);
    assert!(
        said.iter().any(|text| text.contains("without 1 image(s)")),
        "{said:?}"
    );
}

#[tokio::test]
async fn mail_assigned_just_before_a_crash_is_delivered_after_it() {
    let (fixture, _commands) = fixture();
    let coordinator = channel_with_member(&fixture).await;
    let member = fixture.second;
    let mut notices = fixture.hub.take_notices();
    let mut b = borrowed(&fixture);
    let mut state = loop_state(&fixture, &mut b);
    owed::restore_saved(&mut state).await;
    assign_footer(&fixture, coordinator, &code(&fixture, member)).await;
    // The app dies before the loop handled the assignment, or saved.
    while notices.try_recv().is_ok() {}
    state.hub.inbox.forget_all();

    owed::restore_saved(&mut state).await;
    let again = notices.try_recv().unwrap();
    assert_eq!(again.to, member);
    assert!(again.prompt.contains("Make the footer links work."));
    assert_eq!(state.hub.inbox.owed_by(member), 1);
}

#[tokio::test]
async fn a_full_thread_is_still_full_after_a_restart() {
    let (fixture, _commands) = fixture();
    let coordinator = channel_with_member(&fixture).await;
    let member = fixture.second;
    let mut notices = fixture.hub.take_notices();
    let mut b = borrowed(&fixture);
    let mut state = loop_state(&fixture, &mut b);
    owed::restore_saved(&mut state).await;
    let lead = code(&fixture, coordinator);
    let link = HubLink::default();
    link.set(&fixture.hub);
    let hooks = PeerHooks::new(link, member, None);
    let ask = || hooks.deliver_message(AgentMessage::new(&lead, "And now?"));
    for _ in 0..THREAD_REQUESTS {
        ask().await.unwrap();
    }

    restart(&mut state, &mut notices).await;
    let error = ask().await.unwrap_err();
    assert!(error.contains("has carried 8 messages"), "{error}");
}

/// A cloud member owing the footer, its turn followed again after a
/// restart: the conversation its machine left is `messages`, and its
/// stream sends `events`. Returns the notices sent meanwhile.
async fn followed(
    messages: Vec<Message>,
    events: Vec<RemoteEvent>,
) -> (AgentId, Vec<notices::Notice>, usize) {
    let (fixture, _commands) = fixture();
    let coordinator = channel_with_member(&fixture).await;
    let member = fixture.second;
    let mut notices = fixture.hub.take_notices();
    let mut b = borrowed(&fixture);
    let mut state = loop_state(&fixture, &mut b);
    assign_footer(&fixture, coordinator, &code(&fixture, member)).await;
    let assignment = notices.try_recv().unwrap();
    let mut conversation = vec![Message::user(assignment.prompt.clone())];
    conversation.extend(messages);
    let checkpoint =
        SessionCheckpoint::new(1_000, 0, conversation, Vec::new(), SessionUsage::default());
    let sessions = state.hub.factory.checkpoints.clone().unwrap();
    sessions.save(member, &checkpoint).await.unwrap();

    let mut ended = state.cloud.take_ended();
    let (tx, rx) = mpsc::channel(16);
    follow_events(&state, member, rx, "Carry on: the app restarted.".into()).await;
    for event in events {
        tx.send(event).await.unwrap();
    }
    drop(tx);
    let end = tokio::time::timeout(Duration::from_secs(10), ended.recv())
        .await
        .unwrap()
        .unwrap();
    crate::event_loop::cloud::handle_ended(&mut state, end).await;
    let mut sent = Vec::new();
    while let Ok(notice) = notices.try_recv() {
        sent.push(notice);
    }
    (coordinator, sent, state.hub.inbox.owed_by(member))
}

#[tokio::test]
async fn a_cloud_turn_followed_after_a_restart_reports_when_it_ends() {
    let reply = vec![Message::assistant("Footer fixed.".to_string())];
    let done = RemoteEvent::Done(Box::default());
    let (coordinator, sent, owed) =
        followed(reply, vec![RemoteEvent::Text("Footer fixed.".into()), done]).await;
    assert_eq!(owed, 0);
    let [report] = sent.try_into().unwrap();
    assert_eq!(report.to, coordinator);
    assert!(report.prompt.contains("Footer fixed."));
    assert!(report.prompt.ends_with(&task_report_line("3", true)));
}

/// How the cloud hub's follow ends a turn that ended while no app
/// watched it and left no reply ([`crate::cloud::CloudHub::follow_turn`]).
const ENDED_UNSEEN: &str = "it ended while no app watched it, without a reply";

#[tokio::test]
async fn a_cloud_turn_that_ended_unseen_reports_if_it_replied_else_carries_on() {
    // A reply in its record: the follow ends it as done.
    let finished = vec![Message::assistant("Footer fixed.".to_string())];
    let (coordinator, sent, owed) =
        followed(finished, vec![RemoteEvent::Done(Box::default())]).await;
    assert_eq!(owed, 0);
    let [report] = sent.try_into().unwrap();
    assert_eq!(report.to, coordinator);
    assert!(
        report.prompt.ends_with(&task_report_line("3", true)),
        "{}",
        report.prompt
    );

    let mid_tool = vec![Message::new(
        Role::Assistant,
        vec![ContentBlock::ToolUse {
            id: "t1".into(),
            name: "read_file".into(),
            input: serde_json::json!({ "path": "footer.html" }),
        }],
    )];
    let (_, sent, owed) = followed(mid_tool, vec![RemoteEvent::Failed(ENDED_UNSEEN.into())]).await;
    assert_eq!(owed, 1, "not given up");
    assert!(
        !sent.iter().any(|n| n.inbox.is_some()),
        "no report: {:?}",
        sent.iter().map(|n| &n.prompt).collect::<Vec<_>>()
    );
    assert!(
        sent.iter()
            .any(|n| n.inbox.is_none() && n.prompt == "Carry on: the app restarted."),
        "{:?}",
        sent.iter().map(|n| &n.prompt).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn a_cloud_turn_whose_stream_dropped_is_followed_once_and_reports_once_after_reopening() {
    let (fixture, _commands) = fixture();
    let coordinator = channel_with_member(&fixture).await;
    let member = fixture.second;
    let mut notices = fixture.hub.take_notices();
    let mut b = borrowed(&fixture);
    let mut state = loop_state(&fixture, &mut b);
    owed::restore_saved(&mut state).await;
    assign_footer(&fixture, coordinator, &code(&fixture, member)).await;
    let assignment = notices.try_recv().unwrap();
    owed::delivered(&state.hub, member, &assignment.prompt);
    let reply = vec![
        Message::user(assignment.prompt.clone()),
        Message::assistant("Footer fixed.".to_string()),
    ];
    let checkpoint = SessionCheckpoint::new(1_000, 0, reply, Vec::new(), SessionUsage::default());
    let sessions = state.hub.factory.checkpoints.clone().unwrap();
    sessions.save(member, &checkpoint).await.unwrap();

    // Its cloud turn runs as the app quits; the new process holds none.
    mark_running(&state, member, true);
    let resume = restart(&mut state, &mut notices).await;
    mark_running(&state, member, false);
    assert_eq!(state.hub.inbox.owed_by(member), 1);

    // The cloud hub's follow takes the turn up as the hub comes up,
    // before the resume step runs.
    let mut ended = state.cloud.take_ended();
    let (tx, rx) = mpsc::channel(16);
    follow_events(&state, member, rx, "Carry on: the app restarted.".into()).await;
    owed::resume_saved(&mut state, resume).await;
    assert!(!state.hub.is_running(member), "not run a second time here");
    assert!(notices.try_recv().is_err(), "not asked to carry on");

    // Its stream dropped and was attached to again, then it ends.
    for event in [
        RemoteEvent::Status("Reconnecting to the cloud machine".into()),
        RemoteEvent::Copied,
        RemoteEvent::Done(Box::default()),
    ] {
        tx.send(event).await.unwrap();
    }
    drop(tx);
    let end = tokio::time::timeout(Duration::from_secs(10), ended.recv())
        .await
        .unwrap()
        .unwrap();
    crate::event_loop::cloud::handle_ended(&mut state, end).await;

    let sent: Vec<notices::Notice> = std::iter::from_fn(|| notices.try_recv().ok()).collect();
    let [report] = sent.try_into().unwrap();
    assert_eq!(report.to, coordinator);
    assert!(
        report.prompt.ends_with(&task_report_line("3", true)),
        "{}",
        report.prompt
    );
    assert_eq!(state.hub.inbox.owed_by(member), 0);
}
