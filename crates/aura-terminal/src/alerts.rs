//! Getting the attention of a user who looked away: the terminal bell when
//! a long turn finishes or an agent needs an approval, and optionally a
//! desktop notification.

use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use tracing::{debug, warn};

/// Turns shorter than this finish without an alert.
const DEFAULT_NOTIFY_AFTER: Duration = Duration::from_secs(10);

/// How long a desktop notifier may run before it is killed. They return
/// at once when the notification daemon answers; a missing or hung daemon
/// must not leave processes behind.
const NOTIFIER_TIMEOUT: Duration = Duration::from_secs(5);

/// How often the notifier watchdog checks whether it exited.
const NOTIFIER_POLL: Duration = Duration::from_millis(100);

/// Title of desktop notifications.
const NOTIFICATION_TITLE: &str = "Aura";

/// When and how the terminal gets the user's attention.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AlertSettings {
    /// `AURA_TUI_BELL` (default on): ring the terminal bell.
    pub bell: bool,
    /// `AURA_TUI_NOTIFY_AFTER_SECS` (default 10): a turn that ran at least
    /// this long alerts when it finishes. `0` alerts after every turn.
    pub notify_after: Duration,
    /// `AURA_TUI_DESKTOP_NOTIFY` (default off): also show a desktop
    /// notification (`notify-send` on Linux, `osascript` on macOS).
    pub desktop: bool,
}

impl Default for AlertSettings {
    /// The settings with no environment variables set.
    fn default() -> Self {
        Self::from_lookup(|_| None)
    }
}

impl AlertSettings {
    /// Read the settings from the environment.
    #[must_use]
    pub fn from_env() -> Self {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Self {
        let notify_after = get("AURA_TUI_NOTIFY_AFTER_SECS").map_or(DEFAULT_NOTIFY_AFTER, |raw| {
            raw.trim().parse::<u64>().map_or_else(
                |_| {
                    warn!(value = %raw, "Ignoring unreadable AURA_TUI_NOTIFY_AFTER_SECS");
                    DEFAULT_NOTIFY_AFTER
                },
                Duration::from_secs,
            )
        });
        Self {
            bell: get("AURA_TUI_BELL").is_none_or(|raw| !is_off(&raw)),
            notify_after,
            desktop: get("AURA_TUI_DESKTOP_NOTIFY").is_some_and(|raw| is_on(&raw)),
        }
    }
}

fn is_on(raw: &str) -> bool {
    matches!(
        raw.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

fn is_off(raw: &str) -> bool {
    matches!(
        raw.trim().to_ascii_lowercase().as_str(),
        "0" | "false" | "no" | "off"
    )
}

/// Show `message` as a desktop notification without blocking the UI. A
/// platform without a known notifier, or a notifier that is not
/// installed, is logged and otherwise ignored.
pub(crate) fn desktop_notify(message: &str) {
    let Some(mut command) = notifier_command(message) else {
        debug!("No desktop notifier on this platform");
        return;
    };
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    match command.spawn() {
        Ok(child) => {
            let spawned = std::thread::Builder::new()
                .name("aura-notify".to_string())
                .spawn(move || reap_with_timeout(child));
            if let Err(e) = spawned {
                warn!(error = %e, "Could not watch the desktop notifier");
            }
        }
        Err(e) => warn!(error = %e, "Could not start the desktop notifier"),
    }
}

/// Wait for the notifier to exit, killing it after [`NOTIFIER_TIMEOUT`].
fn reap_with_timeout(mut child: Child) {
    let deadline = Instant::now() + NOTIFIER_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(NOTIFIER_POLL),
            Ok(None) => {
                warn!("Desktop notifier timed out; stopping it");
                let _ = child.kill();
                let _ = child.wait();
                return;
            }
            Err(e) => {
                warn!(error = %e, "Lost track of the desktop notifier");
                return;
            }
        }
    }
}

/// The command that shows `message`, for this platform.
fn notifier_command(message: &str) -> Option<Command> {
    if cfg!(target_os = "macos") {
        let mut command = Command::new("osascript");
        command.arg("-e").arg(format!(
            "display notification \"{}\" with title \"{}\"",
            applescript_escape(message),
            NOTIFICATION_TITLE
        ));
        Some(command)
    } else if cfg!(all(unix, not(target_os = "macos"))) {
        let mut command = Command::new("notify-send");
        command
            .arg(format!("--app-name={NOTIFICATION_TITLE}"))
            .arg(NOTIFICATION_TITLE)
            .arg(message);
        Some(command)
    } else {
        None
    }
}

/// `text` safe to put between double quotes in an AppleScript string.
fn applescript_escape(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn settings(vars: &[(&str, &str)]) -> AlertSettings {
        let vars: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        AlertSettings::from_lookup(|name| vars.get(name).cloned())
    }

    #[test]
    fn defaults_ring_after_ten_seconds_without_desktop_notifications() {
        let s = settings(&[]);
        assert!(s.bell);
        assert_eq!(s.notify_after, Duration::from_secs(10));
        assert!(!s.desktop);
    }

    #[test]
    fn reads_every_variable() {
        let s = settings(&[
            ("AURA_TUI_BELL", "off"),
            ("AURA_TUI_NOTIFY_AFTER_SECS", "45"),
            ("AURA_TUI_DESKTOP_NOTIFY", "1"),
        ]);
        assert!(!s.bell);
        assert_eq!(s.notify_after, Duration::from_secs(45));
        assert!(s.desktop);
    }

    #[test]
    fn junk_threshold_keeps_the_default() {
        let s = settings(&[("AURA_TUI_NOTIFY_AFTER_SECS", "soon")]);
        assert_eq!(s.notify_after, DEFAULT_NOTIFY_AFTER);
    }

    #[test]
    fn applescript_quotes_are_escaped() {
        assert_eq!(applescript_escape(r#"say "hi" \o/"#), r#"say \"hi\" \\o/"#);
    }
}
