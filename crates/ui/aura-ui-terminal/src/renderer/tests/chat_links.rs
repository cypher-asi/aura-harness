use super::*;
use crate::components::{Message, MessageRole};
use crossterm::event::{MouseButton, MouseEventKind};

/// Wide enough that the chat does not wrap the shorter replies.
const WIDTH: u16 = 200;

/// Whether every cell of `needle`, where it first is on `row`, is
/// underlined.
fn underlined(buffer: &ratatui::buffer::Buffer, row: u16, needle: &str) -> bool {
    let (column, _) = find_cell(buffer, row, needle).expect("on the row");
    let width = u16::try_from(needle.chars().count()).unwrap();
    (column..column + width).all(|x| buffer[(x, row)].modifier.contains(Modifier::UNDERLINED))
}

fn click(app: &mut crate::App, at: (u16, u16)) {
    mouse_at(app, MouseEventKind::Down(MouseButton::Left), at);
    mouse_at(app, MouseEventKind::Up(MouseButton::Left), at);
}

fn chat_with(role: MessageRole, text: &str) -> crate::App {
    let mut app = crate::App::new();
    app.add_message(Message::new(role, text));
    app
}

#[test]
fn a_link_is_underlined_only_while_the_pointer_is_on_it() {
    let mut app = chat_with(
        MessageRole::Assistant,
        "See [the docs](https://example.com/docs) for more.",
    );
    let buffer = render_buffer(&mut app, WIDTH);
    let row = row_of(&buffer, "the docs");
    let (column, style) = find_cell(&buffer, row, "the docs").unwrap();
    assert_eq!(style.fg, Some(test_theme().colors.warning), "link colored");
    assert!(!underlined(&buffer, row, "the docs"), "not underlined yet");

    mouse_at(&mut app, MouseEventKind::Moved, (column + 2, row));
    let buffer = render_buffer(&mut app, WIDTH);
    assert!(underlined(&buffer, row, "the docs"));
    assert!(
        underlined(&buffer, row, "https://example.com/docs"),
        "its URL too"
    );
    assert!(!underlined(&buffer, row, "See"));
    assert!(!underlined(&buffer, row, "for more."));

    let (away, _) = find_cell(&buffer, row, "See").unwrap();
    mouse_at(&mut app, MouseEventKind::Moved, (away, row));
    let buffer = render_buffer(&mut app, WIDTH);
    assert!(!underlined(&buffer, row, "the docs"));
}

#[test]
fn clicking_a_link_opens_it_and_selects_nothing() {
    let mut app = chat_with(
        MessageRole::Assistant,
        "See [the docs](https://example.com/docs) for more.",
    );
    let buffer = render_buffer(&mut app, WIDTH);
    let row = row_of(&buffer, "the docs");
    let (column, _) = find_cell(&buffer, row, "the docs").unwrap();
    click(&mut app, (column, row));
    assert_eq!(app.opened_links(), ["https://example.com/docs"]);
    assert!(app.selection().is_none());

    // The URL after the label is the same link.
    let (column, _) = find_cell(&buffer, row, "example.com").unwrap();
    click(&mut app, (column, row));
    assert_eq!(app.opened_links().len(), 2);
}

#[test]
fn a_bare_url_opens_without_the_sentence_after_it() {
    let mut app = chat_with(
        MessageRole::Assistant,
        "Open https://x.dev/a. Or run `curl https://y.dev` here.",
    );
    let buffer = render_buffer(&mut app, WIDTH);
    let row = row_of(&buffer, "x.dev/a.");
    let (column, _) = find_cell(&buffer, row, "x.dev/a.").unwrap();
    click(&mut app, (column, row));
    assert_eq!(app.opened_links(), ["https://x.dev/a"]);

    let (code, _) = find_cell(&buffer, row, "y.dev").unwrap();
    mouse_at(&mut app, MouseEventKind::Moved, (code, row));
    assert!(!app.hovers_link(), "a URL in code is code");
    click(&mut app, (code, row));
    let (dot, _) = find_cell(&buffer, row, ". Or").unwrap();
    click(&mut app, (dot, row));
    assert_eq!(app.opened_links().len(), 1);
}

#[test]
fn a_link_to_a_file_is_drawn_but_does_not_open() {
    let mut app = chat_with(
        MessageRole::Assistant,
        "See [passwords](file:///etc/passwd) and [lib](src/lib.rs).",
    );
    let buffer = render_buffer(&mut app, WIDTH);
    let row = row_of(&buffer, "passwords");
    for label in ["passwords", "lib"] {
        let (column, style) = find_cell(&buffer, row, label).unwrap();
        assert_eq!(style.fg, Some(test_theme().colors.warning));
        mouse_at(&mut app, MouseEventKind::Moved, (column, row));
        assert!(!app.hovers_link(), "{label}");
        click(&mut app, (column, row));
    }
    assert!(app.opened_links().is_empty());
}

#[test]
fn a_link_in_a_prompt_opens_where_it_is_drawn_after_the_prompt() {
    let mut app = chat_with(MessageRole::User, "check https://example.com/pr/1 please");
    let buffer = render_buffer(&mut app, WIDTH);
    let row = row_of(&buffer, "example.com/pr/1");
    let (column, _) = find_cell(&buffer, row, "/pr/1").unwrap();
    mouse_at(&mut app, MouseEventKind::Moved, (column, row));
    let buffer = render_buffer(&mut app, WIDTH);
    assert!(underlined(&buffer, row, "https://example.com/pr/1"));
    assert!(!underlined(&buffer, row, "please"));
    click(&mut app, (column, row));
    assert_eq!(app.opened_links(), ["https://example.com/pr/1"]);
}

