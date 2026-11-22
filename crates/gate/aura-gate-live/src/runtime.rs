//! What Gate needs of the running app: the clock, and threads startup
//! doesn't wait for.

use std::time::{SystemTime, UNIX_EPOCH};

/// Now, in Unix milliseconds.
pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// Run `work` on a blocking thread of the current Tokio runtime, or here
/// when there is none.
pub(crate) fn in_background(work: impl FnOnce() + Send + 'static) {
    match tokio::runtime::Handle::try_current() {
        Ok(runtime) => {
            runtime.spawn_blocking(work);
        }
        Err(_) => work(),
    }
}
