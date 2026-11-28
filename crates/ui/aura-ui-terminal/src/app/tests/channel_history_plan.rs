//! HISTORY lists the user's `/plan` requests, which went to a planner's
//! own chat, and a pick opens that chat at the request.

use super::*;
use crate::app::apps::PLANNER_GONE;
use crate::app::HistoryKind;
use crate::events::{
    AgentSummary, ChannelPostAuthor, ChannelPostView, ChannelSummary, MessageRole as Role,
};

/// 2024-01-02, long before today.
const DAY: u64 = 1_704_200_000_000;

const ASKED: &str = "Design the login flow with a magic link and a fallback password form, \
                     then list what the backend needs";

fn post(seq: u64, text: &str, ms: u64, planner: Option<&str>) -> ChannelPostView {
    ChannelPostView {
        seq,
        author: ChannelPostAuthor::User,
        text: text.to_string(),
        at_ms: DAY + ms,
        planner: planner.map(str::to_string),
        ..ChannelPostView::default()
    }
}

fn at(role: Role, text: &str, ms: u64) -> MessageData {
    MessageData {
        timestamp_ms: Some(DAY + ms),
        ..MessageData::new(role, text)
    }
}

/// `#zode`, coordinated by `co`, with planner `pl`, open on the
/// coordinator's chat: a message typed to it, a post to `pl` and a
/// `/plan` request `pl` plans, on the HISTORY tab. `rx` sees what the app
/// asks of the session.
fn plan_app() -> (App, mpsc::Receiver<UiEvent>) {
    let (tx, rx) = mpsc::channel(64);
    let mut app = App::new().with_event_sender(tx);
    let member = |id: &str| AgentSummary {
        id: id.to_string(),
        name: id.to_string(),
        ..AgentSummary::default()
    };
    app.process_command(UiCommand::SetChannels(vec![ChannelSummary {
        id: "c1".to_string(),
        slug: "zode".to_string(),
        coordinator_id: "co".to_string(),
        members: vec![member("co"), member("pl")],
        ..ChannelSummary::default()
    }]));
    app.process_command(UiCommand::SetActiveAgent("co".to_string()));
    app.open_channel_by_id("c1");
    app.process_command(UiCommand::SetActiveAgent("co".to_string()));
    let history = vec![
        at(Role::User, "ship it", 1_000),
        at(Role::Assistant, "Shipping.", 1_500),
    ];
    app.process_command(UiCommand::LoadConversation(history));
    app.process_command(UiCommand::SetChannelPosts {
        channel_id: "c1".to_string(),
        posts: vec![
            post(1, "@pl look at auth", 2_000, None),
            post(2, ASKED, 3_000, Some("pl")),
        ],
    });
    app.select_pane_tab(PaneTab::Side(SidePanelTab::ChannelHistory));
    app.focus = PanelFocus::Sandbox;
    let mut rx = rx;
    while rx.try_recv().is_ok() {}
    (app, rx)
}

fn listed(app: &App) -> Vec<(HistoryKind, String)> {
    app.channel_history()
        .into_iter()
        .map(|item| (item.kind, item.text))
        .collect()
}

#[test]
fn test_history_lists_a_plan_request_by_time_but_the_chat_does_not() {
    let (mut app, _rx) = plan_app();
    assert_eq!(
        listed(&app),
        [
            (HistoryKind::Plan, ASKED.to_string()),
            (HistoryKind::Post, "@pl look at auth".to_string()),
            (HistoryKind::Typed, "ship it".to_string()),
        ]
    );
    assert_eq!(
        app.channel_posts("c1").len(),
        1,
        "the chat's posts leave it out"
    );
    let ((_, posts), _, _) = app.chat_line_cache();
    assert!(
        posts.iter().all(|p| p.planner.is_none()),
        "never drawn in the chat"
    );
}

