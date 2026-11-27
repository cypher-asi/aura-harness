use crate::gateway::session::helpers::{apply_turn_result, finalize_turn, forward_events_to_ws};
use crate::gateway::session::Session;
use crate::protocol::{OutboundMessage, TextDelta};
use aura_agent::{AgentLoopEvent, AgentLoopResult};
use std::path::PathBuf;
use tokio::sync::mpsc;
use tokio::time::Duration;

#[tokio::test]
async fn streamed_text_and_assistant_end_wait_for_outbound_capacity() {
    let (outbound_tx, mut outbound_rx) = mpsc::channel(1);
    outbound_tx
        .send(OutboundMessage::TextDelta(TextDelta {
            text: "backlog".to_string(),
        }))
        .await
        .expect("fill outbound channel");

    let (event_tx, event_rx) = mpsc::channel(1);
    let forward_handle = tokio::spawn(forward_events_to_ws(event_rx, outbound_tx.clone()));
    event_tx
        .send(AgentLoopEvent::TextDelta("hello".to_string()))
        .await
        .expect("queue text event");
    drop(event_tx);

    tokio::time::sleep(Duration::from_millis(10)).await;
    assert!(
        !forward_handle.is_finished(),
        "stream forwarding should wait while outbound is full"
    );

    assert!(matches!(
        outbound_rx.recv().await,
        Some(OutboundMessage::TextDelta(delta)) if delta.text == "backlog"
    ));
    forward_handle.await.expect("stream forward task joins");

    let mut session = Session::new(PathBuf::from("/tmp/aura"));
    let loop_result = AgentLoopResult::default();
    let apply_tx = outbound_tx.clone();
    let apply_handle = tokio::spawn(async move {
        apply_turn_result(&mut session, &loop_result, "msg-1", &apply_tx, None).await;
    });

    tokio::time::sleep(Duration::from_millis(10)).await;
    assert!(
        !apply_handle.is_finished(),
        "assistant end should wait behind the streamed text"
    );

    assert!(matches!(
        outbound_rx.recv().await,
        Some(OutboundMessage::TextDelta(delta)) if delta.text == "hello"
    ));
    assert!(matches!(
        outbound_rx.recv().await,
        Some(OutboundMessage::AssistantMessageEnd(end)) if end.message_id == "msg-1"
    ));
    apply_handle.await.expect("apply task joins");
}

#[tokio::test]
async fn forward_events_coalesces_consecutive_text_deltas() {
    let (event_tx, event_rx) = mpsc::channel(8);
    let (outbound_tx, mut outbound_rx) = mpsc::channel(8);
    let forward_handle = tokio::spawn(forward_events_to_ws(event_rx, outbound_tx));

    event_tx
        .send(AgentLoopEvent::TextDelta("hel".to_string()))
        .await
        .unwrap();
    event_tx
        .send(AgentLoopEvent::TextDelta("lo".to_string()))
        .await
        .unwrap();
    drop(event_tx);

    forward_handle.await.expect("stream forward task joins");

    assert!(matches!(
        outbound_rx.recv().await,
        Some(OutboundMessage::TextDelta(delta)) if delta.text == "hello"
    ));
    assert!(outbound_rx.recv().await.is_none());
}

/// A model that pauses mid-block still streams: what it wrote goes out
/// without waiting for more of the block, or for the block to end.
#[tokio::test(start_paused = true)]
async fn forward_events_sends_a_waiting_delta_while_the_stream_stays_open() {
    let (event_tx, event_rx) = mpsc::channel(8);
    let (outbound_tx, mut outbound_rx) = mpsc::channel(8);
    let forward_handle = tokio::spawn(forward_events_to_ws(event_rx, outbound_tx));

    event_tx
        .send(AgentLoopEvent::ThinkingDelta("Let me ".to_string()))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert!(outbound_rx.try_recv().is_err(), "it waits for more first");
    event_tx
        .send(AgentLoopEvent::ThinkingDelta("look".to_string()))
        .await
        .unwrap();

    let sent = tokio::time::timeout(Duration::from_secs(1), outbound_rx.recv())
        .await
        .expect("sent while the block goes on");
    assert!(matches!(
        sent,
        Some(OutboundMessage::ThinkingDelta(delta)) if delta.thinking == "Let me look"
    ));
    assert!(!forward_handle.is_finished(), "the block goes on");
    drop(event_tx);
    forward_handle.await.expect("stream forward task joins");
    assert!(outbound_rx.recv().await.is_none());
}

