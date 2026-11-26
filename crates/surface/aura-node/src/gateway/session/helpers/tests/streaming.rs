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
