use super::*;
use crate::events::{ChannelPostAuthor, ChannelPostView};
use chrono::DateTime;

fn at(ms: u64) -> chrono::DateTime<chrono::Utc> {
    DateTime::from_timestamp_millis(i64::try_from(ms).unwrap()).unwrap()
}

fn said(role: MessageRole, text: &str, ms: u64) -> Message {
    Message::new(role, text).with_timestamp(at(ms))
}

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

#[test]
fn only_what_the_user_sent_is_listed_newest_first() {
    let messages = [
        said(MessageRole::User, "first prompt", 1_000),
        said(MessageRole::Assistant, "a reply", 2_000),
        Message::shell("cargo test").with_timestamp(at(3_000)),
        said(MessageRole::System, "policy: read_file allowed", 4_000),
        said(MessageRole::User, "second prompt", 9_000),
    ];
    let shown: Vec<&Message> = messages.iter().collect();
    let posts = [
        post(1, ChannelPostAuthor::User, "@RE36 look at this", 5_000),
        post(2, agent(), "Looked: fine", 6_000),
    ];
    let entries = user_entries(&shown, &posts);
    let texts: Vec<&str> = entries.iter().map(|e| e.text.as_str()).collect();
    assert_eq!(
        texts,
        ["second prompt", "@RE36 look at this", "first prompt"]
    );
    assert_eq!(entries[1].target, JumpTarget::Post(1));
    assert_eq!(
        entries[0].target,
        JumpTarget::Message {
            text: "second prompt".to_string(),
            at_ms: 9_000
        }
    );
}

#[test]
fn a_post_sent_on_to_the_coordinator_is_listed_once() {
    let text = "@RE36 @coordinator both of you";
    let messages = [said(MessageRole::User, text, 5_100)];
    let shown: Vec<&Message> = messages.iter().collect();
    let posts = [post(7, ChannelPostAuthor::User, text, 5_000)];
    let entries = user_entries(&shown, &posts);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].target, JumpTarget::Post(7));
    // The same words sent again much later are a send of their own.
    let messages = [said(MessageRole::User, text, 5_000 + 10 * SAME_SEND_MS)];
    let shown: Vec<&Message> = messages.iter().collect();
    assert_eq!(user_entries(&shown, &posts).len(), 2);
}

#[test]
fn another_agents_message_delivered_as_the_users_is_not_listed() {
    use aura_ui::inbox::{header, InboxKind};
    let inbox = format!(
        "{}\n\nAll merged.",
        header(InboxKind::Reply, "9", "REN4", "rust-engineer #4", "Merged")
    );
    let messages = [said(MessageRole::User, &inbox, 1_000)];
    assert!(messages[0].inbox_fold().is_some());
    let shown: Vec<&Message> = messages.iter().collect();
    assert!(user_entries(&shown, &[]).is_empty());
}

#[test]
fn messages_up_to_the_threshold_are_listed_as_sent() {
    let exact = "a".repeat(MESSAGE_SUMMARY_THRESHOLD);
    assert!(!is_long(&exact));
    assert_eq!(history_label(&exact, Some("ignored")), exact);
    assert_eq!(history_label("two\n  lines", None), "two lines");
    let over = format!("{exact}b");
    assert!(is_long(&over));
}

#[test]
fn a_long_message_is_cut_at_a_word_until_its_summary_comes() {
    let long = "word ".repeat(30);
    let cut = history_label(&long, None);
    assert!(cut.ends_with("word…"), "{cut}");
    assert!(cut.chars().count() <= MESSAGE_SUMMARY_THRESHOLD + 1);
    assert_eq!(
        history_label(&long, Some("Repeat the word")),
        "Repeat the word"
    );
    assert_eq!(
        history_label(&long, Some("  ")),
        cut,
        "a blank summary is no summary"
    );
}
