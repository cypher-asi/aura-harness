//! The open channel's HISTORY tab: placement, what it lists, summaries
//! and the jump to a message in the chat.

use super::channel_member_chat::app_in_channel_with_events;
use super::*;
use crate::app::SidePanelTab;
use crate::events::{
    ChannelPostAuthor, ChannelPostView, MessageSummary, MESSAGE_SUMMARY_THRESHOLD,
};

type Screen = ratatui::Terminal<ratatui::backend::TestBackend>;

fn screen() -> Screen {
    ratatui::Terminal::new(ratatui::backend::TestBackend::new(140, 40)).unwrap()
}

/// A message of `role` sent at second `at` of the day.
fn said(role: MessageRole, text: &str, at: u64) -> MessageData {
    MessageData {
        timestamp_ms: Some(1_764_300_000_000 + at * 1_000),
        ..MessageData::new(role, text)
    }
}

/// `#zode` open on its coordinator's chat: `prompts` user prompts, each
/// answered, then HISTORY shown and focused. Events go to the receiver.
fn history_app(prompts: usize) -> (tempfile::TempDir, App, mpsc::Receiver<UiEvent>) {
    let (dir, mut app, mut rx) = app_in_channel_with_events("co");
    let history = (0..prompts as u64)
        .flat_map(|i| {
            [
                said(MessageRole::User, &format!("prompt {i:03}"), 2 * i),
                said(
                    MessageRole::Assistant,
                    &format!("reply {i}\n\nmore\n\nlines"),
                    2 * i + 1,
                ),
            ]
        })
        .collect();
    app.process_command(UiCommand::LoadConversation(history));
    app.show_side_tab(SidePanelTab::ChannelHistory);
    app.focus = PanelFocus::Sandbox;
    while rx.try_recv().is_ok() {}
    (dir, app, rx)
}

/// The `SummarizeMessages` events waiting in `rx`, as their texts.
fn asked(rx: &mut mpsc::Receiver<UiEvent>) -> Vec<Vec<String>> {
    let mut out = Vec::new();
    while let Ok(event) = rx.try_recv() {
        if let UiEvent::SummarizeMessages { channel_id, texts } = event {
            assert_eq!(channel_id, "c1");
            out.push(texts);
        }
    }
    out
}

#[test]
fn history_is_the_last_channel_tab_right_after_tasks() {
    let tabs = SidePanelTab::tabs(true);
    let tasks = tabs.iter().position(|&t| t == SidePanelTab::Tasks).unwrap();
    assert_eq!(tabs.get(tasks + 1), Some(&SidePanelTab::ChannelHistory));
    assert_eq!(tabs.last(), Some(&SidePanelTab::ChannelHistory));
    assert!(!SidePanelTab::tabs(false).contains(&SidePanelTab::ChannelHistory));
    assert!(SidePanelTab::ChannelHistory.is_channel_tab());
    assert_eq!(SidePanelTab::Tasks.next(true), SidePanelTab::ChannelHistory);
    assert_eq!(
        SidePanelTab::ChannelHistory.next(true),
        SidePanelTab::ChannelMembers
    );
    assert_eq!(
        SidePanelTab::ChannelMembers.prev(true),
        SidePanelTab::ChannelHistory
    );

    let (_dir, mut app, _rx) = history_app(2);
    let mut screen = screen();
    draw(&mut app, &mut screen);
    let (tasks_x, bar) = cell_of(&screen, "TASKS").expect("TASKS on the bar");
    // `HISTORY`, or `HIS` on a bar too narrow for the full labels.
    let (history_x, history_bar) = cell_of(&screen, "HIS").expect("HISTORY on the bar");
    assert_eq!(bar, history_bar);
    assert!(history_x > tasks_x);
}

#[test]
fn history_lists_the_users_messages_newest_first() {
    let (_dir, mut app, _rx) = history_app(3);
    app.process_command(UiCommand::SetChannelPosts {
        channel_id: "c1".to_string(),
        posts: vec![
            ChannelPostView {
                seq: 1,
                author: ChannelPostAuthor::User,
                text: "@RE36 take a look".to_string(),
                at_ms: 1_764_300_000_000 + 3_000,
                ..ChannelPostView::default()
            },
            ChannelPostView {
                seq: 2,
                author: ChannelPostAuthor::Agent {
                    id: "re".to_string(),
                    code: Some("RE36".to_string()),
                    name: "Reviewer".to_string(),
                },
                text: "agent post".to_string(),
                at_ms: 1_764_300_000_000 + 3_500,
                ..ChannelPostView::default()
            },
        ],
    });
    let texts: Vec<String> = app
        .channel_history()
        .iter()
        .map(|e| e.text.clone())
        .collect();
    assert_eq!(
        texts,
        [
            "prompt 002",
            "@RE36 take a look",
            "prompt 001",
            "prompt 000"
        ]
    );
    let mut screen = screen();
    draw(&mut app, &mut screen);
    let newest = side_cell(&app, &screen, "prompt 002").map(|(_, y)| y);
    let oldest = side_cell(&app, &screen, "prompt 000").map(|(_, y)| y);
    assert!(newest.is_some() && newest < oldest, "newest first");
    assert!(side_cell(&app, &screen, "agent post").is_none());
}

