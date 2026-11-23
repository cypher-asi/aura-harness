use super::*;

fn text(text: &str) -> ContentBlock {
    ContentBlock::Text {
        text: text.to_string(),
    }
}

fn tool_result() -> ContentBlock {
    ContentBlock::ToolResult {
        tool_use_id: "t".to_string(),
        content: "ok".into(),
        is_error: false,
    }
}

fn user(content: Vec<ContentBlock>) -> Message {
    Message {
        role: Role::User,
        content,
    }
}

#[test]
fn prompts_skip_tool_results_and_empty_messages() {
    let messages = vec![
        Message::user("fix the test"),
        Message::assistant("on it"),
        user(vec![tool_result()]),
        Message::user("  "),
        Message::user("now the docs"),
    ];
    assert_eq!(
        prompts(&messages, |_| false),
        vec!["fix the test".to_string(), "now the docs".to_string()]
    );
}

#[test]
fn prompts_are_the_named_ones_when_any_is_named() {
    let messages = vec![
        Message::user("fix the test"),
        Message::assistant("on it"),
        user(vec![tool_result()]),
        Message::user("  "),
        user(vec![text("now "), text("the docs")]),
        user(vec![text("see"), tool_result()]),
        Message::user("last one"),
    ];
    let typed = ["fix the test", "now the docs", "last one"];
    assert_eq!(prompts(&messages, |_| false), typed);
    assert_eq!(
        prompts(&messages, |text| text == "on it"),
        typed,
        "only what the user typed can be named"
    );
    assert_eq!(
        prompts(&messages, |text| text == "now the docs"),
        ["now the docs"]
    );
    assert_eq!(
        prompts(&messages, |text| text != "now the docs"),
        ["fix the test", "last one"]
    );
}

#[test]
fn titles_are_the_first_line_cut_to_sixty_characters() {
    let x = |n: usize| "x".repeat(n);
    let cases = [
        (String::new(), String::new()),
        ("   ".to_string(), String::new()),
        ("short".to_string(), "short".to_string()),
        (
            "  padded line  \nsecond".to_string(),
            "padded line".to_string(),
        ),
        ("\n\nafter blank lines".to_string(), String::new()),
        ("line\r\nnext".to_string(), "line".to_string()),
        (x(60), x(60)),
        (x(61), format!("{}…", x(60))),
        ("é".repeat(61), format!("{}…", "é".repeat(60))),
        (format!("{} y", x(59)), format!("{} …", x(59))),
    ];
    for (prompt, expected) in cases {
        assert_eq!(title(&prompt), expected, "{prompt:?}");
    }
}

#[test]
fn paths_are_made_relative_to_the_workspace() {
    let ws = Path::new("/work/project");
    assert_eq!(
        relative(Path::new("/work/project/src/lib.rs"), ws),
        Some(PathBuf::from("src/lib.rs"))
    );
    assert_eq!(relative(Path::new("/elsewhere/x"), ws), None);
    assert_eq!(
        relative(Path::new("src/main.rs"), ws),
        Some(PathBuf::from("src/main.rs"))
    );
}
