//! The HISTORY side tab of a channel: the user's messages, newest first,
//! each one summarized when long and a jump back to it in the chat.

use super::channels::channels_app;
use super::*;
use crate::app::{PaneTab, PanelFocus, SidePanelTab};
use crate::events::{ChannelPostAuthor, ChannelPostView, MessageData, UiCommand};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::backend::TestBackend;

const LONG: &str = "Please make every link in the footer open in a new tab, and while you \
                    are there check the privacy link points at the new policy page as well";

fn post(seq: u64, author: ChannelPostAuthor, text: &str) -> ChannelPostView {
    ChannelPostView {
        seq,
        author,
        text: text.to_string(),
        // 2024-01-02, long before today: shown with its date.
        at_ms: 1_704_200_000_000 + seq * 60_000,
        ..ChannelPostView::default()
    }
}

fn worker() -> ChannelPostAuthor {
    ChannelPostAuthor::Agent {
        id: "wk".to_string(),
        code: Some("WK1".to_string()),
        name: "Worker".to_string(),
    }
}

/// `#zode` open on its coordinator's chat, with `posts` and a long
/// conversation before them, on the HISTORY tab.
fn history_app(posts: Vec<ChannelPostView>) -> crate::App {
    let mut app = channels_app();
    app.process_command(UiCommand::SetActiveAgent("co".to_string()));
    app.open_channel_by_id("c1");
    let filler: Vec<MessageData> = (0..80)
        .map(|n| MessageData {
            // After every post: the posts are far back in the chat.
            timestamp_ms: Some(1_704_300_000_000 + n),
            ..MessageData::new(
                crate::components::MessageRole::Assistant,
                format!("note {n}"),
            )
        })
        .collect();
    app.process_command(UiCommand::LoadConversation(filler));
    app.process_command(UiCommand::SetChannelPosts {
        channel_id: "c1".to_string(),
        posts,
    });
    app.select_pane_tab(PaneTab::Side(SidePanelTab::ChannelHistory));
    app
}

fn three_posts() -> Vec<ChannelPostView> {
    vec![
        post(1, ChannelPostAuthor::User, "Kick off the footer work"),
        post(2, worker(), "On it, reading the footer"),
        post(3, ChannelPostAuthor::User, LONG),
    ]
}

fn row_with(rows: &[String], needle: &str) -> usize {
    rows.iter()
        .position(|row| row.contains(needle))
        .unwrap_or_else(|| panic!("no {needle:?} in:\n{}", rows.join("\n")))
}

/// The side panel's column of `rows`, from the column of its tab bar.
fn panel(rows: &[String]) -> Vec<String> {
    let (bar, _) = header_bar(rows, "INFO");
    // The panel's border is left of MEMBERS, whatever its label's width.
    let at = char_column(&rows[bar], "INFO").saturating_sub(10);
    rows.iter()
        .map(|row| row.chars().skip(at).collect())
        .collect()
}

#[test]
fn test_history_lists_only_the_users_messages_newest_first_with_their_time() {
    let mut app = history_app(three_posts());
    let rows = panel(&render_rows_sized(&mut app, 160, 50));
    let heading = row_with(&rows, "Your messages · 2");
    let newest = row_with(&rows, "Please make every link");
    let oldest = row_with(&rows, "Kick off the footer work");
    assert!(heading < newest && newest < oldest, "{}", rows.join("\n"));
    assert!(
        rows[oldest + 1].contains("01-02"),
        "the date of an older day: {}",
        rows[oldest + 1]
    );
    assert!(
        !rows.iter().any(|row| row.contains("On it, reading")),
        "agents' posts aren't listed"
    );
}

#[test]
fn test_a_long_message_is_cut_at_a_word_until_its_summary_comes() {
    let mut app = history_app(three_posts());
    let rows = panel(&render_rows_sized(&mut app, 160, 50));
    let row = &rows[row_with(&rows, "Please make every link")];
    assert!(row.contains('…'), "{row}");
    assert!(!row.contains("policy page as well"), "{row}");

    app.process_command(UiCommand::SetPostSummaries {
        channel_id: "c1".to_string(),
        summaries: vec![(3, "Footer links open in new tabs".to_string())],
    });
    let rows = panel(&render_rows_sized(&mut app, 160, 50));
    row_with(&rows, "Footer links open in new tabs");
    // A short one shows whole, and a summary of it would be ignored.
    row_with(&rows, "Kick off the footer work");
}

