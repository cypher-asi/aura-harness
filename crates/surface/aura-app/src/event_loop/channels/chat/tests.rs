use super::{answer_to, asked_at};
use aura_model::Message;

#[test]
fn an_answer_comes_before_the_next_user_message() {
    let messages = [
        Message::user("[#site] status?"),
        Message::assistant("Building."),
        Message::user("unrelated direct question"),
        Message::assistant("Unrelated answer."),
    ];
    assert_eq!(
        answer_to(&messages, "[#site] status?", &[]),
        Some((0, "Building.".to_string()))
    );

    // A question the model never answered gets nothing, not the reply
    // to what came after it.
    let unanswered = [
        Message::user("[#site] status?"),
        Message::user("unrelated direct question"),
        Message::assistant("Unrelated answer."),
    ];
    assert_eq!(answer_to(&unanswered, "[#site] status?", &[]), None);
}

#[test]
fn identical_questions_each_take_their_own_answer() {
    let messages = [
        Message::user("[#site] status?"),
        Message::assistant("First."),
        Message::user("[#site] status?"),
        Message::assistant("Second."),
    ];
    let first = answer_to(&messages, "[#site] status?", &[]).unwrap();
    let second = answer_to(&messages, "[#site] status?", &[first.0]).unwrap();
    assert_ne!(first.0, second.0);
    assert_ne!(first.1, second.1);
    assert_eq!(asked_at(&messages, "[#site] status?", &[0, 2]), None);
}
