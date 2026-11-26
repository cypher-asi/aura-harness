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
async fn channel_with_member(fixture: &Fixture) -> AgentId {
    let channel = channels::create_channel(
        &fixture.hub,
        NewChannel {
            slug: "site".to_string(),
            title: "Website relaunch".to_string(),
            description: String::new(),
            project_id: None,
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
