//! Agents talking through their inboxes on the event loop: work a
//! coordinator assigns runs as the member's task, and the member's report
//! comes back to the coordinator.

use super::*;
use crate::event_loop::inbox::{assign, AssignArgs};
use aura_exec_tools::todo::task_report_line;
use aura_store_db::NewChannel;

/// A channel `#site` with the fixture's second agent as a member, as
/// a member joins: top-level, the coordinator having brought it in.
/// Returns the coordinator.
pub(super) async fn channel_with_member(fixture: &Fixture) -> AgentId {
    let channel = channels::create_channel(
        &fixture.hub,
        NewChannel {
            slug: "site".to_string(),
            title: "Website relaunch".to_string(),
            description: String::new(),
            ..NewChannel::default()
        },
    )
    .await
    .unwrap();
    let member = lock_registry_agent(fixture, fixture.second);
    channels::join_parent_channel(&fixture.hub, channel.coordinator, &member).unwrap();
    channel.coordinator
}

pub(super) fn code(fixture: &Fixture, agent: AgentId) -> String {
    lock_registry_agent(fixture, agent).code.unwrap()
}

#[tokio::test]
async fn an_assignment_runs_as_the_members_task_and_its_report_reaches_the_coordinator() {
    let (fixture, mut commands) = fixture();
    let coordinator = channel_with_member(&fixture).await;
    let member = fixture.second;
    let (lead, worker) = (code(&fixture, coordinator), code(&fixture, member));
    let mut notices = fixture.hub.take_notices();
    let mut b = borrowed(&fixture);
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);

    let said = assign_footer(&fixture, coordinator, &worker).await;
    assert!(
        said.starts_with(&format!("Assigned to {worker} (")),
        "{said}"
    );

    let assignment = notices.try_recv().unwrap();
    run_notice(&mut state, &mut done, assignment, member).await;
    let asked = only_text(user_texts(&state, member));
    assert!(
        asked.starts_with(&format!("{lead}: Footer\n(inbox assignment ")),
        "{asked}"
    );
    assert!(asked.contains("\n\nMake the footer links work.\n\n"));
    assert_eq!(state.hub.inbox.owed_by(member), 0);

    let report = notices.try_recv().unwrap();
    assert_eq!(report.to, coordinator);
    run_notice(&mut state, &mut done, report, coordinator).await;
    let told = only_text(user_texts(&state, coordinator));
    assert!(
        told.starts_with(&format!("{worker} replied: Footer\n(inbox reply ")),
        "{told}"
    );
    let report_line = task_report_line("3", true);
    assert!(
        told.ends_with(&format!("\n\nDone.\n{report_line}")),
        "{told}"
    );
    assert!(
        notices.try_recv().is_err(),
        "a coordinator answers nobody on its own"
    );

    // Each chat shows its message as a prompt, which it folds.
    assert_prompts_fold(
        &mut commands,
        [
            (member, format!("{lead}: Footer")),
            (coordinator, format!("{worker} replied: Footer")),
        ],
    );
}

/// The coordinator assigns the member whose code is `worker` the footer
/// links, as todo 3, and says to whom the work went.
pub(super) async fn assign_footer(fixture: &Fixture, coordinator: AgentId, worker: &str) -> String {
    assign_footer_asking(fixture, coordinator, worker, "Make the footer links work.").await
}

/// [`assign_footer`], asking `prompt`.
pub(super) async fn assign_footer_asking(
    fixture: &Fixture,
    coordinator: AgentId,
    worker: &str,
    prompt: &str,
) -> String {
    assign(
        &fixture.hub,
        coordinator,
        AssignArgs {
            to: Some(worker.to_lowercase()),
            agent: None,
            subject: "Footer".to_string(),
            prompt: prompt.to_string(),
            todo_id: Some("3".to_string()),
            withdraw: false,
        },
    )
    .await
    .unwrap()
}

/// Hand `notice` to the loop and finish the turn it starts, which must be
/// `agent`'s.
async fn run_notice(
    state: &mut LoopState<'_>,
    done: &mut mpsc::Receiver<WorkDone>,
    notice: notices::Notice,
    agent: AgentId,
) {
    notices::handle_notice(state, notice).await;
    let finished = next_done(done).await;
    assert_eq!(finished.agent_id, agent);
    handle_work_done(state, finished).await;
}