#[test]
fn test_a_plan_request_seen_twice_is_listed_once_and_never_merged_with_typed_text() {
    let (mut app, _rx) = plan_app();
    // The same request again, live: replaced by its `seq`.
    let again = post(2, ASKED, 3_000, Some("pl"));
    app.process_command(UiCommand::ChannelPost {
        channel_id: "c1".to_string(),
        post: again,
    });
    let plans = |app: &App| {
        listed(app)
            .iter()
            .filter(|(kind, _)| *kind == HistoryKind::Plan)
            .count()
    };
    assert_eq!(plans(&app), 1);
    // The same words typed to the coordinator a moment later are their
    // own message: a request never reaches the coordinator.
    app.process_command(UiCommand::LoadConversation(vec![at(
        Role::User,
        ASKED,
        3_100,
    )]));
    assert_eq!(plans(&app), 1);
    let typed = listed(&app)
        .iter()
        .filter(|(kind, text)| *kind == HistoryKind::Typed && text == ASKED)
        .count();
    assert_eq!(typed, 1);
}

#[test]
fn test_a_long_plan_request_shows_its_summary_by_its_seq() {
    let (mut app, _rx) = plan_app();
    app.process_command(UiCommand::SetPostSummaries {
        channel_id: "c1".to_string(),
        summaries: vec![(2, "Plan the magic-link login".to_string())],
    });
    let item = &app.channel_history()[0];
    assert_eq!(item.summary.as_deref(), Some("Plan the magic-link login"));
    let mut screen = ratatui::Terminal::new(ratatui::backend::TestBackend::new(160, 40)).unwrap();
    draw(&mut app, &mut screen);
    let text = screen_text(&screen);
    assert!(text.contains("Plan the magic"), "{text}");
}

#[test]
fn test_enter_on_a_plan_request_opens_the_planners_chat_at_it() {
    let (mut app, mut rx) = plan_app();
    let mut screen = ratatui::Terminal::new(ratatui::backend::TestBackend::new(140, 40)).unwrap();
    draw(&mut app, &mut screen);
    app.handle_key(key(KeyCode::Home));
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(
        app.active_agent_id(),
        "pl",
        "the planner's chat is on screen"
    );
    assert_eq!(app.member_chat().map(|m| m.id.as_str()), Some("pl"));
    assert!(app.channel_open(), "the channel stays open");
    let mut asked = Vec::new();
    while let Ok(event) = rx.try_recv() {
        asked.push(event);
    }
    assert!(
        asked
            .iter()
            .any(|e| matches!(e, UiEvent::SelectAgent(id) | UiEvent::FocusAgent(id) if id == "pl")),
        "{asked:?}"
    );
    assert!(app.chat_jumping(), "waiting for the planner's chat");

    // Its history comes: the request, then a long plan after it.
    let mut history = vec![at(Role::User, ASKED, 3_050)];
    history.extend((0..80).map(|n| at(Role::Assistant, &format!("step {n}"), 4_000 + n)));
    app.process_command(UiCommand::LoadConversation(history));
    draw(&mut app, &mut screen);
    draw(&mut app, &mut screen);
    let text = screen_text(&screen);
    assert!(!text.contains("step 79"), "scrolled back to it:\n{text}");
    assert!(text.contains("Design the login flow"), "{text}");
    assert!(app.chat_flash().is_some(), "and lit");
}

#[test]
fn test_a_request_whose_planner_left_says_so_and_stays() {
    let (mut app, _rx) = plan_app();
    app.process_command(UiCommand::SetChannelPosts {
        channel_id: "c1".to_string(),
        posts: vec![post(3, "/plan gone", 5_000, Some("zz"))],
    });
    app.handle_key(key(KeyCode::Home));
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.active_agent_id(), "co");
    assert!(!app.chat_jumping());
    let notice = app
        .notifications
        .current
        .as_ref()
        .map(|(text, _)| text.as_str());
    assert_eq!(notice, Some(PLANNER_GONE));
}

/// Every row of `screen` as text.
fn screen_text(screen: &ratatui::Terminal<ratatui::backend::TestBackend>) -> String {
    let buffer = screen.backend().buffer();
    (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}
