
use super::*;
use crate::agent_loop::AgentLoopConfig;
use aura_exec_tools::todo::{TodoItem, TodoStatus};

fn state_with_todos(statuses: &[TodoStatus], last_reply: &str) -> LoopState {
    let config = AgentLoopConfig {
        initial_todos: statuses
            .iter()
            .enumerate()
            .map(|(i, status)| TodoItem {
                id: i.to_string(),
                content: format!("step {i}"),
                status: *status,
                assignee: None,
            })
            .collect(),
        ..AgentLoopConfig::for_agent("claude-test-model")
    };
    LoopState::new_for_tests(
        &config,
        vec![Message::user("do it"), Message::assistant(last_reply)],
    )
}

#[test]
fn nudges_toward_in_progress_item_first() {
    let mut state = state_with_todos(
        &[
            TodoStatus::Completed,
            TodoStatus::Pending,
            TodoStatus::InProgress,
        ],
        "Done with step 0.",
    );
    let nudge = open_todo_nudge(&mut state).expect("open items should nudge");
    assert!(nudge.contains("step 2"), "{nudge}");
}

#[test]
fn no_nudge_when_everything_is_closed() {
    let mut state = state_with_todos(&[TodoStatus::Completed, TodoStatus::Cancelled], "All done.");
    assert!(open_todo_nudge(&mut state).is_none());
}

#[test]
fn no_nudge_when_model_asks_the_user() {
    let mut state = state_with_todos(&[TodoStatus::Pending], "Which database should I use?");
    assert!(open_todo_nudge(&mut state).is_none());
}

#[test]
fn gives_up_after_unchanged_nudges() {
    let mut state = state_with_todos(&[TodoStatus::Pending], "Working on it.");
    for _ in 0..MAX_STALLED_TODO_NUDGES {
        assert!(open_todo_nudge(&mut state).is_some());
    }
    assert!(open_todo_nudge(&mut state).is_none());
}

#[test]
fn progress_resets_the_stall_counter() {
    let mut state = state_with_todos(&[TodoStatus::Pending, TodoStatus::Pending], "Ok.");
    assert!(open_todo_nudge(&mut state).is_some());
    assert!(open_todo_nudge(&mut state).is_some());
    state.result.todos[0].status = TodoStatus::Completed;
    assert!(open_todo_nudge(&mut state).is_some());
    assert_eq!(state.stalled_todo_nudges, 0);
}