#[tokio::test]
async fn a_cloud_members_turn_answers_its_assignment_when_the_loop_hears_it_ended() {
    let report_of = |failure: Option<&str>, said: &str| {
        let failure = failure.map(str::to_string);
        let failure_given = failure.is_some();
        let said = said.to_string();
        async move {
            let (fixture, _commands) = fixture();
            let coordinator = channel_with_member(&fixture).await;
            let member = fixture.second;
            let mut notices = fixture.hub.take_notices();
            let mut b = borrowed(&fixture);
            let mut state = loop_state(&fixture, &mut b);
            assign_footer(&fixture, coordinator, &code(&fixture, member)).await;
            let assignment = notices.try_recv().unwrap();
            assert_eq!(state.hub.inbox.owed_by(member), 1);

            // Its machine ran the assignment; the hub copied the
            // conversation back as the turn ended.
            subagents::lock(&state.hub.cloud_turns).insert(member);
            assert!(state.hub.is_working(member), "working, on its machine");
            let messages = vec![
                aura_model::Message::user(&assignment.prompt),
                aura_model::Message::assistant(&said),
            ];
            let outcome = cloud::TurnOutcome {
                failure,
                messages: Some(messages),
            };
            cloud::handle_ended(&mut state, cloud::Ended::Turn(member, Some(outcome))).await;
            assert!(!state.hub.is_working(member));
            assert_eq!(state.hub.inbox.owed_by(member), 0);
            let red = state.hub.status.failure(member).is_some();
            assert_eq!(red, failure_given, "a failed turn shows in red");
            let report = notices.try_recv().unwrap();
            assert_eq!(report.to, coordinator);
            report.prompt
        }
    };

    let done = report_of(None, "Footer links fixed.").await;
    assert!(
        done.ends_with(&format!(
            "\n\nFooter links fixed.\n{}",
            task_report_line("3", true)
        )),
        "{done}"
    );
    let failed = report_of(Some("the model refused"), "Half way.").await;
    assert!(
        failed.contains(
            "Stopped before finishing: the model refused\n\nWhat it said last:\nHalf way.\n"
        ),
        "{failed}"
    );
    assert!(failed.ends_with(&task_report_line("3", false)), "{failed}");
}

/// The one text in `texts`.
fn only_text(texts: Vec<String>) -> String {
    assert_eq!(texts.len(), 1, "{texts:?}");
    texts.into_iter().next().unwrap()
}

/// Each agent's chat shows its message as a prompt that folds to the
/// first line paired with it.
fn assert_prompts_fold(commands: &mut mpsc::Receiver<UiCommand>, chats: [(AgentId, String); 2]) {
    let prompts: Vec<(Option<String>, String)> = drain(commands)
        .into_iter()
        .filter_map(|(to, command)| match command {
            UiCommand::TurnStarted { prompt: Some(text) } => Some((to, text)),
            _ => None,
        })
        .collect();
    for (agent, first_line) in chats {
        let shown = prompts
            .iter()
            .find(|(to, _)| to.as_deref() == Some(agent.to_hex().as_str()))
            .map_or_else(
                || panic!("no prompt in {}'s chat: {prompts:?}", agent.to_hex()),
                |(_, text)| text.as_str(),
            );
        assert_eq!(
            aura_ui::inbox::fold(shown).map(|folded| folded.summary),
            Some(first_line.as_str())
        );
    }
}

/// An assignment the mock model works on until the test releases it.
pub(super) const SLOW_FOOTER: &str = "Make the slow footer links work.";

/// The report of a member whose turn on [`SLOW_FOOTER`] the user stopped
/// with `stop`, then carried on with a message: queued before the stop
/// when `queued`, else sent after it.
async fn report_after_user_stop(stop: fn(AgentId) -> UiEvent, queued: bool) -> String {
    let (fixture, _commands) = fixture();
    let coordinator = channel_with_member(&fixture).await;
    let member = fixture.second;
    let mut notices = fixture.hub.take_notices();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    assign_footer_asking(&fixture, coordinator, &code(&fixture, member), SLOW_FOOTER).await;
    notices::handle_notice(&mut state, notices.try_recv().unwrap()).await;
    assert!(state.hub.is_running(member), "the assignment is under way");
    let carry_on = || message(member, "Also the header.", false);
    if queued {
        handle_ui_event(&mut state, carry_on(), &links).await;
    }

    handle_ui_event(&mut state, stop(member), &links).await;
    let stopped = next_done(&mut done).await;
    // The next turn joins the slow prompt, so the mock holds it too.
    fixture.gate.add_permits(1);
    handle_work_done(&mut state, stopped).await;
    assert_eq!(
        state.hub.inbox.owed_by(member),
        1,
        "the stop gave up nothing"
    );
    assert!(notices.try_recv().is_err(), "nothing is reported yet");

    if !queued {
        handle_ui_event(&mut state, carry_on(), &links).await;
    }
    let finished = next_done(&mut done).await;
    handle_work_done(&mut state, finished).await;
    assert_eq!(state.hub.inbox.owed_by(member), 0);
    let report = notices.try_recv().unwrap();
    assert_eq!(report.to, coordinator);
    report.prompt
}

