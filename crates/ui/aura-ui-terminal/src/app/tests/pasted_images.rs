use super::*;

// ========================================================================
// Pasted images
// ========================================================================

const PNG: &[u8] = b"\x89PNG\r\n\x1a\nimage";

fn paste_image(app: &mut App) {
    app.paste_clipboard(Ok(crate::clipboard::Clipboard::Image(PNG.to_vec())));
}

fn sent_images(rx: &mut mpsc::Receiver<UiEvent>) -> Option<(String, usize)> {
    match rx.try_recv() {
        Ok(UiEvent::UserMessage { text, images, .. }) => Some((text, images.len())),
        _ => None,
    }
}

#[test]
fn test_ctrl_v_asks_the_terminal_for_the_clipboard() {
    let mut app = App::new();
    app.handle_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::CONTROL));
    assert!(app.take_paste_request());
    assert!(!app.take_paste_request(), "once");
    assert!(app.input().is_empty(), "Ctrl+V types nothing");
}

#[test]
fn test_pasted_image_shows_a_chip_and_is_sent_with_the_message() {
    let (tx, mut rx) = mpsc::channel(8);
    let mut app = App::new().with_event_sender(tx);
    type_keys(&mut app, "what is");
    paste_image(&mut app);
    assert_eq!(app.input(), "what is [image 1] ");
    assert_eq!(app.attached_images(), 1);
    type_keys(&mut app, "this?");
    app.handle_key(KeyEvent::from(KeyCode::Enter));

    assert_eq!(
        sent_images(&mut rx),
        Some(("what is [image 1] this?".to_string(), 1))
    );
    let shown: Vec<&str> = app.messages().iter().map(Message::content).collect();
    assert_eq!(shown, ["what is [image 1] this?"]);
    assert_eq!(app.attached_images(), 0, "the next message starts empty");
}

#[test]
fn test_deleting_the_chip_drops_the_image() {
    let (tx, mut rx) = mpsc::channel(8);
    let mut app = App::new().with_event_sender(tx);
    paste_image(&mut app);
    paste_image(&mut app);
    assert_eq!(app.input(), "[image 1] [image 2] ");
    app.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
    paste_image(&mut app);
    assert_eq!(
        app.input(),
        "[image 1] ",
        "numbering restarts once chips are gone"
    );
    app.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
    type_keys(&mut app, "no picture");
    app.handle_key(KeyEvent::from(KeyCode::Enter));
    assert_eq!(sent_images(&mut rx), Some(("no picture".to_string(), 0)));
}

#[test]
fn test_images_that_are_too_big_or_too_many_are_refused() {
    let mut app = App::new();
    let mut huge = PNG.to_vec();
    huge.resize(images::MAX_IMAGE_BYTES + 1, 0);
    app.paste_clipboard(Ok(crate::clipboard::Clipboard::Image(huge)));
    assert!(app.input().is_empty());
    let (problem, _) = app.notification().unwrap();
    assert!(problem.contains("the limit is 3.75 MiB"), "{problem}");

    app.paste_clipboard(Ok(crate::clipboard::Clipboard::Image(b"GIF8".to_vec())));
    assert!(app
        .notification()
        .unwrap()
        .0
        .contains("PNG, JPEG, GIF and WebP"));

    for _ in 0..images::MAX_IMAGES {
        paste_image(&mut app);
    }
    paste_image(&mut app);
    assert_eq!(app.attached_images(), images::MAX_IMAGES);
    assert!(app.notification().unwrap().0.contains("at most 5 images"));
}

#[test]
fn test_images_for_a_busy_agent_are_queued() {
    let (tx, mut rx) = mpsc::channel(8);
    let mut app = App::new().with_event_sender(tx);
    app.state = AppState::Processing;
    paste_image(&mut app);
    app.handle_key(KeyEvent::from(KeyCode::Enter));
    assert!(app
        .notification()
        .unwrap()
        .0
        .starts_with("Queued: the agent gets the images"));
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::UserMessage { shown: false, images, .. }) if images.len() == 1
    ));
}

#[test]
fn test_pasting_an_image_path_attaches_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("shot.png");
    std::fs::write(&path, PNG).unwrap();
    let mut app = App::new();
    app.handle_paste(&format!("'{}'", path.display()));
    assert_eq!(app.input(), "[image 1] ");
    assert_eq!(app.attached_images(), 1);
}

#[test]
fn test_pasted_text_keeps_its_newlines_instead_of_sending() {
    let (tx, mut rx) = mpsc::channel(8);
    let mut app = App::new().with_event_sender(tx);
    app.handle_paste("line one\r\nline two");
    assert_eq!(app.input(), "line one\nline two");
    assert!(rx.try_recv().is_err(), "nothing was sent");

    app.state = AppState::LinkCode;
    app.input.text.clear();
    app.input.cursor_pos = 0;
    app.handle_paste("  code-123\n");
    assert_eq!(app.input(), "code-123");
}

#[test]
fn test_clipboard_errors_are_reported() {
    let mut app = App::new();
    app.paste_clipboard(Err(std::io::Error::other("no tool")));
    assert_eq!(
        app.notification().unwrap().0,
        "Cannot read the clipboard: no tool"
    );
    app.paste_clipboard(Ok(crate::clipboard::Clipboard::Text("hi".into())));
    assert_eq!(app.input(), "hi");
}
