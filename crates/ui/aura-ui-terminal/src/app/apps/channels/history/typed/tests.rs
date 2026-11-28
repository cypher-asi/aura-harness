use super::*;

fn item(kind: HistoryKind, text: &str, at_ms: u64) -> HistoryItem {
    HistoryItem {
        seq: prompt_key(text),
        kind,
        planner: None,
        text: text.to_string(),
        summary: None,
        at_ms,
    }
}

#[test]
fn only_what_the_user_typed_is_listed() {
    let messages: VecDeque<Message> = [
        Message::new(MessageRole::User, "fix the parser"),
        Message::new(MessageRole::Assistant, "Fixed it."),
        Message::new(MessageRole::System, "policy: read_file allowed"),
        Message::new(MessageRole::User, "   "),
        Message::shell("cargo test"),
        Message::new(
            MessageRole::User,
            "RE36: Parser done\n(inbox reply 7 from RE36, Parser)\nAll green.",
        ),
        Message::new(MessageRole::User, "now ship it"),
    ]
    .into_iter()
    .collect();
    let summaries = HashMap::from([(prompt_key("now ship it"), "Ship".to_string())]);
    let items = typed_items(&messages, Some(&summaries));
    let texts: Vec<&str> = items.iter().map(|i| i.text.as_str()).collect();
    assert_eq!(texts, ["fix the parser", "now ship it"]);
    assert!(items.iter().all(|i| i.kind == HistoryKind::Typed));
    assert_eq!(items[0].seq, prompt_key("fix the parser"));
    assert_eq!(items[1].summary.as_deref(), Some("Ship"), "by its key");
}

#[test]
fn posts_and_typed_messages_merge_newest_first_and_one_sent_both_ways_once() {
    let posts = vec![
        item(HistoryKind::Post, "@RE36 and @CO1 look", 9_100),
        item(HistoryKind::Post, "@RE36 only", 5_000),
    ];
    let typed = vec![
        item(HistoryKind::Typed, "first", 1_000),
        item(HistoryKind::Typed, "@RE36 and @CO1 look", 9_000),
        item(HistoryKind::Typed, "@RE36 only", 60_000),
    ];
    let merged = merge_newest_first(posts, typed);
    let merged: Vec<(HistoryKind, &str)> =
        merged.iter().map(|i| (i.kind, i.text.as_str())).collect();
    assert_eq!(
        merged,
        [
            (HistoryKind::Typed, "@RE36 only"),
            (HistoryKind::Post, "@RE36 and @CO1 look"),
            (HistoryKind::Post, "@RE36 only"),
            (HistoryKind::Typed, "first"),
        ],
        "the same text sent again much later is another message"
    );
}
