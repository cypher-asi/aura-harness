use super::*;
use aura_core_types::AgentId;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Started no background commands.
struct NoBackground;

impl BackgroundProcessHook for NoBackground {
    fn log_dir(&self) -> PathBuf {
        std::env::temp_dir()
    }

    fn adopt(
        &self,
        _agent: AgentId,
        _process: aura_exec_tools::BackgroundProcess,
    ) -> Result<(), String> {
        Err("not in this test".into())
    }
}

/// A session task whose event loop is `event_loop`, given the quit
/// token.
fn session_task<F, Fut>(event_loop: F) -> SessionTask
where
    F: FnOnce(CancellationToken) -> Fut,
    Fut: Future<Output = anyhow::Result<()>> + Send + 'static,
{
    let quit = CancellationToken::new();
    SessionTask {
        handle: tokio::spawn(event_loop(quit.clone())),
        background: Arc::new(NoBackground),
        quit,
    }
}

/// The bug behind "cancelled a turn, closed the app, and lost the
/// data": `finish` aborted the loop while it was still running, so
/// turns cancelled at quit never got to save their sessions.
#[tokio::test(start_paused = true)]
async fn finish_lets_the_event_loop_shut_down_instead_of_aborting_it() {
    let saved = Arc::new(AtomicBool::new(false));
    let loop_saved = Arc::clone(&saved);
    let task = session_task(move |quit| async move {
        quit.cancelled().await;
        // Its cancelled turns end and save their sessions.
        tokio::time::sleep(Duration::from_secs(2)).await;
        loop_saved.store(true, Ordering::SeqCst);
        Ok(())
    });

    task.finish().await.unwrap();

    assert!(
        saved.load(Ordering::SeqCst),
        "the loop was told to quit and allowed to finish"
    );
}

#[tokio::test(start_paused = true)]
async fn finish_aborts_an_event_loop_that_does_not_shut_down_in_time() {
    let (alive, aborted) = tokio::sync::oneshot::channel::<()>();
    let task = session_task(move |_quit| async move {
        // Dropped, closing the channel, when the loop is aborted.
        let _alive = alive;
        std::future::pending::<anyhow::Result<()>>().await
    });
    let started = tokio::time::Instant::now();

    task.finish().await.unwrap();

    assert!(started.elapsed() >= event_loop::FINISH_GRACE);
    let ended = tokio::time::timeout(Duration::from_secs(1), aborted).await;
    assert!(matches!(ended, Ok(Err(_))), "the stuck loop was aborted");
}

#[tokio::test(start_paused = true)]
async fn finish_reports_an_event_loop_that_failed() {
    let task = session_task(|_quit| async { Err(anyhow::anyhow!("store closed")) });

    let error = task.finish().await.unwrap_err();

    assert_eq!(
        format!("{error:#}"),
        "agent event loop failed: store closed"
    );
}
