use super::channel_member_chat::{app_in_channel_with_events, switch_request};
use super::*;
use crate::events::{ChannelPostAuthor, ChannelPostView, MessageSummaryView};

type Screen = ratatui::Terminal<ratatui::backend::TestBackend>;

fn screen() -> Screen {
    ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 40)).unwrap()
}

/// Every cell of `screen`'s last frame, row by row.
fn screen_text(screen: &Screen) -> String {
    let buffer = screen.backend().buffer();
    let width = usize::from(buffer.area.width);
    buffer
        .content()
        .chunks(width)
        .map(|row| {
            row.iter()
                .map(ratatui::buffer::Cell::symbol)
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

const LONG: &str = "Please add a history tab to the right panel of channels that lists every \
message I sent there, summarized when it runs long";

fn message(role: MessageRole, text: &str, at_ms: u64) -> MessageData {
    MessageData {
        timestamp_ms: Some(at_ms),
        ..MessageData::new(role, text)
    }
}

/// `co`'s chat: two prompts, the long one first, each answered, then
/// `replies` more replies after them.
fn load_coordinator(app: &mut App, replies: u64) {
    let mut history = vec![
        message(MessageRole::User, LONG, 1_000),
        message(MessageRole::Assistant, "On it.", 2_000),
        message(MessageRole::System, "policy: read_file allowed", 2_500),
        message(MessageRole::User, "ship it", 3_000),
    ];
    history.extend((0..replies).map(|i| {
        let text = format!("reply {i}\n\nwith a second paragraph");
        message(MessageRole::Assistant, &text, 4_000 + i)
    }));
    app.process_command(for_agent("co", UiCommand::LoadConversation(history)));
}

/// `#zode`'s posts: one by the user, one by RE36.
fn post_in_zode(app: &mut App) {
    let post = |seq, author, text: &str| ChannelPostView {
        seq,
        author,
        text: text.to_string(),
        at_ms: 2_200,
        ..ChannelPostView::default()
    };
    let agent = ChannelPostAuthor::Agent {
        id: "re".to_string(),
        code: Some("RE36".to_string()),
        name: "Reviewer".to_string(),
    };
    app.process_command(UiCommand::SetChannelPosts {
        channel_id: "c1".to_string(),
        posts: vec![
            post(1, ChannelPostAuthor::User, "@RE36 review it"),
            post(2, agent, "Reviewed, looks good"),
        ],
    });
}

/// `#zode` open on `co`'s chat with HISTORY showing and focused.
fn channel_on_history(replies: u64) -> (tempfile::TempDir, App, mpsc::Receiver<UiEvent>) {
    let (dir, mut app, rx) = app_in_channel_with_events("co");
    load_coordinator(&mut app, replies);
    post_in_zode(&mut app);
    app.show_side_tab(SidePanelTab::ChannelHistory);
    app.focus = PanelFocus::Sandbox;
    app.refresh_channel_history();
    (dir, app, rx)
}

fn listed(app: &App) -> Vec<String> {
    app.channel_history()
        .iter()
        .map(|entry| entry.line.clone())
        .collect()
}

/// The summaries asked for in the events waiting in `rx`: who was asked
/// and the texts.
fn summary_requests(rx: &mut mpsc::Receiver<UiEvent>) -> Vec<(String, Vec<String>)> {
    let mut out = Vec::new();
    while let Ok(event) = rx.try_recv() {
        if let UiEvent::SummarizeMessages { agent_id, requests } = event {
            out.push((agent_id, requests.into_iter().map(|r| r.text).collect()));
        }
    }
    out
}

#[test]
fn test_history_is_the_tab_after_tasks_and_only_in_a_channel() {
    let in_channel = SidePanelTab::tabs(true);
    assert_eq!(
        in_channel[in_channel.len() - 2..],
        [SidePanelTab::Tasks, SidePanelTab::ChannelHistory]
    );
    assert!(!SidePanelTab::tabs(false).contains(&SidePanelTab::ChannelHistory));
    assert_eq!(SidePanelTab::ChannelHistory.label(), "HISTORY");
    assert!(SidePanelTab::ChannelHistory.is_channel_tab());
    assert_eq!(SidePanelTab::Tasks.next(true), SidePanelTab::ChannelHistory);

    let (_dir, mut app, _rx) = channel_on_history(0);
    assert_eq!(app.side_tab(), SidePanelTab::ChannelHistory);
    app.close_channel();
    app.show_side_tab(SidePanelTab::ChannelHistory);
    assert_ne!(
        app.side_tab(),
        SidePanelTab::ChannelHistory,
        "not outside one"
    );
}

#[test]
fn test_history_lists_only_the_users_messages_oldest_first() {
    let (_dir, mut app, _rx) = channel_on_history(3);
    let long = crate::app::apps::one_line(LONG);
    assert_eq!(
        listed(&app),
        [long, "@RE36 review it".to_string(), "ship it".to_string()]
    );
    assert_eq!(app.selected_channel_history(), 2, "on the newest");

    // A member's chat on screen keeps the coordinator's prompts listed.
    app.process_command(UiCommand::SetActiveAgent("re".to_string()));
    app.refresh_channel_history();
    assert_eq!(listed(&app).len(), 3);
}

#[test]
fn test_only_long_messages_ask_for_a_summary_once_and_a_failure_keeps_the_cut() {
    let (_dir, mut app, mut rx) = channel_on_history(0);
    let mut screen = screen();
    draw(&mut app, &mut screen);
    assert_eq!(
        summary_requests(&mut rx),
        [("co".to_string(), vec![LONG.to_string()])],
        "the coordinator is asked, for the long one only"
    );
    draw(&mut app, &mut screen);
    assert!(summary_requests(&mut rx).is_empty(), "asked once");
    let shown = |app: &App| {
        app.channel_history_line(&app.channel_history()[0])
            .into_owned()
    };
    assert!(shown(&app).starts_with("Please add a history tab"));
    assert!(shown(&app).ends_with('…'), "cut while it is pending");
    assert!(shown(&app).chars().count() <= 80);
    assert_eq!(
        app.channel_history_line(&app.channel_history()[2]),
        "ship it"
    );

    let key = |app: &App| {
        let entry = &app.channel_history()[0];
        format!("{:016x}", crate::app::apps::text_digest(&entry.text))
    };
    let failed = MessageSummaryView {
        key: key(&app),
        summary: None,
    };
    app.process_command(UiCommand::SetMessageSummary(failed));
    draw(&mut app, &mut screen);
    assert!(
        summary_requests(&mut rx).is_empty(),
        "a failure is not asked again"
    );
    assert!(shown(&app).ends_with('…'), "the cut stays");

    let written = MessageSummaryView {
        key: key(&app),
        summary: Some("Add a channel history tab".to_string()),
    };
    app.process_command(UiCommand::SetMessageSummary(written));
    assert_eq!(shown(&app), "Add a channel history tab");
    draw(&mut app, &mut screen);
    assert!(screen_text(&screen).contains("›  Add a"), "drawn");
}

#[test]
fn test_keys_move_the_highlight_and_the_wheel_only_scrolls() {
    let (_dir, mut app, _rx) = channel_on_history(0);
    for i in 0..80 {
        app.process_command(UiCommand::ChannelPost {
            channel_id: "c1".to_string(),
            post: ChannelPostView {
                seq: 10 + i,
                author: ChannelPostAuthor::User,
                text: format!("note {i}"),
                at_ms: 10_000 + i,
                ..ChannelPostView::default()
            },
        });
    }
    let mut screen = screen();
    draw(&mut app, &mut screen);
    let newest = app.channel_history().len() - 1;
    assert_eq!(app.selected_channel_history(), newest, "follows the newest");
    let scroll = app.channel_history_scroll();
    assert!(scroll > 0, "the list is longer than the panel");

    app.scroll_panel_up(PanelFocus::Sandbox, 3);
    draw(&mut app, &mut screen);
    assert_eq!(
        app.selected_channel_history(),
        newest,
        "the wheel leaves it"
    );
    assert_eq!(app.channel_history_scroll(), scroll - 3);

    app.handle_key(key(KeyCode::Up));
    assert_eq!(app.selected_channel_history(), newest - 1);
    app.handle_key(key(KeyCode::Home));
    assert_eq!(app.selected_channel_history(), 0);
    draw(&mut app, &mut screen);
    assert_eq!(
        app.channel_history_scroll(),
        0,
        "a key brings the list back"
    );
    app.handle_key(key(KeyCode::End));
    assert_eq!(app.selected_channel_history(), newest);
}

#[test]
fn test_enter_scrolls_the_chat_far_up_to_the_message_and_lights_it() {
    let (_dir, mut app, _rx) = channel_on_history(80);
    let mut screen = screen();
    draw(&mut app, &mut screen);
    assert_eq!(
        app.scroll_offset(),
        0,
        "at the bottom, far below the prompts"
    );
    assert!(
        !screen_text(&screen).contains("→ ship it"),
        "the chat doesn't show it"
    );

    app.handle_key(key(KeyCode::Up));
    app.handle_key(key(KeyCode::Up));
    assert_eq!(
        app.channel_history()[0].line.split(' ').next(),
        Some("Please")
    );
    app.handle_key(key(KeyCode::Enter));
    assert!(app.chat_jump().is_some());
    draw(&mut app, &mut screen);
    assert!(app.chat_jump().is_none(), "resolved by the frame");
    assert!(app.scroll_offset() > 100, "{}", app.scroll_offset());
    let text = screen_text(&screen);
    assert!(text.contains("→ Please add a history tab"), "{text}");
    assert!(
        app.chat_flash_block(std::time::Instant::now()).is_some(),
        "lit up"
    );
    let later = std::time::Instant::now() + crate::app::apps::CHAT_FLASH;
    assert!(app.chat_flash_block(later).is_none(), "only for a moment");
}

#[test]
fn test_clicking_a_post_scrolls_the_chat_to_it() {
    let (_dir, mut app, _rx) = channel_on_history(80);
    let mut screen = screen();
    draw(&mut app, &mut screen);
    let rows = app.channel_history_rows_area().expect("the rows are drawn");
    app.click_channel_history(rows.x + 4, rows.y + 1);
    assert_eq!(app.selected_channel_history(), 1);
    draw(&mut app, &mut screen);
    assert!(app.scroll_offset() > 100);
    let text = screen_text(&screen);
    assert!(
        text.contains("@RE36 review it  "),
        "the chat shows the post:\n{text}"
    );
}

#[test]
fn test_picking_a_message_from_a_members_chat_goes_back_to_the_channels() {
    let (_dir, mut app, mut rx) = channel_on_history(0);
    app.process_command(UiCommand::SetActiveAgent("re".to_string()));
    assert!(app.member_chat().is_some());
    while rx.try_recv().is_ok() {}
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(switch_request(&mut rx).as_deref(), Some("co"));
    assert_eq!(app.active_agent_id(), "co");
    assert!(app.chat_jump().is_some(), "waits for the channel's chat");
}
