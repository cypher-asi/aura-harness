//! Output a foreground command prints while it runs, handed in batches to
//! the [`CommandOutputHook`] of the call that started it.

use aura_exec_traits::CommandOutputHook;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// Longest a batch waits before it is handed on.
const FLUSH_INTERVAL: Duration = Duration::from_millis(150);
/// A batch this big is handed on at once.
const FLUSH_BYTES: usize = 8 * 1024;

/// Where a call's output goes while it runs: nowhere, or a hook under the
/// call's `tool_use` id. Clones share one batch, so stdout and stderr
/// arrive interleaved in the order they were read.
#[derive(Clone, Default)]
pub struct LiveOutput(Option<Arc<Tee>>);

struct Tee {
    hook: Arc<dyn CommandOutputHook>,
    tool_use_id: String,
    pending: Mutex<Pending>,
}

#[derive(Default)]
struct Pending {
    bytes: Vec<u8>,
    flushed_at: Option<Instant>,
}

impl LiveOutput {
    /// Output for the call `tool_use_id`, shown to `hook`.
    #[must_use]
    pub fn new(hook: Arc<dyn CommandOutputHook>, tool_use_id: String) -> Self {
        Self(Some(Arc::new(Tee {
            hook,
            tool_use_id,
            pending: Mutex::default(),
        })))
    }

    /// Add `chunk`, handing on the batch once it is big or old enough.
    pub fn push(&self, chunk: &[u8]) {
        if let Some(tee) = &self.0 {
            let mut pending = tee.lock();
            pending.bytes.extend_from_slice(chunk);
            if pending.bytes.len() >= FLUSH_BYTES || pending.due() {
                tee.flush(&mut pending, false);
            }
        }
    }

    /// Hand on output that has waited long enough. Wait loops call this
    /// as they poll, so output that stops arriving still gets shown.
    pub fn tick(&self) {
        if let Some(tee) = &self.0 {
            let mut pending = tee.lock();
            if !pending.bytes.is_empty() && pending.due() {
                tee.flush(&mut pending, false);
            }
        }
    }

    /// Hand on everything still waiting.
    pub fn finish(&self) {
        if let Some(tee) = &self.0 {
            let mut pending = tee.lock();
            tee.flush(&mut pending, true);
        }
    }
}

impl Pending {
    fn due(&self) -> bool {
        self.flushed_at
            .is_none_or(|at| at.elapsed() >= FLUSH_INTERVAL)
    }
}

impl Tee {
    fn lock(&self) -> std::sync::MutexGuard<'_, Pending> {
        self.pending.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Hand on the batch, holding back a character cut off at its end
    /// unless this is the last batch.
    fn flush(&self, pending: &mut Pending, last: bool) {
        let keep = if last {
            0
        } else {
            incomplete_tail(&pending.bytes)
        };
        let send = pending.bytes.len() - keep;
        if send == 0 {
            return;
        }
        let rest = pending.bytes.split_off(send);
        let batch = std::mem::replace(&mut pending.bytes, rest);
        pending.flushed_at = Some(Instant::now());
        self.hook.output(&self.tool_use_id, &batch);
    }
}

/// Bytes at the end of `bytes` that start a UTF-8 character still missing
/// its other bytes.
fn incomplete_tail(bytes: &[u8]) -> usize {
    match std::str::from_utf8(bytes) {
        Err(e) if e.error_len().is_none() => bytes.len() - e.valid_up_to(),
        _ => 0,
    }
}

impl std::fmt::Debug for LiveOutput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            Some(tee) => f.debug_tuple("LiveOutput").field(&tee.tool_use_id).finish(),
            None => f.write_str("LiveOutput(None)"),
        }
    }
}

impl PartialEq for LiveOutput {
    fn eq(&self, other: &Self) -> bool {
        match (&self.0, &other.0) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        }
    }
}

impl Eq for LiveOutput {}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Recording(Mutex<Vec<(String, Vec<u8>)>>);

    impl CommandOutputHook for Recording {
        fn output(&self, tool_use_id: &str, chunk: &[u8]) {
            self.0
                .lock()
                .unwrap()
                .push((tool_use_id.to_string(), chunk.to_vec()));
        }
    }

    fn live() -> (Arc<Recording>, LiveOutput) {
        let hook = Arc::new(Recording::default());
        let live = LiveOutput::new(hook.clone(), "t1".into());
        (hook, live)
    }

    #[test]
    fn first_chunk_goes_out_at_once_and_later_ones_wait() {
        let (hook, live) = live();
        live.push(b"a\n");
        live.push(b"b\n");
        assert_eq!(
            hook.0.lock().unwrap().clone(),
            vec![("t1".into(), b"a\n".to_vec())]
        );
        live.finish();
        assert_eq!(hook.0.lock().unwrap()[1].1, b"b\n");
    }

    #[test]
    fn tick_hands_on_output_that_waited() {
        let (hook, live) = live();
        live.push(b"a");
        live.push(b"b");
        live.tick();
        assert_eq!(hook.0.lock().unwrap().len(), 1);
        std::thread::sleep(FLUSH_INTERVAL);
        live.tick();
        assert_eq!(hook.0.lock().unwrap()[1].1, b"b");
    }

    #[test]
    fn a_split_character_waits_for_its_other_bytes() {
        let (hook, live) = live();
        let snow = "é".as_bytes();
        live.push(&[b'x', snow[0]]);
        live.push(&snow[1..]);
        live.finish();
        let sent: Vec<Vec<u8>> = hook
            .0
            .lock()
            .unwrap()
            .iter()
            .map(|(_, c)| c.clone())
            .collect();
        assert_eq!(sent, vec![b"x".to_vec(), snow.to_vec()]);
    }

    #[test]
    fn no_hook_does_nothing() {
        let live = LiveOutput::default();
        live.push(b"a");
        live.tick();
        live.finish();
    }
}
