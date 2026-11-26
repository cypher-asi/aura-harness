//! An agent's open plan stays its context outside plan mode: a typed
//! prompt carries on with it, and requests in plan mode file under it,
//! until every todo of the plan is closed.

use super::*;
use crate::event_loop::tasks::load_all;
use aura_context_prompts::model_messages::build_plan::continue_plan_note;

const PLAN: &str = ".aura/plans/parser_00000001.plan.md";

/// A turn that only answers keeps no task, so the prompts the tests send
/// make a tool call (see [`scripted`]).
const KEEP_GOING: &str = "keep going and read the big file";

/// Write [`PLAN`] into `fixture`'s workspace with todos `a` and `b`, `b`
/// in `b_status`, and make it the first agent's plan.
fn give_plan(fixture: &Fixture, state: &mut LoopState<'_>, b_status: &str) {
    let dir = fixture.workspace.join(".aura/plans");
    std::fs::create_dir_all(&dir).unwrap();
    let text = format!(
        "---\nname: Parser\ntodos:\n  - id: a\n    content: Lex\n    status: completed\n  \
         - id: b\n    content: Parse\n    status: {b_status}\n---\n"
    );
    std::fs::write(fixture.workspace.join(PLAN), text).unwrap();
    state.idle.get_mut(&fixture.first).unwrap().active_plan = Some(PLAN.to_string());
}

/// The plan and `builds` of every task kept in `fixture`'s workspace.
fn kept_tasks(fixture: &Fixture) -> Vec<(Option<String>, bool)> {
    load_all(&fixture.workspace)
        .into_iter()
        .map(|record| (record.view.plan, record.view.builds))
        .collect()
}

#[tokio::test]
async fn a_prompt_outside_plan_mode_continues_the_open_plan() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    give_plan(&fixture, &mut state, "pending");

    turn(&mut state, &mut done, &links, first, KEEP_GOING).await;

    let session = &state.idle[&first];
    assert!(session.building_plan);
    let ids: Vec<&str> = session.todos.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(ids, ["a", "b"], "the plan's todos are the agent's list");
    let sent = user_texts(&state, first);
    let first_message = sent.first().unwrap();
    assert!(first_message.starts_with(KEEP_GOING), "{first_message}");
    assert!(
        first_message.ends_with(&format!("\n\n{}", continue_plan_note(PLAN))),
        "{first_message}"
    );
    assert_eq!(kept_tasks(&fixture), [(Some(PLAN.to_string()), true)]);
}

#[tokio::test]
async fn a_plan_with_every_todo_closed_is_not_continued() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    give_plan(&fixture, &mut state, "cancelled");

    turn(&mut state, &mut done, &links, first, KEEP_GOING).await;

    let session = &state.idle[&first];
    assert!(!session.building_plan);
    assert!(session.todos.is_empty());
    assert_eq!(user_texts(&state, first).first().unwrap(), KEEP_GOING);
    assert_eq!(kept_tasks(&fixture), [(None, false)]);
}

/// The chat of `agent` dropped its list, said the plan is complete and
/// shows no plan.
fn assert_plan_completion_shown(commands: &mut mpsc::Receiver<UiCommand>, agent: AgentId) {
    let hex = agent.to_hex();
    let shown: Vec<UiCommand> = drain(commands)
        .into_iter()
        .filter(|(to, _)| to.as_deref() == Some(hex.as_str()))
        .map(|(_, command)| command)
        .collect();
    let last_todos = shown.iter().rev().find_map(|c| match c {
        UiCommand::SetTodos(todos) => Some(todos),
        _ => None,
    });
    assert_eq!(last_todos.map(Vec::len), Some(0), "the chat drops the list");
    assert!(shown
        .iter()
        .any(|c| matches!(c, UiCommand::ShowMessage(m) if m.content == "Plan complete: Parser")));
    assert!(
        matches!(
            shown
                .iter()
                .rev()
                .find(|c| matches!(c, UiCommand::SetPlan(_))),
            Some(UiCommand::SetPlan(None))
        ),
        "no plan is being worked on"
    );
}

#[tokio::test]
async fn closing_every_todo_of_the_plan_being_built_completes_it() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    give_plan(&fixture, &mut state, "pending");

    turn(&mut state, &mut done, &links, first, "close every todo").await;

    let session = &state.idle[&first];
    assert!(!session.building_plan);
    assert!(session.todos.is_empty(), "a finished list is dropped");
    assert_eq!(session.active_plan, None, "the next /plan writes a new one");
    let plan = aura_exec_tools::plan_tools::PlanFile::load(&fixture.workspace.join(PLAN)).unwrap();
    assert_eq!(plan.progress(), (2, 2));
    assert_plan_completion_shown(&mut commands, first);

    // A new list is the agent's own: it is not filed under the finished
    // plan, nor mirrored into it.
    turn(&mut state, &mut done, &links, first, "start new todos").await;
    let session = &state.idle[&first];
    let ids: Vec<&str> = session.todos.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(ids, ["x"]);
    assert!(!session.building_plan);
    let after = aura_exec_tools::plan_tools::PlanFile::load(&fixture.workspace.join(PLAN)).unwrap();
    assert_eq!(after.meta.todos, plan.meta.todos);
    assert_eq!(
        kept_tasks(&fixture)
            .iter()
            .filter(|t| t.0.is_none())
            .count(),
        1
    );
}

#[tokio::test]
async fn a_request_in_plan_mode_files_under_the_open_plan() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    give_plan(&fixture, &mut state, "in_progress");
    handle_ui_event(&mut state, UiEvent::TogglePlan(first.to_hex()), &links).await;
    assert_eq!(state.hub.mode_of(first), PermissionMode::Plan);

    turn(&mut state, &mut done, &links, first, KEEP_GOING).await;

    let session = &state.idle[&first];
    assert!(!session.building_plan);
    assert_eq!(user_texts(&state, first).first().unwrap(), KEEP_GOING);
    assert_eq!(kept_tasks(&fixture), [(Some(PLAN.to_string()), false)]);
}
