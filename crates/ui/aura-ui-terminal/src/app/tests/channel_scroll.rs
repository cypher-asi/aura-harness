use super::channel_member_chat::app_in_channel_with_events;
use super::*;

type Screen = ratatui::Terminal<ratatui::backend::TestBackend>;

fn screen() -> Screen {
    ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap()
}

/// The whole history of `agent`: `count` replies.
fn load_replies(app: &mut App, agent: &str, count: usize) {
    let history = (0..count)
        .map(|i| MessageData::new(MessageRole::Assistant, format!("{agent} reply {i}")))
        .collect();
    app.process_command(for_agent(agent, UiCommand::LoadConversation(history)));
}

/// A reply `agent` streams, on screen or not.
fn stream_reply(app: &mut App, agent: &str, text: &str) {
    for command in [
        UiCommand::TurnStarted { prompt: None },
        UiCommand::StartStreaming,
        UiCommand::AppendText(text.to_string()),
        UiCommand::FinishStreaming,
        UiCommand::Complete,
    ] {
        app.process_command(for_agent(agent, command));
    }
}

/// `count` posts in `#zode`, which its chat shows among its
/// coordinator's messages.
fn post_in_zode(app: &mut App, count: u64) {
    use crate::events::{ChannelPostAuthor, ChannelPostView};
    let posts = (1..=count)
        .map(|seq| ChannelPostView {
            seq,
            author: ChannelPostAuthor::User,
            text: format!("post {seq}"),
            at_ms: seq,
            ..ChannelPostView::default()
        })
        .collect();
    app.process_command(UiCommand::SetChannelPosts {
        channel_id: "c1".to_string(),
        posts,
    });
}

/// Scroll the chat `rows` up, as the wheel over it does, and draw it.
fn scroll_chat_up(app: &mut App, screen: &mut Screen, rows: usize) {
    app.scroll_panel_up(PanelFocus::Chat, rows);
    draw(app, screen);
}

/// Bring `tab` to the front, as clicking it does, and draw what it shows.
/// What it asks of the session is dropped, so the requests never pile up.
fn show_app(app: &mut App, rx: &mut mpsc::Receiver<UiEvent>, screen: &mut Screen, tab: AgentsTab) {
    app.show_agents_tab(tab);
    assert_eq!(app.agents_tab(), tab);
    while rx.try_recv().is_ok() {}
    draw(app, screen);
}

/// `#zode`, coordinated by `bb`, open in CHANNELS with the chat scrolled
/// up 6 rows; `aa`, which PROJECTS shows, is not in it.
fn channel_of_bb_scrolled_up() -> (App, mpsc::Receiver<UiEvent>, Screen) {
    let (mut app, mut rx) = app_with_agents();
    load_replies(&mut app, "aa", 30);
    load_replies(&mut app, "bb", 40);
    app.process_command(UiCommand::SetChannels(vec![zode("bb")]));
    let mut screen = screen();
    show_app(&mut app, &mut rx, &mut screen, AgentsTab::Channels);
    assert_eq!(app.active_agent_id(), "bb");
    scroll_chat_up(&mut app, &mut screen, 6);
    (app, rx, screen)
}

#[test]
fn scrolling_the_coordinator_in_the_agents_app_leaves_its_channel_where_it_was() {
    // `#zode` is coordinated by `co`, an instance of `coordinator`, which
    // AGENTS shows.
    let (mut app, mut rx) = app_with_agents();
    app.process_command(UiCommand::SetAgentDefinitions(vec![
        crate::events::AgentDefinitionSummary {
            name: "coordinator".to_string(),
            ..Default::default()
        },
    ]));
    let coordinator = crate::events::AgentSummary {
        definition: Some("coordinator".to_string()),
        ..agent("co", "Coordinator")
    };
    app.process_command(UiCommand::SetChannels(vec![
        crate::events::ChannelSummary {
            members: vec![coordinator],
            ..zode("co")
        },
    ]));
    load_replies(&mut app, "co", 40);
    post_in_zode(&mut app, 20);
    let mut screen = screen();
    show_app(&mut app, &mut rx, &mut screen, AgentsTab::Channels);
    assert_eq!(app.active_agent_id(), "co");
    scroll_chat_up(&mut app, &mut screen, 6);
    assert_eq!(app.scroll_offset(), 6);

    show_app(&mut app, &mut rx, &mut screen, AgentsTab::Roster);
    assert_eq!(app.active_agent_id(), "co");
    assert_eq!(app.scroll_offset(), 0, "co's own chat is at its bottom");
    scroll_chat_up(&mut app, &mut screen, 12);

    show_app(&mut app, &mut rx, &mut screen, AgentsTab::Channels);
    assert_eq!(app.scroll_offset(), 6, "the channel is where it was left");
    scroll_chat_up(&mut app, &mut screen, 3);

    show_app(&mut app, &mut rx, &mut screen, AgentsTab::Roster);
    assert_eq!(app.scroll_offset(), 12, "and co's own chat where it was");
}

#[test]
fn visiting_another_agent_leaves_the_channel_where_it_was() {
    let (mut app, mut rx, mut screen) = channel_of_bb_scrolled_up();

    show_app(&mut app, &mut rx, &mut screen, AgentsTab::Projects);
    assert_eq!(app.active_agent_id(), "aa");
    scroll_chat_up(&mut app, &mut screen, 4);

    show_app(&mut app, &mut rx, &mut screen, AgentsTab::Channels);
    assert_eq!(app.active_agent_id(), "bb");
    assert_eq!(app.scroll_offset(), 6, "the channel is where it was left");

    show_app(&mut app, &mut rx, &mut screen, AgentsTab::Projects);
    assert_eq!(app.scroll_offset(), 4, "and aa where it was");
}

#[test]
fn replies_while_away_leave_the_same_lines_of_the_channel_on_screen() {
    let (mut app, mut rx, mut screen) = channel_of_bb_scrolled_up();
    let first = app.chat.first_line;

    show_app(&mut app, &mut rx, &mut screen, AgentsTab::Projects);
    stream_reply(&mut app, "bb", "done while away");

    show_app(&mut app, &mut rx, &mut screen, AgentsTab::Channels);
    assert_eq!(app.chat.first_line, first, "the same lines on screen");
    assert!(app.scroll_offset() > 6, "the reply came in below them");
}

#[test]
fn a_members_chat_leaves_the_channel_where_it_was() {
    let (_dir, mut app, mut rx) = app_in_channel_with_events("co");
    load_replies(&mut app, "co", 40);
    load_replies(&mut app, "re", 10);
    let mut screen = screen();
    draw(&mut app, &mut screen);
    scroll_chat_up(&mut app, &mut screen, 6);

    app.open_channel_member(1);
    assert_eq!(app.member_chat().map(|m| m.id.as_str()), Some("re"));
    draw(&mut app, &mut screen);
    app.close_member_chat();
    while rx.try_recv().is_ok() {}
    assert_eq!(app.active_agent_id(), "co");
    draw(&mut app, &mut screen);
    assert_eq!(app.scroll_offset(), 6, "the channel is where it was left");
}
