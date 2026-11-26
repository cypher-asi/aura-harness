
use super::*;

struct ImmediateAutomaton;

#[async_trait::async_trait]
impl Automaton for ImmediateAutomaton {
    fn kind(&self) -> &str {
        "immediate"
    }

    async fn tick(&self, _ctx: &mut TickContext) -> Result<TickOutcome, AutomatonError> {
        Ok(TickOutcome::Done)
    }
}

#[tokio::test]
async fn test_automaton_runtime_install_and_list() {
    let runtime = AutomatonRuntime::new();

    let (handle, _rx) = runtime
        .install(Box::new(ImmediateAutomaton), serde_json::json!({}), None)
        .await
        .unwrap();

    let list = runtime.list();
    assert!(
        list.iter()
            .any(|info| info.id.as_str() == handle.id().as_str()),
        "installed automaton should appear in list()"
    );
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].kind, "immediate");
}

#[tokio::test]
async fn registry_trait_read_only_view() {
    use aura_core_types::{Registry, RegistryError};

    let mut runtime = AutomatonRuntime::new();
    assert!(Registry::is_empty(&runtime));

    let (handle, _rx) = runtime
        .install(Box::new(ImmediateAutomaton), serde_json::json!({}), None)
        .await
        .unwrap();
    let id = handle.id().as_str().to_string();

    assert_eq!(Registry::len(&runtime), 1);
    let info = Registry::get(&runtime, &id).expect("info present");
    assert_eq!(info.kind, "immediate");

    let iter_ids: Vec<_> = Registry::iter(&runtime)
        .into_iter()
        .map(|(k, _)| k)
        .collect();
    assert_eq!(iter_ids, vec![id.clone()]);

    let err = Registry::register(&mut runtime, id.clone(), info)
        .expect_err("direct register must be unsupported");
    assert!(matches!(err, RegistryError::Unsupported(_)));

    let removed = Registry::remove(&mut runtime, &id);
    assert!(removed.is_none(), "direct remove is unsupported");
}

/// Slow-tick automaton used to exercise the user-stop path.
///
/// Each tick races a 50ms timer against the shared cancellation
/// token and always returns `Continue` — the runtime loop top
/// is what observes cancellation and exits with
/// `AutomatonStatus::Stopped`. Returning `Done` from inside the
/// tick would route the exit through the natural-completion
/// branch (`AutomatonStatus::Completed`), masking the stop path
/// this test is meant to cover. The short timer keeps the
/// post-stop loop iteration tight so the test finishes quickly
/// without sacrificing the "tick was actually running" invariant.
struct SlowAutomaton;

#[async_trait::async_trait]
impl Automaton for SlowAutomaton {
    fn kind(&self) -> &str {
        "slow"
    }

    async fn tick(&self, ctx: &mut TickContext) -> Result<TickOutcome, AutomatonError> {
        tokio::select! {
            () = ctx.cancellation_token().cancelled() => Ok(TickOutcome::Continue),
            () = tokio::time::sleep(std::time::Duration::from_millis(50)) => {
                Ok(TickOutcome::Continue)
            }
        }
    }
}

/// Operator-initiated `stop()` on a long-running automaton must
/// surface a `Stopped { reason: "Stopped" }` event so downstream
/// observers (the `aura-os-server` run-log forwarder, the
/// operator UI's status badge, the `/stream/:run_id` WS consumer)
/// can distinguish a user-driven halt from a natural
/// completion (`reason: "Completed"`) or a crash
/// (`reason: "Failed"`).
///
/// Companion to the dev-loop `record_task_cancelled` /
/// task-run `finalize_task` cancellation guards: those keep the
/// in-flight task out of the failed bucket, this asserts the
/// outer automaton lifecycle event still fires with the right
/// reason string.
#[tokio::test(flavor = "current_thread")]
async fn user_stop_emits_stopped_event_with_stopped_reason() {
    let runtime = AutomatonRuntime::new();
    let (handle, mut rx) = runtime
        .install(Box::new(SlowAutomaton), serde_json::json!({}), None)
        .await
        .unwrap();

    // Wait for `Started` so we know the loop is actually running
    // (i.e. the cancellation fires mid-execution, not before
    // `on_install`).
    let mut saw_started = false;
    while let Ok(Some(evt)) =
        tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv()).await
    {
        if matches!(evt, AutomatonEvent::Started { .. }) {
            saw_started = true;
            break;
        }
    }
    assert!(saw_started, "automaton must reach Started before stop()");

    handle.stop();

    let mut stop_reason: Option<String> = None;
    while let Ok(Some(evt)) =
        tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv()).await
    {
        match evt {
            AutomatonEvent::Stopped { reason, .. } => {
                stop_reason = Some(reason);
            }
            AutomatonEvent::Done => break,
            _ => {}
        }
    }

    assert_eq!(
        stop_reason.as_deref(),
        Some("Stopped"),
        "user-initiated stop must emit Stopped {{ reason: \"Stopped\" }} \
             so downstream observers can distinguish it from Completed/Failed"
    );
}

#[tokio::test]
async fn test_automaton_runtime_start_stop() {
    let runtime = AutomatonRuntime::new();

    let (handle, mut rx) = runtime
        .install(Box::new(ImmediateAutomaton), serde_json::json!({}), None)
        .await
        .unwrap();

    let id = handle.id().as_str().to_string();

    let mut saw_started = false;
    let mut saw_done = false;
    while let Ok(evt) = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv()).await {
        match evt {
            Some(AutomatonEvent::Started { .. }) => saw_started = true,
            Some(AutomatonEvent::Done) => {
                saw_done = true;
                break;
            }
            None => break,
            _ => {}
        }
    }

    assert!(saw_started, "should have received Started event");
    assert!(saw_done, "should have received Done event");

    let stop_result = runtime.stop(&id);
    assert!(
        stop_result.is_err(),
        "stop after completion should return NotFound since the instance is cleaned up"
    );
}
