use super::*;

#[test]
fn test_agents_scroll_keeps_selection_visible() {
    // Rows: project 0, agent 1, gap 2, project 3, agent 4.
    let (mut app, _rx) = app_with_agents();
    app.selected_agent = 1;
    app.sync_agents_scroll(2);
    assert_eq!(app.agents_scroll(), 0);

    app.selected_agent = 4;
    app.sync_agents_scroll(2);
    assert_eq!(app.agents_scroll(), 3);

    app.selected_agent = 3;
    app.sync_agents_scroll(2);
    assert_eq!(app.agents_scroll(), 3);

    app.selected_agent = 0;
    app.sync_agents_scroll(2);
    assert_eq!(app.agents_scroll(), 0);
}

#[test]
fn test_follow_selection_moves_only_to_keep_selection_visible() {
    use crate::app::follow_selection;
    // Moving within the view leaves it alone.
    assert_eq!(follow_selection(0, 4, 5, 20), 0);
    // Stepping past the bottom scrolls one row, not half a page.
    assert_eq!(follow_selection(0, 5, 5, 20), 1);
    // Stepping back up inside the view keeps it where it is.
    assert_eq!(follow_selection(6, 7, 5, 20), 6);
    assert_eq!(follow_selection(6, 5, 5, 20), 5);
    // Never scrolls past the end of the list.
    assert_eq!(follow_selection(18, 19, 5, 20), 15);
    assert_eq!(follow_selection(3, 1, 5, 2), 0);
}

#[test]
fn test_chat_scroll_holds_position_as_lines_arrive() {
    let mut app = App::new();
    assert_eq!(app.sync_chat_scroll(100, 20, true), 80);

    app.scroll_up(10);
    assert_eq!(app.sync_chat_scroll(100, 20, false), 70);
    // 15 lines stream in below: the same lines stay on screen.
    assert_eq!(app.sync_chat_scroll(115, 20, false), 70);
    assert_eq!(app.scroll_offset(), 25);
    // A reflow moves every line, so the offset is only clamped.
    assert_eq!(app.sync_chat_scroll(60, 20, true), 15);

    app.scroll_chat_to_bottom();
    assert_eq!(app.sync_chat_scroll(70, 20, false), 50);
    assert_eq!(app.scroll_offset(), 0);
}

#[test]
fn test_chat_page_keys_scroll_a_screenful() {
    let mut app = App::new();
    app.set_chat_text_area(Rect::new(0, 0, 60, 22));
    app.sync_chat_scroll(200, 22, true);

    app.handle_key(key(KeyCode::PageUp));
    assert_eq!(app.scroll_offset(), 20);
    app.handle_key(KeyEvent::new(KeyCode::Home, KeyModifiers::CONTROL));
    app.sync_chat_scroll(200, 22, false);
    assert_eq!(app.scroll_offset(), 178);
    app.handle_key(KeyEvent::new(KeyCode::End, KeyModifiers::CONTROL));
    assert_eq!(app.scroll_offset(), 0);
}

#[test]
fn test_incoming_messages_keep_scroll_but_sending_jumps_to_bottom() {
    let mut app = App::new();
    app.scroll_offset = 7;
    app.add_message(Message::new(MessageRole::System, "tool output"));
    assert_eq!(app.scroll_offset(), 7);

    app.input.text = "hello".to_string();
    app.submit_input();
    assert_eq!(app.scroll_offset(), 0);
}

#[test]
fn test_load_conversation_replaces_messages() {
    let mut app = App::new();
    app.add_message(Message::new(MessageRole::User, "old"));
    app.process_command(UiCommand::StartThinking);
    app.process_command(UiCommand::StartStreaming);
    app.scroll_offset = 7;

    app.process_command(UiCommand::LoadConversation(vec![
        MessageData::new(MessageRole::User, "hi"),
        MessageData {
            activity: Some(ActivityData::Thinking {
                text: "hmm".to_string(),
                duration_ms: Some(1200),
            }),
            ..MessageData::new(MessageRole::Assistant, "")
        },
        MessageData {
            activity: Some(ActivityData::Tool {
                id: "t1".to_string(),
                name: "read_file".to_string(),
                args: r#"{"path":"a.rs"}"#.to_string(),
                result: Some("fn main() {}".to_string()),
                is_error: false,
                duration_ms: Some(5),
                policy: None,
            }),
            ..MessageData::new(MessageRole::Assistant, "")
        },
        MessageData::new(MessageRole::Assistant, "hello"),
        MessageData {
            command: Some("ls -la".to_string()),
            ..MessageData::new(MessageRole::User, "ls -la")
        },
    ]));

    let contents: Vec<&str> = app.messages().iter().map(Message::content).collect();
    assert_eq!(contents, ["hi", "", "", "hello", "ls -la"]);
    assert!(matches!(
        app.messages()[1].activity_step(),
        Some(Activity::Thinking(step)) if step.text == "hmm" && !step.running
    ));
    assert!(matches!(
        app.messages()[2].activity_step(),
        Some(Activity::Tool(tool)) if tool.status == ToolStatus::Success
    ));
    let run = app.messages()[4].shell_run().expect("shell message");
    assert_eq!(run.command, "ls -la");
    assert!(!app.messages()[4].is_running_shell());
    assert!(!app.has_live_activity());
    assert!(app.messages().iter().all(|m| !m.is_streaming()));
    assert_eq!(app.scroll_offset(), 0);
}

#[test]
fn test_reply_model_sets_the_turn_so_far_on_replies_since_last_user_message() {
    let mut app = App::new();
    app.add_message(Message::new(MessageRole::Assistant, "earlier reply"));
    app.add_message(Message::new(MessageRole::User, "question"));
    app.process_command(UiCommand::StartThinking);
    app.process_command(UiCommand::FinishThinking);
    app.process_command(UiCommand::StartStreaming);
    app.process_command(UiCommand::AppendText("first part".to_string()));
    app.process_command(UiCommand::FinishStreaming);
    app.process_command(UiCommand::SetReplyModel {
        model: "qwen3:4b".to_string(),
        usage: Some(CallUsage {
            tokens: 900,
            cost_usd: None,
            unpriced_calls: 0,
        }),
        sources: Vec::new(),
    });
    app.process_command(UiCommand::StartStreaming);
    app.process_command(UiCommand::AppendText("second part".to_string()));
    let opus = CallUsage {
        tokens: 12_400,
        cost_usd: Some(0.042),
        unpriced_calls: 0,
    };
    app.process_command(UiCommand::SetReplyModel {
        model: "qwen3:4b · claude-opus-4-7".to_string(),
        usage: Some(opus),
        sources: Vec::new(),
    });

    let models: Vec<(&str, Option<&str>, Option<u64>)> = app
        .messages()
        .iter()
        .map(|m| (m.content(), m.model(), m.usage().map(|u| u.tokens)))
        .collect();
    assert_eq!(
        models,
        [
            ("earlier reply", None, None),
            ("question", None, None),
            ("", None, None),
            (
                "first part",
                Some("qwen3:4b · claude-opus-4-7"),
                Some(12_400)
            ),
            (
                "second part",
                Some("qwen3:4b · claude-opus-4-7"),
                Some(12_400)
            ),
        ]
    );
    assert_eq!(app.messages().back().unwrap().usage(), Some(opus));
}
