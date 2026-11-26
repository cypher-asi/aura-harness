use super::*;

#[test]
fn a_request_keeps_the_messages_it_was_sent_with() {
    let mut history = History::from(vec![Message::user("one")]);
    let sent = history.shared();
    assert!(
        Arc::ptr_eq(&sent, &history.shared()),
        "sending copies nothing"
    );
    assert_eq!(history.chars(), 3);

    history.push(Message::user("three"));
    assert_eq!(sent.len(), 1);
    assert_eq!(history.len(), 2);
    assert_eq!(history.chars(), 8, "a change is counted again");

    drop(sent);
    let before = history.shared();
    drop(before);
    let unshared = Arc::as_ptr(&history.shared());
    history.push(Message::user("x"));
    assert_eq!(
        Arc::as_ptr(&history.shared()),
        unshared,
        "nothing holds it: no copy"
    );
}
