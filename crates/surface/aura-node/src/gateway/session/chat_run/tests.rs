use super::*;
use crate::protocol::{AssistantMessageStart, TextDelta};

fn text(s: &str) -> OutboundMessage {
    OutboundMessage::TextDelta(TextDelta {
        text: s.to_string(),
    })
}

#[test]
fn subscribe_replays_history_in_order() {
    let ch = ChatEventChannel::new();
    ch.push(text("a"));
    ch.push(text("b"));

    let sub = ch.subscribe();
    let replayed: Vec<String> = sub
        .history
        .iter()
        .map(|m| match m {
            OutboundMessage::TextDelta(d) => d.text.clone(),
            _ => String::new(),
        })
        .collect();
    assert_eq!(replayed, vec!["a".to_string(), "b".to_string()]);
    assert!(!sub.already_done);
}

#[test]
fn history_caps_at_capacity_dropping_oldest() {
    let ch = ChatEventChannel::new();
    for i in 0..(CHAT_EVENT_HISTORY_CAPACITY + 5) {
        ch.push(text(&i.to_string()));
    }
    let sub = ch.subscribe();
    assert_eq!(sub.history.len(), CHAT_EVENT_HISTORY_CAPACITY);
    // Oldest five evicted: first retained entry is "5".
    match &sub.history[0] {
        OutboundMessage::TextDelta(d) => assert_eq!(d.text, "5"),
        other => panic!("unexpected first entry: {other:?}"),
    }
}

#[tokio::test]
async fn subscribe_delivers_live_events_once() {
    let ch = ChatEventChannel::new();
    ch.push(text("history"));

    let mut sub = ch.subscribe();
    assert_eq!(sub.history.len(), 1, "pre-subscribe event lands in history");

    ch.push(text("live"));
    let got = sub.live.recv().await.expect("live event delivered");
    match got {
        OutboundMessage::TextDelta(d) => assert_eq!(d.text, "live"),
        other => panic!("unexpected live event: {other:?}"),
    }
    // The history event must NOT also arrive on the live receiver
    // (exactly-once across history + live).
    assert!(
        sub.live.try_recv().is_err(),
        "no duplicate delivery of the replayed history event"
    );
}

#[tokio::test]
async fn forwarder_drains_into_history_and_live() {
    let ch = ChatEventChannel::new();
    let tx = spawn_event_forwarder(ch.clone());

    tx.send(OutboundMessage::AssistantMessageStart(
        AssistantMessageStart {
            message_id: "m1".into(),
        },
    ))
    .await
    .unwrap();

    // Give the forwarder a tick to drain.
    for _ in 0..50 {
        if !ch.subscribe().history.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let sub = ch.subscribe();
    assert_eq!(sub.history.len(), 1);
    match &sub.history[0] {
        OutboundMessage::AssistantMessageStart(s) => assert_eq!(s.message_id, "m1"),
        other => panic!("unexpected forwarded entry: {other:?}"),
    }
}

#[test]
fn mark_done_sets_already_done() {
    let ch = ChatEventChannel::new();
    assert!(!ch.subscribe().already_done);
    ch.mark_done();
    assert!(ch.subscribe().already_done);
}

#[test]
fn attach_guard_tracks_count() {
    let count = Arc::new(AtomicUsize::new(0));
    {
        let _g1 = AttachGuard::new(count.clone());
        assert_eq!(count.load(Ordering::Acquire), 1);
        {
            let _g2 = AttachGuard::new(count.clone());
            assert_eq!(count.load(Ordering::Acquire), 2);
        }
        assert_eq!(count.load(Ordering::Acquire), 1);
    }
    assert_eq!(count.load(Ordering::Acquire), 0);
}

/// Two concurrent subscribers (i.e. a live attach plus a reattach)
/// both replay the full history and both then receive live events —
/// the channel-level guarantee behind multi-attach (no 409) and
/// reattach (history replay + live).
#[tokio::test]
async fn two_subscribers_both_replay_and_receive_live() {
    let ch = ChatEventChannel::new();
    ch.push(text("session_ready"));

    let mut first = ch.subscribe();
    // A second attach to the same run is non-destructive: it also
    // replays the existing history.
    let mut second = ch.subscribe();
    assert_eq!(first.history.len(), 1);
    assert_eq!(second.history.len(), 1);

    ch.push(text("live"));
    for sub in [&mut first, &mut second] {
        match sub.live.recv().await.expect("live event delivered") {
            OutboundMessage::TextDelta(d) => assert_eq!(d.text, "live"),
            other => panic!("unexpected live event: {other:?}"),
        }
    }
}
