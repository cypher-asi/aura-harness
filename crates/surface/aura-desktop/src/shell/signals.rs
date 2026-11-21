//! Ctrl+C in the terminal the window was started from, and the system
//! asking the process to stop.
//!
//! Left to themselves these kill the process outright, and the
//! background commands its agents started, which lead their own process
//! groups, run on with nothing to stop them. So the first closes the
//! window as its close button does, and the session shuts down after
//! it; a second exits at once, for when that shutdown hangs.

use super::window::Wake;

/// Exit status after a second signal: the shell convention for SIGINT.
const INTERRUPTED: i32 = 130;

/// Close the window with `wake` on the first signal, and exit the
/// process on the second. Never returns once the signals are taken,
/// since the system no longer stops the process on them.
pub async fn quit_on_signal(wake: impl Fn(Wake) -> bool) {
    let mut signals = match Signals::new() {
        Ok(signals) => signals,
        Err(e) => {
            tracing::warn!(error = %e, "Ctrl+C will not close the window cleanly");
            return;
        }
    };
    signals.next().await;
    tracing::info!("stop signal received; closing the window");
    wake(Wake::Quit);
    signals.next().await;
    std::process::exit(INTERRUPTED);
}

#[cfg(unix)]
struct Signals {
    interrupt: tokio::signal::unix::Signal,
    terminate: tokio::signal::unix::Signal,
}

#[cfg(unix)]
impl Signals {
    fn new() -> std::io::Result<Self> {
        use tokio::signal::unix::{signal, SignalKind};
        Ok(Self {
            interrupt: signal(SignalKind::interrupt())?,
            terminate: signal(SignalKind::terminate())?,
        })
    }

    async fn next(&mut self) {
        tokio::select! {
            Some(()) = self.interrupt.recv() => {}
            Some(()) = self.terminate.recv() => {}
            else => std::future::pending().await,
        }
    }
}

#[cfg(not(unix))]
struct Signals;

#[cfg(not(unix))]
impl Signals {
    #[allow(clippy::unnecessary_wraps)] // as on Unix, where it can fail
    const fn new() -> std::io::Result<Self> {
        Ok(Self)
    }

    async fn next(&mut self) {
        if tokio::signal::ctrl_c().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}