#[test]
fn every_row_of_a_wrapped_link_underlines_together() {
    let label = (1..=40)
        .map(|i| format!("w{i}"))
        .collect::<Vec<_>>()
        .join(" ");
    let mut app = chat_with(
        MessageRole::Assistant,
        &format!("Before [{label}](https://x.dev/long) after"),
    );
    let buffer = render_buffer(&mut app, WIDTH);
    let (first, last) = (row_of(&buffer, "w1 "), row_of(&buffer, "w40"));
    assert!(last > first, "the label wraps");
    let (column, _) = find_cell(&buffer, last, "w40").unwrap();
    mouse_at(&mut app, MouseEventKind::Moved, (column, last));
    let buffer = render_buffer(&mut app, WIDTH);
    assert!(
        underlined(&buffer, first, "w1 w2"),
        "the first row lights up"
    );
    assert!(underlined(&buffer, last, "w40"));
    assert!(!underlined(&buffer, first, "Before"));

    let (column, _) = find_cell(&buffer, first, "w2").unwrap();
    click(&mut app, (column, first));
    assert_eq!(app.opened_links(), ["https://x.dev/long"]);
}

#[test]
fn hovering_one_link_does_not_underline_another() {
    let mut app = chat_with(
        MessageRole::Assistant,
        "[one](https://a.dev) and [two](https://b.dev)\n\nthen [three](https://a.dev)",
    );
    let buffer = render_buffer(&mut app, WIDTH);
    let row = row_of(&buffer, "one");
    let (column, _) = find_cell(&buffer, row, "two").unwrap();
    mouse_at(&mut app, MouseEventKind::Moved, (column, row));
    let buffer = render_buffer(&mut app, WIDTH);
    assert!(underlined(&buffer, row, "two"));
    assert!(!underlined(&buffer, row, "one"));
    let below = row_of(&buffer, "three");
    assert!(
        !underlined(&buffer, below, "three"),
        "same URL, another link"
    );
}

#[test]
fn a_link_scrolled_to_opens_where_it_now_is() {
    let mut app = chat_with(MessageRole::Assistant, "Start at https://x.dev/top");
    for i in 0..60 {
        app.add_message(Message::new(MessageRole::Assistant, &format!("reply {i}")));
    }
    scroll_chat_to_top(&mut app, "reply");
    let buffer = render_buffer(&mut app, WIDTH);
    let row = row_of(&buffer, "x.dev/top");
    let (column, _) = find_cell(&buffer, row, "x.dev/top").unwrap();
    click(&mut app, (column, row));
    assert_eq!(app.opened_links(), ["https://x.dev/top"]);
}

#[test]
fn a_link_drawn_over_does_not_open() {
    let mut app = chat_with(MessageRole::Assistant, "Go to https://x.dev now");
    let mut buffer = render_buffer(&mut app, WIDTH);
    let row = row_of(&buffer, "x.dev");
    let (column, _) = find_cell(&buffer, row, "https").unwrap();
    // A menu drawn over the link, as the slash menu is after the chat.
    buffer.set_string(column, row, "/clear  ", Style::default());
    app.drop_covered_links(&buffer);
    click(&mut app, (column, row));
    assert!(app.opened_links().is_empty());
}

#[test]
fn a_view_over_the_chat_hides_its_links() {
    let mut app = chat_with(MessageRole::Assistant, "Go to https://x.dev now");
    let buffer = render_buffer(&mut app, WIDTH);
    let row = row_of(&buffer, "x.dev");
    let (column, _) = find_cell(&buffer, row, "x.dev").unwrap();
    app.open_detail(crate::app::DetailView::Graph);
    assert!(app.detail_view().is_some(), "the graph is over the chat");
    mouse_at(&mut app, MouseEventKind::Moved, (column, row));
    assert!(!app.hovers_link());
}

#[test]
fn a_link_in_measured_prose_is_found_in_the_cells_it_was_packed_into() {
    #[allow(clippy::cast_precision_loss)]
    fn half(text: &str, _: ratatui::style::Modifier) -> f32 {
        text.chars().count() as f32 / 2.0
    }

    let mut app = crate::App::new();
    app.set_prose_measure(half);
    app.add_message(Message::new(
        MessageRole::Assistant,
        "aaaa aaaa aaaa aaaa aaaa aaaa [docs](https://x.dev) tail",
    ));
    let buffer = drawn_buffer(&mut app);
    let row = app.prose_rows()[0].clone();
    let label = (row.x..row.x + row.width)
        .find(|&x| buffer[(x, row.y)].fg == test_theme().colors.warning)
        .expect("the label is drawn in the link color");
    // On the grid the label would be 30 columns in; packed it is near 15.
    assert!(label < row.x + 20, "packed at {label}");
    mouse_at(&mut app, MouseEventKind::Moved, (label, row.y));
    assert!(app.hovers_link());
    let buffer = drawn_buffer(&mut app);
    assert!(buffer[(label, row.y)]
        .modifier
        .contains(Modifier::UNDERLINED));
    click(&mut app, (label, row.y));
    assert_eq!(app.opened_links(), ["https://x.dev"]);
}

/// The buffer the chat was drawn into, before a terminal backend reads
/// it: a packed cell holds several characters, which a backend takes for
/// a wide one.
fn drawn_buffer(app: &mut crate::App) -> ratatui::buffer::Buffer {
    let backend = ratatui::backend::TestBackend::new(WIDTH, 30);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    let frame = terminal
        .draw(|frame| super::super::render(frame, app, &test_theme()))
        .unwrap();
    frame.buffer.clone()
}
