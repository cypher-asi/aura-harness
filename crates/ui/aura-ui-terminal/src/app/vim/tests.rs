
use super::*;

const SCREEN: Screen = Screen {
    first: 0,
    shown: 10,
};

fn press(vim: &mut Vim, keys: &str, lines: &[&str]) -> Outcome {
    let mut scroll = 0;
    let mut outcome = Outcome::Stay;
    for c in keys.chars() {
        let code = match c {
            '\n' => KeyCode::Enter,
            '\x1b' => KeyCode::Esc,
            c => KeyCode::Char(c),
        };
        outcome = vim.handle_key(KeyEvent::from(code), lines, &mut scroll, SCREEN);
    }
    outcome
}

fn at(vim: &Vim) -> (usize, usize) {
    (vim.line(), vim.col())
}

#[test]
fn hjkl_move_and_keep_the_column_goal() {
    let lines = ["hello world", "hi", "goodbye world"];
    let mut vim = Vim::default();
    press(&mut vim, "8l", &lines);
    assert_eq!(at(&vim), (0, 8));
    press(&mut vim, "j", &lines);
    assert_eq!(at(&vim), (1, 1));
    press(&mut vim, "j", &lines);
    assert_eq!(at(&vim), (2, 8));
    press(&mut vim, "$k", &lines);
    assert_eq!(at(&vim), (1, 1));
    press(&mut vim, "k", &lines);
    assert_eq!(at(&vim), (0, 10));
    press(&mut vim, "0", &lines);
    assert_eq!(at(&vim), (0, 0));
    press(&mut vim, "100j", &lines);
    assert_eq!(vim.line(), 2);
}

#[test]
fn word_motions_follow_vim_word_classes() {
    let lines = ["foo.bar baz", "", "  qux(1)"];
    let mut vim = Vim::default();
    let mut stops = Vec::new();
    for _ in 0..7 {
        press(&mut vim, "w", &lines);
        stops.push(at(&vim));
    }
    assert_eq!(
        stops,
        [(0, 3), (0, 4), (0, 8), (1, 0), (2, 2), (2, 5), (2, 6)]
    );
    press(&mut vim, "gg", &lines);
    press(&mut vim, "W", &lines);
    assert_eq!(at(&vim), (0, 8));
    press(&mut vim, "e", &lines);
    assert_eq!(at(&vim), (0, 10));
    press(&mut vim, "e", &lines);
    assert_eq!(at(&vim), (2, 4));
    press(&mut vim, "b", &lines);
    assert_eq!(at(&vim), (2, 2));
    press(&mut vim, "b", &lines);
    assert_eq!(at(&vim), (1, 0));
    press(&mut vim, "b", &lines);
    assert_eq!(at(&vim), (0, 8));
    press(&mut vim, "ge", &lines);
    assert_eq!(at(&vim), (0, 6));
    press(&mut vim, "B", &lines);
    assert_eq!(at(&vim), (0, 0));
}

#[test]
fn gg_g_counts_and_line_starts() {
    let lines = ["a", "  b", "c", "  d"];
    let mut vim = Vim::default();
    press(&mut vim, "G", &lines);
    assert_eq!(at(&vim), (3, 2));
    press(&mut vim, "2G", &lines);
    assert_eq!(at(&vim), (1, 2));
    press(&mut vim, "gg", &lines);
    assert_eq!(at(&vim), (0, 0));
    press(&mut vim, "4gg", &lines);
    assert_eq!(at(&vim), (3, 2));
    press(&mut vim, ":2\n", &lines);
    assert_eq!(at(&vim), (1, 2));
    press(&mut vim, "50%", &lines);
    assert_eq!(vim.line(), 1);
}

#[test]
fn paragraphs_and_brackets() {
    let lines = ["fn a() {", "    x(1);", "}", "", "fn b() {}"];
    let mut vim = Vim::default();
    press(&mut vim, "}", &lines);
    assert_eq!(at(&vim), (3, 0));
    press(&mut vim, "}", &lines);
    assert_eq!(at(&vim), (4, 8));
    press(&mut vim, "{", &lines);
    assert_eq!(at(&vim), (3, 0));
    press(&mut vim, "gg%", &lines);
    assert_eq!(at(&vim), (0, 5));
    press(&mut vim, "%", &lines);
    assert_eq!(at(&vim), (0, 4));
    press(&mut vim, "$%", &lines);
    assert_eq!(at(&vim), (2, 0));
    press(&mut vim, "%", &lines);
    assert_eq!(at(&vim), (0, 7));
}