#[tokio::test]
async fn a_turn_the_user_stops_leaves_its_report_to_the_next_turn() {
    let finished = format!("\n\nDone.\n{}", task_report_line("3", true));
    let send_now = |agent: AgentId| UiEvent::SendQueuedNow(agent.to_hex());
    let report = report_after_user_stop(send_now, true).await;
    assert!(report.ends_with(&finished), "sent now: {report}");
    let escape = |agent: AgentId| UiEvent::Cancel(agent.to_hex());
    let report = report_after_user_stop(escape, false).await;
    assert!(report.ends_with(&finished), "Esc: {report}");
    let kill = |agent: AgentId| UiEvent::KillProcess(format!("turn:{}", agent.to_hex()));
    let report = report_after_user_stop(kill, false).await;
    assert!(report.ends_with(&finished), "killed row: {report}");
}

#[tokio::test]
async fn archiving_a_member_mid_turn_gives_up_on_its_assignment() {
    let (fixture, _commands) = fixture();
    let coordinator = channel_with_member(&fixture).await;
    let member = fixture.second;
    let mut notices = fixture.hub.take_notices();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    assign_footer_asking(&fixture, coordinator, &code(&fixture, member), SLOW_FOOTER).await;
    notices::handle_notice(&mut state, notices.try_recv().unwrap()).await;
    // Stopped by the user first, the turn is still given up on.
    handle_ui_event(&mut state, UiEvent::Cancel(member.to_hex()), &links).await;

    handle_ui_event(&mut state, UiEvent::ArchiveAgent(member.to_hex()), &links).await;
    let stopped = next_done(&mut done).await;
    handle_work_done(&mut state, stopped).await;
    assert_eq!(state.hub.inbox.owed_by(member), 0);
    let report = notices.try_recv().unwrap();
    assert_eq!(report.to, coordinator);
    assert!(
        report.prompt.ends_with(&format!(
            "\n\nStopped before finishing: it was stopped\n{}",
            task_report_line("3", false)
        )),
        "{}",
        report.prompt
    );
}

#[tokio::test]
async fn a_cloud_turn_the_user_stops_leaves_its_report_to_the_next_turn() {
    let (fixture, _commands) = fixture();
    let coordinator = channel_with_member(&fixture).await;
    let member = fixture.second;
    let mut notices = fixture.hub.take_notices();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut state = loop_state(&fixture, &mut b);
    assign_footer(&fixture, coordinator, &code(&fixture, member)).await;
    let asked = aura_model::Message::user(notices.try_recv().unwrap().prompt);
    let ended = |failure: Option<&str>, said: &[&str]| {
        let mut messages = vec![asked.clone()];
        for (at, text) in said.iter().enumerate() {
            messages.push(if at % 2 == 0 {
                aura_model::Message::assistant(*text)
            } else {
                aura_model::Message::user(*text)
            });
        }
        let outcome = cloud::TurnOutcome {
            failure: failure.map(str::to_string),
            messages: Some(messages),
        };
        cloud::Ended::Turn(member, Some(outcome))
    };

    // Esc stops the turn on its machine, which ends it stopped.
    state.cloud.start_turn(member);
    handle_ui_event(&mut state, UiEvent::Cancel(member.to_hex()), &links).await;
    let stopped = ended(Some("it was stopped"), &["Half way."]);
    cloud::handle_ended(&mut state, stopped).await;
    assert_eq!(
        state.hub.inbox.owed_by(member),
        1,
        "the stop gave up nothing"
    );
    assert!(notices.try_recv().is_err(), "nothing is reported yet");

    let finished = ended(None, &["Half way.", "Carry on.", "Footer links fixed."]);
    cloud::handle_ended(&mut state, finished).await;
    assert_eq!(state.hub.inbox.owed_by(member), 0);
    let report = notices.try_recv().unwrap();
    assert_eq!(report.to, coordinator);
    assert!(
        report.prompt.ends_with(&format!(
            "\n\nFooter links fixed.\n{}",
            task_report_line("3", true)
        )),
        "{}",
        report.prompt
    );
}
