//! Agents talking through their inboxes on the event loop: work a
//! coordinator assigns runs as the member's task, and the member's report
//! comes back to the coordinator.

use super::*;
use crate::event_loop::inbox::{assign, AssignArgs};
use aura_exec_tools::todo::task_report_line;
use aura_store_db::NewChannel;
use aura_ui::events::MessageRole;

/// A channel `#site` with the fixture's second agent as a member, as
/// a member joins: top-level, the coordinator having brought it in.
/// Returns the coordinator.
async fn channel_with_member(fixture: &Fixture) -> AgentId {
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

fn code(fixture: &Fixture, agent: AgentId) -> String {
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
async fn assign_footer(fixture: &Fixture, coordinator: AgentId, worker: &str) -> String {
    assign(
        &fixture.hub,
        coordinator,
        AssignArgs {
            to: Some(worker.to_lowercase()),
            agent: None,
            subject: "Footer".to_string(),
            prompt: "Make the footer links work.".to_string(),
            todo_id: Some("3".to_string()),
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
async fn a_new_member_of_a_cloud_channel_without_a_project_works_here_and_says_why() {
    let (fixture, mut commands) = fixture();
    let in_cloud = NewChannel {
        slug: "site".to_string(),
        title: "Website relaunch".to_string(),
        placement: aura_store_db::ChannelPlacement::Cloud,
        ..NewChannel::default()
    };
    let channel = channels::create_channel(&fixture.hub, in_cloud)
        .await
        .unwrap();
    let mut notices = fixture.hub.take_notices();
    let mut b = borrowed(&fixture);
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);

    let brief = AssignArgs {
        to: None,
        agent: Some("reviewer".to_string()),
        subject: "Footer".to_string(),
        prompt: "Make the footer links work.".to_string(),
        todo_id: None,
    };
    let said = assign(&fixture.hub, channel.coordinator, brief)
        .await
        .unwrap();
    assert!(said.contains("(new member)"), "{said}");
    let assignment = notices.try_recv().unwrap();
    let member = assignment.to;
    assert!(subagents::lock(&state.hub.cloud_starts).contains(&member));

    run_notice(&mut state, &mut done, assignment, member).await;
    assert!(subagents::lock(&state.hub.cloud_starts).is_empty());
    assert!(!state.cloud.is_cloud(member) && !state.cloud.is_moving(member));
    let member_hex = member.to_hex();
    let why: Vec<String> = drain(&mut commands)
        .into_iter()
        .filter(|(to, _)| to.as_deref() == Some(member_hex.as_str()))
        .filter_map(|(_, command)| match command {
            UiCommand::ShowMessage(message) if message.role == MessageRole::System => {
                Some(message.content)
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        why,
        [
            "The channel has no project folder for a cloud machine to start from — it works on \
          this computer instead."
        ],
    );
}

#[tokio::test]
async fn a_cloud_members_turn_answers_its_assignment_when_the_loop_hears_it_ended() {
    let report_of = |failure: Option<&str>, said: &str| {
        let failure = failure.map(str::to_string);
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
        failed.contains("Stopped before finishing: the model refused"),
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