#[test]
fn test_no_messages_says_so() {
    let mut app = history_app(vec![post(1, worker(), "Hello")]);
    let screen = render_rows_sized(&mut app, 160, 50).join("\n");
    assert!(screen.contains("No messages yet."), "{screen}");
}

#[test]
fn test_cutting_keeps_whole_words() {
    use super::super::channel_history::cut_at_word;
    assert_eq!(cut_at_word("short", 10), "short");
    assert_eq!(
        cut_at_word("make the footer links open", 16),
        "make the footer…"
    );
    assert_eq!(cut_at_word("supercalifragilistic", 8), "superca…");
}

/// How many times `needle` is on screen.
fn chat_count(rows: &[String], needle: &str) -> usize {
    rows.iter().map(|row| row.matches(needle).count()).sum()
}

/// Whether `needle` is on any of `rows`.
fn chat_shows(rows: &[String], needle: &str) -> bool {
    rows.iter().any(|row| row.contains(needle))
}

/// Draw `app` on a 160 by 40 screen.
fn draw(app: &mut crate::App) -> ratatui::buffer::Buffer {
    let mut terminal = ratatui::Terminal::new(TestBackend::new(160, 40)).unwrap();
    terminal
        .draw(|frame| super::super::render(frame, app, &test_theme()))
        .unwrap();
    terminal.backend().buffer().clone()
}

#[test]
fn test_enter_jumps_the_chat_back_to_the_message_and_lights_it() {
    let mut app = history_app(three_posts());
    app.focus = PanelFocus::Sandbox;
    let rows = render_rows_sized(&mut app, 160, 40);
    assert!(chat_shows(&rows, "note 79"), "the chat starts at its end");
    assert_eq!(
        chat_count(&rows, "Kick off the footer work"),
        1,
        "only the list has it"
    );

    // The oldest is the second item.
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(
        app.focus(),
        PanelFocus::Sandbox,
        "the focus stays on the list"
    );
    let buffer = draw(&mut app);
    let rows: Vec<String> = (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect()
        })
        .collect();
    assert!(
        !chat_shows(&rows, "note 79"),
        "the chat scrolled back:\n{}",
        rows.join("\n")
    );
    // Lit: the message's text cell in the chat takes the selection colour.
    let y = rows
        .iter()
        .enumerate()
        .filter(|(_, row)| row.contains("Kick off the footer work"))
        .map(|(y, _)| y)
        .min_by_key(|&y| char_column(&rows[y], "Kick off the footer work"))
        .unwrap();
    let x = char_column(&rows[y], "Kick off the footer work");
    let cell = &buffer[(u16::try_from(x).unwrap(), u16::try_from(y).unwrap())];
    assert_eq!(
        cell.bg,
        test_theme().colors.selection,
        "row {y}: {}",
        rows[y]
    );
    assert!(app.chat_flash().is_some());
}

#[test]
fn test_a_click_on_an_item_jumps_and_the_wheel_moves_only_the_list() {
    let posts: Vec<ChannelPostView> = (1..=30)
        .map(|seq| {
            post(
                seq,
                ChannelPostAuthor::User,
                &format!("message number {seq}"),
            )
        })
        .collect();
    let mut app = history_app(posts);
    app.focus = PanelFocus::Sandbox;
    let rows = render_rows_sized(&mut app, 160, 40);
    let y = row_with(&rows, "message number 29");
    let x = char_column(&rows[y], "message number 29");
    let (x, y) = (u16::try_from(x).unwrap(), u16::try_from(y).unwrap());
    mouse_at(&mut app, MouseEventKind::Down(MouseButton::Left), (x, y));
    assert_eq!(app.selected_history_item(), 1);
    assert!(app.chat_jumping(), "the jump is on its way");

    let before = app.selected_history_item();
    for _ in 0..3 {
        mouse_at(&mut app, MouseEventKind::ScrollDown, (x, y));
    }
    let rows = panel(&render_rows_sized(&mut app, 160, 40));
    assert_eq!(
        app.selected_history_item(),
        before,
        "the wheel leaves the highlight"
    );
    // Only the list's column: the chat shows the messages too.
    assert!(
        !chat_shows(&rows, "message number 30"),
        "the list scrolled:\n{}",
        rows.join("\n")
    );
}

#[test]
fn test_a_jump_from_a_members_chat_goes_back_to_the_channels() {
    let mut app = history_app(three_posts());
    // The worker's own chat is on screen.
    app.process_command(UiCommand::SetActiveAgent("wk".to_string()));
    assert!(app.member_chat().is_some());
    app.focus = PanelFocus::Sandbox;
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.chat_jumping(), "the jump waits for the channel's chat");
}