#[test]
fn long_messages_are_cut_then_summarized_and_asked_for_once() {
    let (_dir, mut app, mut rx) = history_app(0);
    let exact = format!("short {}", "x".repeat(MESSAGE_SUMMARY_THRESHOLD - 6));
    let long = format!("{} tail", "word ".repeat(17));
    app.process_command(UiCommand::LoadConversation(vec![
        said(MessageRole::User, &exact, 1),
        said(MessageRole::User, &long, 2),
    ]));
    let entries = app.channel_history().to_vec();
    assert_eq!(app.channel_history_label(&entries[1]), exact);
    let cut = app.channel_history_label(&entries[0]);
    assert!(cut.ends_with('…') && cut.len() < long.len(), "{cut}");

    app.tick();
    assert_eq!(asked(&mut rx), [vec![long.clone()]], "only the long one");
    app.tick();
    assert!(asked(&mut rx).is_empty(), "asked once");

    app.process_command(UiCommand::MessageSummaries(vec![MessageSummary {
        text: long.clone(),
        summary: "Repeat the word".to_string(),
    }]));
    assert_eq!(app.channel_history_label(&entries[0]), "Repeat the word");
}

#[test]
fn summaries_are_only_asked_for_while_history_shows() {
    let (_dir, mut app, mut rx) = history_app(0);
    let long = "word ".repeat(30);
    app.process_command(UiCommand::LoadConversation(vec![said(
        MessageRole::User,
        &long,
        1,
    )]));
    app.show_side_tab(SidePanelTab::ChannelMembers);
    app.tick();
    assert!(asked(&mut rx).is_empty());
    app.show_side_tab(SidePanelTab::ChannelHistory);
    app.tick();
    assert_eq!(asked(&mut rx).len(), 1);
}

/// Whether row `y` of the chat is lit by a jump.
fn is_lit(app: &App, screen: &Screen, y: u16) -> bool {
    let theme = crate::Theme::by_name("cyber");
    let chat = app.areas.chat.expect("the chat panel");
    let buffer = screen.backend().buffer();
    (chat.x..chat.right()).any(|x| buffer[(x, y)].bg == theme.colors.surface_hover)
}

#[test]
fn enter_scrolls_the_chat_far_back_to_the_message_and_lights_it() {
    let (_dir, mut app, _rx) = history_app(300);
    let mut screen = screen();
    draw(&mut app, &mut screen);
    assert!(
        chat_cell(&app, &screen, "prompt 003").is_none(),
        "far back at first"
    );
    // Newest first: the 300th row from the top is the oldest, prompt 0;
    // three up from it is prompt 3.
    app.handle_key(key(KeyCode::End));
    for _ in 0..3 {
        app.handle_key(key(KeyCode::Up));
    }
    assert_eq!(app.selected_channel_history(), 296);
    app.handle_key(key(KeyCode::Enter));
    draw(&mut app, &mut screen);
    let (_, y) = chat_cell(&app, &screen, "prompt 003").expect("prompt 3 scrolled into view");
    assert!(is_lit(&app, &screen, y), "the message is lit");
    assert!(app.scroll_offset() > 0, "scrolled up from the end");

    app.age_history_flash(crate::app::JUMP_HIGHLIGHT);
    draw(&mut app, &mut screen);
    assert!(!is_lit(&app, &screen, y), "the light goes out");
}

/// Where `text` is drawn in the chat panel.
fn chat_cell(app: &App, screen: &Screen, text: &str) -> Option<(u16, u16)> {
    let chat = app.areas.chat.expect("the chat panel");
    let buffer = screen.backend().buffer();
    (chat.y..chat.bottom()).find_map(|y| {
        let row: String = (chat.x..chat.right())
            .map(|x| buffer[(x, y)].symbol())
            .collect();
        let at = row.find(text)?;
        Some((chat.x + u16::try_from(row[..at].chars().count()).ok()?, y))
    })
}

/// Where `text` is drawn in the side panel.
fn side_cell(app: &App, screen: &Screen, text: &str) -> Option<(u16, u16)> {
    let side = app.areas.sandbox.expect("the side panel");
    cell_of(screen, text).filter(|&(x, _)| x >= side.x)
}

#[test]
fn a_click_jumps_from_a_members_chat_to_the_channels() {
    let (_dir, mut app, _rx) = history_app(40);
    let mut screen = screen();
    draw(&mut app, &mut screen);
    let (x, y) = side_cell(&app, &screen, "prompt 035").expect("row of prompt 35");
    app.process_command(UiCommand::SetActiveAgent("re".to_string()));
    assert!(app.member_chat().is_some());
    click_at(&mut app, (x, y));
    assert_eq!(app.active_agent_id(), "co", "back on the channel's chat");
    draw(&mut app, &mut screen);
    let (_, row) = chat_cell(&app, &screen, "prompt 035").expect("prompt 35 in the chat");
    assert!(is_lit(&app, &screen, row));
}

#[test]
fn the_wheel_scrolls_history_without_moving_the_highlight() {
    let (_dir, mut app, _rx) = history_app(100);
    let mut screen = screen();
    draw(&mut app, &mut screen);
    assert_eq!(app.selected_channel_history(), 0);
    let (x, y) = side_cell(&app, &screen, "prompt 099").expect("newest row");
    app.handle_mouse(mouse(crossterm::event::MouseEventKind::ScrollDown, x, y));
    app.handle_mouse(mouse(crossterm::event::MouseEventKind::ScrollDown, x, y));
    draw(&mut app, &mut screen);
    assert_eq!(app.selected_channel_history(), 0, "highlight stays");
    assert!(
        side_cell(&app, &screen, "prompt 099").is_none(),
        "rows scrolled"
    );
    app.handle_key(key(KeyCode::Down));
    draw(&mut app, &mut screen);
    assert_eq!(app.selected_channel_history(), 1);
    assert!(
        side_cell(&app, &screen, "prompt 098").is_some(),
        "back on the highlight"
    );
}
