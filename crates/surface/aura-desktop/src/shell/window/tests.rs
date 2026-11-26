use super::*;
use std::sync::atomic::AtomicUsize;
use std::time::Duration;

fn status(n: usize) -> UiCommand {
    UiCommand::SetStatus(n.to_string())
}

#[test]
fn frames_are_paced_a_little_under_the_refresh() {
    assert_eq!(pace(60_000), Duration::from_millis(15));
    assert_eq!(pace(144_000), Duration::from_micros(6250));
    assert_eq!(pace(0), pace(REFRESH), "a display that says nothing");
}

#[tokio::test]
async fn commands_wake_the_loop_once_until_they_are_taken() {
    let (tx, rx) = mpsc::channel(8);
    let inbox = Arc::new(Inbox::default());
    let (wakes, ended) = (
        Arc::new(AtomicUsize::new(0)),
        Arc::new(AtomicBool::new(false)),
    );
    let (count, end) = (Arc::clone(&wakes), Arc::clone(&ended));
    let pump = tokio::spawn(pump(rx, Arc::clone(&inbox), move |wake| {
        match wake {
            Wake::Commands => {
                count.fetch_add(1, Ordering::SeqCst);
            }
            _ => end.store(true, Ordering::SeqCst),
        }
        true
    }));

    for n in 0..5 {
        tx.send(status(n)).await.unwrap();
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    assert_eq!(wakes.load(Ordering::SeqCst), 1, "one wake for all five");
    assert_eq!(inbox.take().len(), 5);
    assert!(inbox.take().is_empty());

    tx.send(status(5)).await.unwrap();
    tokio::time::sleep(Duration::from_millis(2)).await;
    assert_eq!(wakes.load(Ordering::SeqCst), 2, "and one for the next");
    assert_eq!(inbox.take().len(), 1);

    drop(tx);
    pump.await.unwrap();
    assert!(ended.load(Ordering::SeqCst));
}

#[tokio::test]
async fn a_full_inbox_holds_the_session_up() {
    let (tx, rx) = mpsc::channel(16);
    let inbox = Arc::new(Inbox::default());
    let pump = tokio::spawn(pump(rx, Arc::clone(&inbox), |_| true));
    let total = INBOX_ROOM + 3 * MAX_BATCH;
    let sender = tokio::spawn(async move {
        for n in 0..total {
            tx.send(status(n)).await.unwrap();
        }
    });

    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!sender.is_finished(), "the session waits for the window");
    let mut taken = 0;
    while taken < total {
        let batch = inbox.take();
        assert!(batch.len() < INBOX_ROOM + MAX_BATCH, "{}", batch.len());
        taken += batch.len();
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    assert_eq!(taken, total, "nothing is lost");
    sender.await.unwrap();
    pump.await.unwrap();
}
