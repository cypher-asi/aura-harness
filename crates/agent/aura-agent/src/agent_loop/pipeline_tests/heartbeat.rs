use super::*;

/// Phase 6 of agent-stuck-and-reset: while a long-running tool is
/// in flight, [`super::super::tool_pipeline::spawn_tool_heartbeat`] must
/// emit periodic [`AgentLoopEvent::Progress`] frames with
/// `stage: "tool_running"` so aura-os's sliding-idle watchdog (and
/// the client-side stuck-stream watchdog) see forward motion.
///
/// Drives the spawn helper directly with a 50ms cadence so the test
/// stays under a second; full-loop coverage of the same path lives
/// in [`pipeline_heartbeat_fires_during_long_tool_call`].
#[tokio::test]
async fn spawn_tool_heartbeat_emits_periodic_progress_events() {
    use std::time::Duration;
    use tokio::time::timeout;

    use super::super::tool_pipeline::spawn_tool_heartbeat;

    let (tx, mut rx) = mpsc::channel::<AgentLoopEvent>(16);
    let to_execute = vec![ToolCallInfo {
        id: "call_1".to_string(),
        name: "slow_tool".to_string(),
        input: serde_json::json!({}),
    }];

    let guard = spawn_tool_heartbeat(Some(&tx), &to_execute, Duration::from_millis(50));

    let mut tool_running_events = 0usize;
    let deadline = tokio::time::Instant::now() + Duration::from_millis(400);
    while tool_running_events < 2 && tokio::time::Instant::now() < deadline {
        match timeout(Duration::from_millis(150), rx.recv()).await {
            Ok(Some(AgentLoopEvent::Progress {
                stage,
                tool_name,
                elapsed_ms,
                ..
            })) => {
                if stage == "tool_running" {
                    assert_eq!(tool_name.as_deref(), Some("slow_tool"));
                    assert!(
                        elapsed_ms.unwrap_or(0) >= 50,
                        "elapsed_ms must reflect at least one full interval"
                    );
                    tool_running_events += 1;
                }
            }
            Ok(Some(_)) => continue,
            Ok(None) | Err(_) => break,
        }
    }

    drop(guard);
    assert!(
        tool_running_events >= 2,
        "expected at least 2 tool_running heartbeats inside the wait \
         window, got {tool_running_events}"
    );
}

/// Drop semantics: once the tool batch returns and the
/// [`super::super::tool_pipeline::HeartbeatGuard`] is dropped, no further
/// `tool_running` heartbeats may land on the channel. Guards a
/// regression where the heartbeat task could outlive the tool call
/// and keep emitting frames against a closed turn (which would race
/// with `AssistantMessageEnd` ordering on the wire).
#[tokio::test]
async fn spawn_tool_heartbeat_stops_after_guard_drops() {
    use std::time::Duration;
    use tokio::time::timeout;

    use super::super::tool_pipeline::spawn_tool_heartbeat;

    let (tx, mut rx) = mpsc::channel::<AgentLoopEvent>(16);
    let to_execute = vec![ToolCallInfo {
        id: "call_1".to_string(),
        name: "slow_tool".to_string(),
        input: serde_json::json!({}),
    }];

    {
        let _guard = spawn_tool_heartbeat(Some(&tx), &to_execute, Duration::from_millis(20));
        let _ = timeout(Duration::from_millis(80), rx.recv()).await;
    }
    while let Ok(Some(_)) = timeout(Duration::from_millis(20), rx.recv()).await {}

    let post_drop = timeout(Duration::from_millis(120), rx.recv()).await;
    let none_arrived = match post_drop {
        Err(_) => true,
        Ok(None) => true,
        Ok(Some(AgentLoopEvent::Progress { .. })) => false,
        Ok(Some(_)) => true,
    };
    assert!(
        none_arrived,
        "heartbeat task must abort when its guard drops; received a Progress \
         frame after drop"
    );
}

/// Heartbeat env knob clamping (now via `aura_config`): zero is bumped
/// to the floor, gigantic values clamp to the ceiling. The env-var
/// name (`AURA_TURN_TOOL_HEARTBEAT_INTERVAL_SECS`) is shared with the
/// aura-os watchdog; the boundary tests assert no caller outside
/// `aura-config` parses it directly.
#[test]
fn tool_heartbeat_interval_clamps_via_aura_config() {
    use std::time::Duration;

    use super::super::tool_pipeline::tool_heartbeat_interval;

    fn install(secs: u64) -> aura_config::ConfigGuard {
        let mut cfg = aura_config::current();
        cfg.agent.tools.heartbeat_interval = Duration::from_secs(secs);
        aura_config::install_for_test(cfg)
    }

    {
        let _g = install(10);
        assert_eq!(tool_heartbeat_interval(), Duration::from_secs(10));
    }
    {
        let _g = install(aura_config::MIN_TOOL_HEARTBEAT_INTERVAL_SECS);
        assert_eq!(
            tool_heartbeat_interval(),
            Duration::from_secs(aura_config::MIN_TOOL_HEARTBEAT_INTERVAL_SECS)
        );
    }
    {
        let _g = install(aura_config::MAX_TOOL_HEARTBEAT_INTERVAL_SECS);
        assert_eq!(
            tool_heartbeat_interval(),
            Duration::from_secs(aura_config::MAX_TOOL_HEARTBEAT_INTERVAL_SECS)
        );
    }
}
