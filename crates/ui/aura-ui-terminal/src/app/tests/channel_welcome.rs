use super::*;

/// Everything on a 120 by 30 screen after a frame of `app`.
fn screen_text(app: &mut App) -> String {
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 30)).unwrap();
    draw(app, &mut terminal);
    let buffer = terminal.backend().buffer();
    (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn test_the_welcome_waits_for_the_coordinators_history() {
    let (_dir, mut app) = app_in_channel("co");
    app.switching.chat_loaded = false;
    app.switching.chat_loading_since = Some(std::time::Instant::now());
    assert!(app.chat_loading());
    let waiting = screen_text(&mut app);
    assert!(!waiting.contains("Welcome to #zode"), "{waiting}");

    // An empty history: the channel has nothing said yet.
    app.process_command(UiCommand::LoadConversation(Vec::new()));
    assert!(!app.chat_loading());
    let loaded = screen_text(&mut app);
    assert!(loaded.contains("Welcome to #zode."), "{loaded}");
    assert!(
        loaded.contains("CO1"),
        "the coordinator's code labels it:\n{loaded}"
    );
}

#[test]
fn test_a_members_own_empty_chat_has_no_welcome() {
    let (_dir, mut app) = app_in_channel("re");
    app.process_command(UiCommand::LoadConversation(Vec::new()));
    assert!(app.member_chat().is_some());
    let screen = screen_text(&mut app);
    assert!(!screen.contains("Welcome to"), "{screen}");
}