#[test]
fn find_char_and_repeat() {
    let lines = ["a,b,c,d"];
    let mut vim = Vim::default();
    press(&mut vim, "f,", &lines);
    assert_eq!(vim.col(), 1);
    press(&mut vim, ";", &lines);
    assert_eq!(vim.col(), 3);
    press(&mut vim, ",", &lines);
    assert_eq!(vim.col(), 1);
    press(&mut vim, "0t,", &lines);
    assert_eq!(vim.col(), 0);
    press(&mut vim, ";", &lines);
    assert_eq!(vim.col(), 2);
    press(&mut vim, "$F,", &lines);
    assert_eq!(vim.col(), 5);
    press(&mut vim, "T,", &lines);
    assert_eq!(vim.col(), 4);
    press(&mut vim, "0fz", &lines);
    assert_eq!(vim.col(), 0);
}

#[test]
fn search_wraps_and_smart_cases() {
    let lines = ["let Foo = foo;", "foo()", "bar"];
    let mut vim = Vim::default();
    press(&mut vim, "/foo\n", &lines);
    assert_eq!(at(&vim), (0, 4));
    press(&mut vim, "n", &lines);
    assert_eq!(at(&vim), (0, 10));
    press(&mut vim, "n", &lines);
    assert_eq!(at(&vim), (1, 0));
    assert_eq!(vim.message(), None);
    press(&mut vim, "n", &lines);
    assert_eq!(at(&vim), (0, 4));
    assert_eq!(vim.message(), Some("search hit BOTTOM, continuing at TOP"));
    press(&mut vim, "N", &lines);
    assert_eq!(at(&vim), (1, 0));
    press(&mut vim, "/Foo\n", &lines);
    assert_eq!(at(&vim), (0, 4));
    press(&mut vim, "?bar\n", &lines);
    assert_eq!(at(&vim), (2, 0));
    press(&mut vim, "/nope\n", &lines);
    assert_eq!(at(&vim), (2, 0));
    assert_eq!(vim.message(), Some("Pattern not found: nope"));
    assert_eq!(vim.matches("bar nope"), [(4, 8)]);
    press(&mut vim, ":noh\n", &lines);
    assert!(vim.matches("bar nope").is_empty());
}

#[test]
fn star_searches_the_whole_word_under_the_cursor() {
    let lines = ["foo foobar", "x.foo"];
    let mut vim = Vim::default();
    press(&mut vim, "l*", &lines);
    assert_eq!(at(&vim), (1, 2));
    press(&mut vim, "#", &lines);
    assert_eq!(at(&vim), (0, 0));
}

#[test]
fn closing_keys_and_escape() {
    let lines = ["a"];
    let mut vim = Vim::default();
    assert_eq!(press(&mut vim, "q", &lines), Outcome::Close);
    assert_eq!(press(&mut vim, ":q\n", &lines), Outcome::Close);
    assert_eq!(press(&mut vim, "ZZ", &lines), Outcome::Close);
    assert_eq!(press(&mut vim, "3\x1b", &lines), Outcome::Stay);
    assert_eq!(press(&mut vim, "/x\x1b", &lines), Outcome::Stay);
    assert_eq!(vim.prompt(), None);
    assert_eq!(press(&mut vim, "\x1b", &lines), Outcome::Close);
    assert_eq!(press(&mut vim, ":wat\n", &lines), Outcome::Stay);
    assert_eq!(vim.message(), Some("Not an editor command: wat"));
}

#[test]
fn page_keys_move_view_and_cursor() {
    let lines: Vec<String> = (0..100).map(|n| n.to_string()).collect();
    let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
    let mut vim = Vim::default();
    let mut scroll = 0;
    let ctrl = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
    vim.handle_key(ctrl('d'), &lines, &mut scroll, SCREEN);
    assert_eq!((scroll, vim.line()), (5, 5));
    assert_eq!(vim.take_align(), Align::View);
    vim.handle_key(ctrl('f'), &lines, &mut scroll, SCREEN);
    assert_eq!((scroll, vim.line()), (13, 13));
    vim.handle_key(ctrl('u'), &lines, &mut scroll, SCREEN);
    assert_eq!((scroll, vim.line()), (8, 8));
    vim.handle_key(ctrl('e'), &lines, &mut scroll, SCREEN);
    assert_eq!((scroll, vim.line()), (9, 8));
    vim.keep_within(9, 18, &lines);
    assert_eq!(vim.line(), 9);
    let screen = Screen {
        first: 9,
        shown: 10,
    };
    vim.handle_key(
        KeyEvent::from(KeyCode::Char('L')),
        &lines,
        &mut scroll,
        screen,
    );
    assert_eq!(vim.line(), 18);
    vim.handle_key(
        KeyEvent::from(KeyCode::Char('M')),
        &lines,
        &mut scroll,
        screen,
    );
    assert_eq!(vim.line(), 13);
    vim.handle_key(
        KeyEvent::from(KeyCode::Char('z')),
        &lines,
        &mut scroll,
        screen,
    );
    vim.handle_key(
        KeyEvent::from(KeyCode::Char('z')),
        &lines,
        &mut scroll,
        screen,
    );
    assert_eq!(vim.take_align(), Align::Center);
}