#[tokio::test]
async fn forward_events_flushes_failed_deltas_before_stream_reset() {
    let (event_tx, event_rx) = mpsc::channel(8);
    let (outbound_tx, mut outbound_rx) = mpsc::channel(8);
    let task = tokio::spawn(forward_events_to_ws(event_rx, outbound_tx));
    event_tx
        .send(AgentLoopEvent::TextDelta("failed 😀".into()))
        .await
        .unwrap();
    event_tx
        .send(AgentLoopEvent::StreamReset {
            reason: "connection reset".into(),
            text_bytes: "failed 😀".len(),
            thinking_bytes: 0,
        })
        .await
        .unwrap();
    event_tx
        .send(AgentLoopEvent::TextDelta("recovered".into()))
        .await
        .unwrap();
    drop(event_tx);
    task.await.unwrap();
    assert!(
        matches!(outbound_rx.recv().await, Some(OutboundMessage::TextDelta(d)) if d.text == "failed 😀")
    );
    assert!(
        matches!(outbound_rx.recv().await, Some(OutboundMessage::Progress(p)) if p.stage == "stream_reset" && p.reset_text_bytes == Some("failed 😀".len() as u64))
    );
    assert!(
        matches!(outbound_rx.recv().await, Some(OutboundMessage::TextDelta(d)) if d.text == "recovered")
    );
}

#[tokio::test]
async fn forward_events_flushes_text_before_non_text_event() {
    let (event_tx, event_rx) = mpsc::channel(8);
    let (outbound_tx, mut outbound_rx) = mpsc::channel(8);
    let forward_handle = tokio::spawn(forward_events_to_ws(event_rx, outbound_tx));

    event_tx
        .send(AgentLoopEvent::TextDelta("before".to_string()))
        .await
        .unwrap();
    event_tx
        .send(AgentLoopEvent::ToolStart {
            id: "tool-1".to_string(),
            name: "read_file".to_string(),
        })
        .await
        .unwrap();
    event_tx
        .send(AgentLoopEvent::TextDelta("after".to_string()))
        .await
        .unwrap();
    drop(event_tx);

    forward_handle.await.expect("stream forward task joins");

    assert!(matches!(
        outbound_rx.recv().await,
        Some(OutboundMessage::TextDelta(delta)) if delta.text == "before"
    ));
    assert!(matches!(
        outbound_rx.recv().await,
        Some(OutboundMessage::ToolUseStart(start))
            if start.id == "tool-1" && start.name == "read_file"
    ));
    assert!(matches!(
        outbound_rx.recv().await,
        Some(OutboundMessage::TextDelta(delta)) if delta.text == "after"
    ));
    assert!(outbound_rx.recv().await.is_none());
}

/// A model call's end goes out after the text it wrote, so a client can
/// show the next call's text as a reply of its own.
#[tokio::test]
async fn forward_events_ends_each_model_call_after_its_text() {
    let (event_tx, event_rx) = mpsc::channel(8);
    let (outbound_tx, mut outbound_rx) = mpsc::channel(8);
    let forward_handle = tokio::spawn(forward_events_to_ws(event_rx, outbound_tx));

    for event in [
        AgentLoopEvent::TextDelta("first".to_string()),
        AgentLoopEvent::IterationComplete {
            iteration: 0,
            input_tokens: 100,
            output_tokens: 10,
        },
        AgentLoopEvent::TextDelta("second".to_string()),
    ] {
        event_tx.send(event).await.unwrap();
    }
    drop(event_tx);
    forward_handle.await.expect("stream forward task joins");

    assert!(matches!(
        outbound_rx.recv().await,
        Some(OutboundMessage::TextDelta(delta)) if delta.text == "first"
    ));
    assert!(matches!(
        outbound_rx.recv().await,
        Some(OutboundMessage::IterationComplete(end))
            if end.iteration == 0 && end.input_tokens == 100 && end.output_tokens == 10
    ));
    assert!(matches!(
        outbound_rx.recv().await,
        Some(OutboundMessage::TextDelta(delta)) if delta.text == "second"
    ));
    assert!(outbound_rx.recv().await.is_none());
}

