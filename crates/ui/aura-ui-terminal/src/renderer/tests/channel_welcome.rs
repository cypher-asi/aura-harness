use super::channels::channels_app;
use super::*;
use crate::components::MessageRole;
use crate::events::{ChannelPostAuthor, ChannelPostView, MessageData, UiCommand};

/// The first row of `zode` in small letters, as the banner draws it.
const ZODE_SMALL_TOP: &str = "▀▀▀█ ▄▀▀▄ █▀▀▄ █▀▀▀";

/// The first row of `zode` in large letters.
const ZODE_LARGE_TOP: &str = "████████    ████    ██████    ████████";

/// The CHANNELS app with `#zode` open on its coordinator, whose history
/// has arrived and holds `messages`.
fn channel_with(messages: Vec<MessageData>) -> crate::App {
    let mut app = coordinator_on_screen();
    app.process_command(UiCommand::LoadConversation(messages));
    app
}

/// The CHANNELS app with `#zode` open and the session having put its
/// coordinator on screen.
fn coordinator_on_screen() -> crate::App {
    let mut app = channels_app();
    app.process_command(UiCommand::SetActiveAgent("co".to_string()));
    app
}

fn screen(app: &mut crate::App, width: u16) -> String {
    render_rows(app, width).join("\n")
}

#[test]
fn test_empty_channel_shows_its_banner_and_the_coordinators_welcome() {
    let mut app = channel_with(Vec::new());
    assert!(
        app.coordinated_channel().is_some(),
        "the coordinator is on screen"
    );
    let rows = render_rows(&mut app, 160);
    let screen = rows.join("\n");
    let at = |needle: &str| {
        rows.iter()
            .position(|row| row.contains(needle))
            .unwrap_or_else(|| panic!("no {needle:?} in:\n{screen}"))
    };
    let banner = at(ZODE_LARGE_TOP);
    let hello = at("Welcome to #zode. Ship it.");
    let label = hello - 1;
    assert!(rows[label].contains("Coordinator"), "{screen}");
    let start = at("Tell me what you need and I'll hand it to an agent");
    // The banner, a blank row, the label, then the words under it.
    assert_eq!(banner + 6, label, "{screen}");
    assert_eq!(label + 1, hello, "{screen}");
    assert!(hello < start, "{screen}");
    // Banner and words start on the same column, the chat's text edge.
    let column = |row: usize, needle: &str| {
        rows[row]
            .find(needle)
            .map(|at| rows[row][..at].chars().count())
    };
    assert_eq!(column(banner, "█"), column(hello, "Welcome"), "{screen}");
}

#[test]
fn test_narrower_chat_draws_the_banner_in_half_blocks() {
    let mut app = channel_with(Vec::new());
    let screen = screen(&mut app, 100);
    assert!(screen.contains(ZODE_SMALL_TOP), "{screen}");
    assert!(!screen.contains(ZODE_LARGE_TOP), "{screen}");
    assert!(screen.contains("Welcome to #zode"), "{screen}");
}

#[test]
fn test_narrow_screen_falls_back_and_stays_in_the_panel() {
    for width in [40, 60, 80, 100, 120] {
        let mut app = channel_with(Vec::new());
        let rows = render_rows(&mut app, width);
        let screen = rows.join("\n");
        assert!(screen.contains("Welcome"), "{width}:\n{screen}");
        for row in &rows {
            assert_eq!(row.chars().count(), usize::from(width), "{width}: {row}");
        }
        // Every banner row, or the plain `#zode` that stands in for it,
        // ends inside the chat panel's right border.
        let chat_right = rows
            .iter()
            .find(|row| row.contains("Welcome"))
            .and_then(|row| {
                let chars: Vec<char> = row.chars().collect();
                let start = row[..row.find("Welcome")?].chars().count();
                chars[start..]
                    .iter()
                    .position(|&c| c == '│')
                    .map(|at| start + at)
            })
            .unwrap_or_else(|| panic!("{width}: no chat border:\n{screen}"));
        for row in rows
            .iter()
            .filter(|row| row.contains('▀') || row.contains("██"))
        {
            let chars: Vec<char> = row.chars().collect();
            assert_eq!(chars[chat_right], '│', "{width}: {row}");
        }
    }
}

#[test]
fn test_channel_with_a_post_shows_the_chat_not_the_welcome() {
    let mut app = channel_with(Vec::new());
    app.process_command(UiCommand::SetChannelPosts {
        channel_id: "c1".to_string(),
        posts: vec![ChannelPostView {
            seq: 1,
            author: ChannelPostAuthor::User,
            text: "first words".to_string(),
            ..ChannelPostView::default()
        }],
    });
    let screen = screen(&mut app, 160);
    assert!(screen.contains("first words"), "{screen}");
    assert!(!screen.contains("Welcome to #zode"), "{screen}");
    assert!(!screen.contains(ZODE_LARGE_TOP), "{screen}");
}

#[test]
fn test_coordinator_with_its_own_messages_shows_no_welcome() {
    let mut app = channel_with(vec![MessageData::new(MessageRole::User, "hello there")]);
    let screen = screen(&mut app, 160);
    assert!(screen.contains("hello there"), "{screen}");
    assert!(!screen.contains("Welcome to #zode"), "{screen}");
}

#[test]
fn test_no_welcome_outside_an_open_channel() {
    use crate::app::{AgentsTab, PaneTab};
    let mut app = crate::App::new();
    app.select_pane_tab(PaneTab::Agents(AgentsTab::Channels));
    let channels = screen(&mut app, 160);
    assert!(!channels.contains("Welcome to"), "{channels}");
    // An agent's own empty chat has none either.
    let mut app = crate::App::new();
    app.process_command(UiCommand::LoadConversation(Vec::new()));
    let agent = screen(&mut app, 160);
    assert!(!agent.contains("Welcome to"), "{agent}");
}
