//! Restarts and checkpoints.

use super::*;

/// A tool call with its result, as a turn leaves them in the history.
fn tool_exchange(id: &str) -> [aura_model::Message; 2] {
    use aura_model::{ContentBlock, Message, Role, ToolResultContent};
    [
        Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: id.to_string(),
                name: "read_file".to_string(),
                input: serde_json::json!({ "path": "src/main.rs" }),
            }],
        ),
        Message::new(
            Role::User,
            vec![ContentBlock::ToolResult {
                tool_use_id: id.to_string(),
                content: ToolResultContent::Text("fn main() {}".to_string()),
                is_error: false,
            }],
        ),
    ]
}

#[tokio::test]
async fn a_restart_keeps_tool_calls_todos_and_usage() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);

    handle_ui_event(&mut state, message(first, "read main", true), &links).await;
    let finished = next_done(&mut done).await;
    handle_work_done(&mut state, finished).await;
    let session = state.idle.get_mut(&first).unwrap();
    session.messages.extend(tool_exchange("t1"));
    session.todos = vec![todo("Fix main")];
    session.usage.calls = 4;
    session.save_checkpoint().await;

    let reopened = open_again(&fixture, first);
    assert_eq!(reopened.messages.len(), 4, "{:?}", reopened.messages);
    assert!(reopened.messages[2]
        .content
        .iter()
        .any(|b| matches!(b, aura_model::ContentBlock::ToolUse { .. })));
    assert_eq!(reopened.todos, [todo("Fix main")]);
    assert_eq!(reopened.usage.calls, 4);
    assert_eq!(reopened.started_ms, state.idle[&first].started_ms);
}

#[tokio::test]
async fn the_record_past_a_checkpoint_is_replayed() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let first = fixture.first;
    let mut state = loop_state(&fixture, &mut b);
    let session = state.idle.get_mut(&first).unwrap();
    session.messages = tool_exchange("t1").to_vec();
    session.save_checkpoint().await;

    let later = Transaction::user_prompt(first, "written after the checkpoint");
    session.kernel.process_direct(later).await.unwrap();

    let reopened = open_again(&fixture, first);
    assert_eq!(reopened.messages.len(), 3);
    assert_eq!(
        reopened.messages[2].text_content(),
        "written after the checkpoint"
    );
}

#[tokio::test]
async fn a_new_session_drops_the_checkpoint_and_resume_brings_it_back() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let first = fixture.first;
    let mut state = loop_state(&fixture, &mut b);
    let session = state.idle.get_mut(&first).unwrap();
    session.messages = vec![aura_model::Message::user("old task")];
    session.messages.extend(tool_exchange("t1"));
    session.todos = vec![todo("Finish the old task")];
    session.started_ms = 1_000;
    session.save_checkpoint().await;

    assert!(handlers::handle_new_session(&mut state).await);
    assert!(open_again(&fixture, first).messages.is_empty());

    handlers::handle_resume(&mut state, first, None).await;
    let listed = drain(&mut commands).into_iter().any(|(_, c)| {
        matches!(c, UiCommand::ShowMessage(m) if m.content.contains("1. ") && m.content.contains("old task"))
    });
    assert!(listed, "the old session is listed");

    handlers::handle_resume(&mut state, first, Some(1)).await;
    assert_eq!(state.idle[&first].messages.len(), 3);
    assert_eq!(state.idle[&first].todos, [todo("Finish the old task")]);
    let commands = drain(&mut commands);
    assert!(commands
        .iter()
        .any(|(_, c)| matches!(c, UiCommand::LoadConversation(chat) if chat.len() == 2)));

    let reopened = open_again(&fixture, first);
    assert_eq!(reopened.messages.len(), 3, "the resumed session is current");
    assert_eq!(reopened.started_ms, 1_000);
}