#[tokio::test]
async fn forward_events_coalesces_thinking_separately_from_text() {
    let (event_tx, event_rx) = mpsc::channel(8);
    let (outbound_tx, mut outbound_rx) = mpsc::channel(8);
    let forward_handle = tokio::spawn(forward_events_to_ws(event_rx, outbound_tx));

    event_tx
        .send(AgentLoopEvent::ThinkingDelta("think".to_string()))
        .await
        .unwrap();
    event_tx
        .send(AgentLoopEvent::ThinkingDelta("ing".to_string()))
        .await
        .unwrap();
    event_tx
        .send(AgentLoopEvent::TextDelta("text".to_string()))
        .await
        .unwrap();
    event_tx
        .send(AgentLoopEvent::ThinkingDelta("more".to_string()))
        .await
        .unwrap();
    drop(event_tx);

    forward_handle.await.expect("stream forward task joins");

    assert!(matches!(
        outbound_rx.recv().await,
        Some(OutboundMessage::ThinkingDelta(delta)) if delta.thinking == "thinking"
    ));
    assert!(matches!(
        outbound_rx.recv().await,
        Some(OutboundMessage::TextDelta(delta)) if delta.text == "text"
    ));
    assert!(matches!(
        outbound_rx.recv().await,
        Some(OutboundMessage::ThinkingDelta(delta)) if delta.thinking == "more"
    ));
    assert!(outbound_rx.recv().await.is_none());
}

#[tokio::test]
async fn forward_events_flushes_pending_text_before_terminal_error() {
    let (event_tx, event_rx) = mpsc::channel(8);
    let (outbound_tx, mut outbound_rx) = mpsc::channel(8);
    let forward_handle = tokio::spawn(forward_events_to_ws(event_rx, outbound_tx));

    event_tx
        .send(AgentLoopEvent::TextDelta("partial".to_string()))
        .await
        .unwrap();
    event_tx
        .send(AgentLoopEvent::Error {
            code: "stream_error".to_string(),
            message: "boom".to_string(),
            recoverable: true,
        })
        .await
        .unwrap();
    drop(event_tx);

    forward_handle.await.expect("stream forward task joins");

    assert!(matches!(
        outbound_rx.recv().await,
        Some(OutboundMessage::TextDelta(delta)) if delta.text == "partial"
    ));
    assert!(matches!(
        outbound_rx.recv().await,
        Some(OutboundMessage::Error(err))
            if err.code == "stream_error" && err.message == "boom" && err.recoverable
    ));
    assert!(outbound_rx.recv().await.is_none());
}

#[tokio::test]
async fn finalize_turn_error_waits_for_outbound_capacity() {
    let (outbound_tx, mut outbound_rx) = mpsc::channel(1);
    outbound_tx
        .send(OutboundMessage::TextDelta(TextDelta {
            text: "backlog".to_string(),
        }))
        .await
        .expect("fill outbound channel");

    async fn panicking_turn() -> anyhow::Result<AgentLoopResult> {
        panic!("turn panic")
    }
    let panic_handle = tokio::spawn(panicking_turn());
    let join_result = panic_handle.await;

    let mut session = Session::new(PathBuf::from("/tmp/aura"));
    let finalize_tx = outbound_tx.clone();
    let finalize_handle = tokio::spawn(async move {
        finalize_turn(&mut session, join_result, "msg-2", &finalize_tx, None).await;
    });

    tokio::time::sleep(Duration::from_millis(10)).await;
    assert!(
        !finalize_handle.is_finished(),
        "turn error should wait while outbound is full"
    );

    assert!(matches!(
        outbound_rx.recv().await,
        Some(OutboundMessage::TextDelta(delta)) if delta.text == "backlog"
    ));
    assert!(matches!(
        outbound_rx.recv().await,
        Some(OutboundMessage::Error(err)) if err.code == "internal_error"
    ));
    assert!(matches!(
        outbound_rx.recv().await,
        Some(OutboundMessage::AssistantMessageEnd(end))
            if end.message_id == "msg-2" && end.stop_reason == "error"
    ));
    finalize_handle.await.expect("finalize task joins");
}
