use super::*;

/// The chat panel's right border beside its text, below the header bar,
/// the body's top border and the blank row above the conversation, down
/// to the line above the input.
fn chat_track(rows: &[String]) -> String {
    let (tabs, _, right) = chat_panel(rows);
    rows[tabs + 4..]
        .iter()
        .map(|r| r.chars().nth(right).unwrap())
        .take_while(|&c| c != '┤')
        .collect()
}

#[test]
fn test_record_rows_align_kind_what_and_group_columns() {
    use crate::events::{RecordStatus, RecordSummary, UiCommand};

    let mut app = crate::App::new();
    for (seq, kind, what, group, detail) in [
        (1, "Policy", "read_file", "file", "allow · user_default"),
        (2, "ToolCall", "run_command", "shell", "cargo test"),
    ] {
        app.process_command(UiCommand::NewRecord(RecordSummary {
            seq,
            timestamp: "12:00:00".to_string(),
            full_hash: String::new(),
            hash_suffix: String::new(),
            tx_kind: kind.to_string(),
            sender: String::new(),
            message: String::new(),
            action_count: 0,
            effect_status: String::new(),
            status: RecordStatus::Ok,
            info: String::new(),
            what: what.to_string(),
            group: group.to_string(),
            detail: detail.to_string(),
            error_details: String::new(),
            tx_id: String::new(),
            agent_id: String::new(),
            ts_ms: 0,
            policy: None,
        }));
    }
    let rows = render_rows(&mut app, 250);
    let screen = rows.join("\n");
    let row = |needle: &str| {
        rows.iter()
            .find(|row| row.contains(needle))
            .unwrap_or_else(|| panic!("{needle:?} not in:\n{screen}"))
    };
    let column = |row: &str, needle: &str| row[..row.find(needle).unwrap()].chars().count();
    let policy = row("READ_FILE");
    let call = row("RUN_COMMAND");

    assert_eq!(column(policy, "POLICY"), column(call, "TOOLCALL"));
    assert_eq!(column(policy, "READ_FILE"), column(call, "RUN_COMMAND"));
    assert_eq!(column(policy, " FILE "), column(call, " SHELL "));
    assert_eq!(column(policy, "allow"), column(call, "cargo test"));
    assert!(!screen.contains("12:00:00"), "{screen}");
}

#[test]
fn test_overflowing_panels_show_a_scrollbar() {
    use crate::components::{Message, MessageRole};
    use crate::events::{RecordStatus, RecordSummary, UiCommand};

    let mut app = crate::App::new();
    app.add_message(Message::new(MessageRole::User, "short"));
    let rows = render_rows(&mut app, 120);
    assert!(!rows.join("\n").contains('┃'), "{}", rows.join("\n"));

    for i in 0..40 {
        app.add_message(Message::new(MessageRole::User, &format!("message {i}")));
        app.process_command(UiCommand::NewRecord(RecordSummary {
            seq: i,
            timestamp: "12:00:00".to_string(),
            full_hash: String::new(),
            hash_suffix: String::new(),
            tx_kind: "Prompt".to_string(),
            sender: String::new(),
            message: String::new(),
            action_count: 0,
            effect_status: String::new(),
            status: RecordStatus::Ok,
            info: format!("record {i}"),
            what: String::new(),
            group: String::new(),
            detail: String::new(),
            error_details: String::new(),
            tx_id: String::new(),
            agent_id: String::new(),
            ts_ms: 0,
            policy: None,
        }));
    }
    let rows = render_rows(&mut app, 120);
    let screen = rows.join("\n");

    // The chat sits at its end, so the thumb is at the bottom of the track,
    // which starts under the chat's blank top row.
    let chat = chat_track(&rows);
    assert!(
        chat.starts_with('│') && chat.ends_with('┃'),
        "{chat:?}\n{screen}"
    );
    // Records list oldest first and follow the newest at the bottom.
    let records = right_border(&rows, "│KRN│");
    assert!(
        records.starts_with('│') && records.ends_with('┃'),
        "{records:?}\n{screen}"
    );

    app.scroll_chat_to_top();
    let rows = render_rows(&mut app, 120);
    let chat = chat_track(&rows);
    assert!(chat.starts_with('┃') && chat.ends_with('│'), "{chat:?}");
}

#[test]
fn test_status_line_shows_context_cache_and_cost() {
    use crate::events::{CallStats, UiCommand};

    let mut app = crate::App::new();
    app.process_command(UiCommand::CallStats(CallStats {
        model: "aura-claude-sonnet-4-6".to_string(),
        input_tokens: 4_000,
        cache_read_tokens: 120_000,
        cache_write_tokens: 0,
        output_tokens: 900,
        cost_usd: Some(3.42),
        unpriced: false,
        context_limit: Some(200_000),
    }));
    app.process_command(UiCommand::SetQueue(vec![crate::events::QueuedItemView {
        id: 1,
        text: "next".to_string(),
        editable: true,
    }]));

    // Wide enough for every part: the ring takes two columns more.
    let rows = render_rows(&mut app, 170);
    let status = &rows[rows.len() - 1];
    assert!(status.contains("◑ 62% context used"), "{status:?}");
    assert!(status.contains("96% cached"), "{status:?}");
    assert!(status.contains("$3.42"), "{status:?}");
    assert!(status.contains("1 call"), "{status:?}");
    assert!(status.contains("1 queued"), "{status:?}");
}

#[test]
fn test_status_line_drops_trailing_parts_when_narrow() {
    use crate::events::{CallStats, UiCommand};

    let mut app = crate::App::new();
    app.process_command(UiCommand::CallStats(CallStats {
        input_tokens: 10_000,
        context_limit: Some(200_000),
        cost_usd: Some(0.5),
        ..CallStats::default()
    }));
    let rows = render_rows(&mut app, 90);
    let status = &rows[rows.len() - 1];
    assert!(status.contains("5% context used"), "{status:?}");
    assert!(!status.contains("call"), "{status:?}");
}
