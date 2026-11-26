use std::sync::OnceLock;
use std::time::{Duration, Instant};
use tracing::info;

static OUTBOUND_REQUEST_THROTTLE: OnceLock<tokio::sync::Mutex<Option<Instant>>> = OnceLock::new();

pub(super) async fn throttle_outbound_request(min_interval_ms: u64, model: &str) {
    if min_interval_ms == 0 {
        return;
    }

    let min_interval = Duration::from_millis(min_interval_ms);
    let lock = OUTBOUND_REQUEST_THROTTLE.get_or_init(|| tokio::sync::Mutex::new(None));
    let mut last_sent_at = lock.lock().await;

    if let Some(last) = *last_sent_at {
        let elapsed = last.elapsed();
        if elapsed < min_interval {
            let sleep = min_interval - elapsed;
            let throttle_ms = u64::try_from(sleep.as_millis()).unwrap_or(u64::MAX);
            info!(
                model = %model,
                throttle_ms,
                min_request_interval_ms = min_interval_ms,
                "Throttling outbound LLM request"
            );
            tokio::time::sleep(sleep).await;
        }
    }

    *last_sent_at = Some(Instant::now());
}
