use super::summaries::{cut_at_word, history_line, SUMMARY_THRESHOLD_CHARS};
use super::*;
use crate::events::{ChannelPostAuthor, ChannelPostView};
use std::collections::HashMap;

fn post(seq: u64, author: ChannelPostAuthor, text: &str, at_ms: u64) -> ChannelPostView {
    ChannelPostView {
        seq,
        author,
        text: text.to_string(),
        at_ms,
        ..ChannelPostView::default()
    }
}

fn agent() -> ChannelPostAuthor {
    ChannelPostAuthor::Agent {
        id: "re".to_string(),
        code: Some("RE36".to_string()),
        name: "Reviewer".to_string(),
    }
}

fn entry(text: &str) -> HistoryEntry {
    HistoryEntry::new(HistoryKey::Post { seq: 1 }, text, 0)
}

#[test]
fn only_the_users_typed_messages_are_prompts() {
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
    let lines: Vec<_> = prompt_entries(&messages)
        .into_iter()
        .map(|entry| entry.line)
        .collect();
    assert_eq!(lines, ["fix the parser", "now ship it"]);
}

#[test]
fn only_the_users_posts_are_listed() {
    let posts = [
        post(1, ChannelPostAuthor::User, "@RE36 review it", 10),
        post(2, agent(), "Looks good", 20),
        post(3, ChannelPostAuthor::User, "thanks\n  @RE36", 30),
    ];
    let entries = post_entries(&posts);
    let keys: Vec<_> = entries.iter().map(|entry| entry.key).collect();
    assert_eq!(
        keys,
        [HistoryKey::Post { seq: 1 }, HistoryKey::Post { seq: 3 }]
    );
    assert_eq!(entries[1].line, "thanks @RE36", "folded to one line");
    assert_eq!(entries[1].text, "thanks\n  @RE36", "kept as sent");
}

#[test]
fn prompts_and_posts_merge_by_time_and_one_sent_both_ways_is_listed_once() {
    let prompt = |text: &str, at_ms| {
        HistoryEntry::new(HistoryKey::Message { at_ms, digest: 0 }, text, at_ms)
    };
    let prompts = vec![prompt("first", 1_000), prompt("@RE36 and @CO1 look", 9_000)];
    let posts = post_entries(&[
        post(1, ChannelPostAuthor::User, "@RE36 only", 5_000),
        post(2, ChannelPostAuthor::User, "@RE36 and @CO1 look", 9_100),
        post(3, ChannelPostAuthor::User, "first", 60_000),
    ]);
    let merged: Vec<_> = merge_entries(prompts, posts)
        .into_iter()
        .map(|entry| (entry.line, entry.at_ms))
        .collect();
    assert_eq!(
        merged,
        [
            ("first".to_string(), 1_000),
            ("@RE36 only".to_string(), 5_000),
            ("@RE36 and @CO1 look".to_string(), 9_000),
            ("first".to_string(), 60_000),
        ],
        "the same text sent again later is another message"
    );
}

#[test]
fn messages_up_to_the_threshold_show_as_they_are() {
    let none = HashMap::new();
    let at_limit = "a".repeat(SUMMARY_THRESHOLD_CHARS);
    assert_eq!(history_line(&entry(&at_limit), &none), at_limit);
    let over = "a".repeat(SUMMARY_THRESHOLD_CHARS + 1);
    let over = entry(&over);
    let shown = history_line(&over, &none);
    assert!(shown.ends_with('…'), "{shown}");
    assert_eq!(shown.chars().count(), SUMMARY_THRESHOLD_CHARS);
}

#[test]
fn a_long_message_without_a_summary_is_cut_at_a_word() {
    let text = "please add a history tab to the right of tasks that shows every message \
                I sent with summaries";
    let shown = cut_at_word(&one_line(text));
    assert_eq!(
        shown,
        "please add a history tab to the right of tasks that shows every message I sent…"
    );
    assert!(shown.chars().count() <= SUMMARY_THRESHOLD_CHARS);
    let unbroken = "x".repeat(200);
    assert_eq!(
        cut_at_word(&unbroken).chars().count(),
        SUMMARY_THRESHOLD_CHARS
    );
}

#[test]
fn a_long_message_shows_its_summary_once_it_has_one() {
    let long = entry(&"word ".repeat(30));
    let key = format!("{:016x}", text_digest(&long.text));
    let summaries = HashMap::from([(key, "Repeat a word".to_string())]);
    assert_eq!(history_line(&long, &summaries), "Repeat a word");
}
