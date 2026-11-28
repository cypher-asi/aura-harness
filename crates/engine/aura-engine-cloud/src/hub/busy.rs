//! One turn at a time per agent: a turn asked for while the agent's
//! machine has one under way already (one its mail started, one whose
//! stream dropped, one asked for before this hub started) waits for it
//! to end, instead of running the agent twice at once.

use std::time::Duration;

use aura_core_protocol::RuntimeRequest;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::node_client::{NodeClient, RunStart};
use super::turn::RemoteEvent;
use crate::{CloudError, CloudResult};

/// How often a node is asked again while its agent's other turn runs.
#[cfg(not(test))]
const BUSY_RETRY: Duration = Duration::from_secs(5);
#[cfg(test)]
const BUSY_RETRY: Duration = Duration::from_millis(20);

/// What a turn's chat says while it waits.
pub(super) const WAITING: &str = "Waiting for the turn already running on its machine";

/// Start the chat run `request` asks for on `node` once its agent has no
/// turn under way there, showing on `tx` that it waits. Returns the
/// node's answer. The caller's own deadline bounds the wait.
///
/// # Errors
///
/// When the node can't be asked or refuses the run, or `cancel` stops
/// the wait.
pub(super) async fn start_when_free(
    node: &NodeClient,
    request: &RuntimeRequest,
    tx: &mpsc::Sender<RemoteEvent>,
    cancel: &CancellationToken,
) -> CloudResult<serde_json::Value> {
    let mut told = false;
    loop {
        let run = match node.start_run(request).await? {
            RunStart::Started(started) => return Ok(started),
            RunStart::Busy(run) => run,
        };
        if !told {
            told = true;
            tracing::info!(node = node.node(), %run, "the agent has a turn under way on its machine; waiting for it");
            let _ = tx.send(RemoteEvent::Status(WAITING.into())).await;
        }
        tokio::select! {
            () = cancel.cancelled() => {
                return Err(CloudError::BadState {
                    machine: node.node().into(),
                    state: "cancelled".into(),
                    op: "wait for the turn under way",
                });
            }
            () = tokio::time::sleep(BUSY_RETRY) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub::request::{chat_request, TurnAsk};
    use aura_config::BoxdToken;
    use aura_core_types::AgentId;
    use axum::http::StatusCode;
    use axum::routing::post;
    use axum::{Json, Router};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// A node that turns the first `busy` runs away as `agent_busy`,
    /// then starts them; returns its base URL and the runs asked for.
    async fn node(busy: usize) -> (NodeClient, Arc<AtomicUsize>) {
        let asked = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&asked);
        let router = Router::new().route(
            "/v1/run",
            post(move || {
                let n = counted.fetch_add(1, Ordering::SeqCst);
                async move {
                    if n < busy {
                        let body = serde_json::json!({"code": "agent_busy", "run_id": "r1"});
                        (StatusCode::CONFLICT, Json(body))
                    } else {
                        let body =
                            serde_json::json!({"run_id": "r2", "event_stream_url": "/stream/r2"});
                        (StatusCode::CREATED, Json(body))
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        let client = NodeClient::new(
            "aura-a",
            format!("http://{addr}"),
            BoxdToken::new("node-token-0123456"),
        );
        (client, asked)
    }

    #[tokio::test]
    async fn a_turn_waits_for_the_one_under_way_on_its_machine_then_starts() {
        let (node, asked) = node(2).await;
        let request = chat_request(AgentId::new([1; 32]), &TurnAsk::default());
        let (tx, mut rx) = mpsc::channel(8);
        let started = start_when_free(&node, &request, &tx, &CancellationToken::new())
            .await
            .expect("started");
        assert_eq!(
            started["run_id"], "r2",
            "its own run, once the other turn ended"
        );
        assert_eq!(asked.load(Ordering::SeqCst), 3);
        assert_eq!(
            rx.try_recv().ok(),
            Some(RemoteEvent::Status(WAITING.into())),
            "said once"
        );
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn stopping_a_waiting_turn_starts_nothing() {
        let (node, asked) = node(usize::MAX).await;
        let request = chat_request(AgentId::new([1; 32]), &TurnAsk::default());
        let (tx, _rx) = mpsc::channel(8);
        let cancel = CancellationToken::new();
        let stopper = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            stopper.cancel();
        });
        let waited = start_when_free(&node, &request, &tx, &cancel).await;
        assert!(waited.is_err());
        let after = asked.load(Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(asked.load(Ordering::SeqCst), after, "it stopped asking");
    }
}
