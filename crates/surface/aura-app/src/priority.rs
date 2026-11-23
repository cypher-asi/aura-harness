//! Agents run below the UI in CPU priority.
//!
//! Many agents at once can keep every core busy: their own threads and
//! the builds and tests their tools start. The UI draws on a thread of
//! normal priority. The async runtime's threads lower themselves as they
//! start ([`agent_thread_start`]), and what they spawn inherits their
//! priority, so a build can't starve the UI of the CPU it needs to keep
//! up with input.
//!
//! `AURA_AGENT_NICE` says how far below, in nice levels (0 to 19; 0
//! leaves them as they are). Only Linux keeps a nice level per thread,
//! so elsewhere this does nothing.

/// Nice levels agents run below the UI unless `AURA_AGENT_NICE` says
/// otherwise.
pub const DEFAULT_NICE: i32 = 10;

/// How many nice levels agents run below the UI: `AURA_AGENT_NICE`,
/// else [`DEFAULT_NICE`].
#[must_use]
pub fn agent_nice() -> i32 {
    nice_from(std::env::var("AURA_AGENT_NICE").ok().as_deref())
}

fn nice_from(value: Option<&str>) -> i32 {
    value
        .and_then(|value| value.trim().parse::<i32>().ok())
        .map_or(DEFAULT_NICE, |nice| nice.clamp(0, 19))
}

/// What a runtime's threads run as they start (tokio's
/// `on_thread_start`) so agents run [`agent_nice`] levels below the UI.
pub fn agent_thread_start() -> impl Fn() + Send + Sync + 'static {
    let by = agent_nice();
    move || lower_this_thread(by)
}

/// Lower the calling thread `by` nice levels, and with it everything it
/// starts from now on. Lowering needs no privileges; raising it back
/// would, so this is for threads that stay low.
pub fn lower_this_thread(by: i32) {
    if by <= 0 {
        return;
    }
    #[cfg(target_os = "linux")]
    {
        let thread = Some(rustix::thread::gettid());
        let lowered = rustix::process::getpriority_process(thread)
            .and_then(|nice| rustix::process::setpriority_process(thread, (nice + by).min(19)));
        if let Err(error) = lowered {
            tracing::debug!(%error, by, "could not lower an agent thread's priority");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_nice_level_is_read_and_kept_in_range() {
        assert_eq!(nice_from(None), DEFAULT_NICE);
        assert_eq!(nice_from(Some(" 5 ")), 5);
        assert_eq!(nice_from(Some("0")), 0, "off");
        assert_eq!(nice_from(Some("40")), 19);
        assert_eq!(nice_from(Some("-3")), 0, "never above the UI");
        assert_eq!(nice_from(Some("low")), DEFAULT_NICE);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_lowered_thread_and_what_it_starts_run_below_the_rest() {
        let (own, child) = std::thread::spawn(|| {
            lower_this_thread(7);
            let own = rustix::process::getpriority_process(Some(rustix::thread::gettid()));
            let child = std::thread::spawn(|| {
                rustix::process::getpriority_process(Some(rustix::thread::gettid()))
            })
            .join()
            .expect("the child thread ran");
            (own, child)
        })
        .join()
        .expect("the thread ran");
        let base = rustix::process::getpriority_process(Some(rustix::thread::gettid()))
            .expect("this thread's priority");
        // A test runner already niced can't go past 19.
        assert_eq!(own.expect("its priority"), (base + 7).min(19));
        assert_eq!(child.expect("its priority"), (base + 7).min(19));
    }
}
