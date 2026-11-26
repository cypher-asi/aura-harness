//! The notification in the corner: what it says, when it goes away and
//! how long it has been showing.

use std::time::{Duration, Instant};

use super::NotificationType;

/// How long a notification stays in the corner.
const NOTIFICATION_TTL: Duration = Duration::from_secs(4);

/// How long an error stays in the corner.
const ERROR_NOTIFICATION_TTL: Duration = Duration::from_secs(6);

/// How long a notification too long for its row stays once it has
/// scrolled to its end.
const NOTIFICATION_TAIL: Duration = Duration::from_secs(2);

/// The corner notification and its timer.
#[derive(Debug, Default)]
pub(crate) struct Notifications {
    /// Notification message (ephemeral)
    pub(crate) current: Option<(String, NotificationType)>,
    /// The notification on screen and when it goes away.
    pub(crate) shown: Option<((String, NotificationType), Instant)>,
    /// When the notification on screen first showed.
    pub(crate) since: Option<Instant>,
}

impl Notifications {
    /// Start the timer of a notification seen for the first time, and drop
    /// one whose time is up.
    pub(crate) fn expire(&mut self) {
        let Some(current) = &self.current else {
            self.shown = None;
            self.since = None;
            return;
        };
        let now = Instant::now();
        match &self.shown {
            Some((shown, until)) if shown == current => {
                if now >= *until {
                    self.current = None;
                    self.shown = None;
                    self.since = None;
                }
            }
            _ => {
                let ttl = match current.1 {
                    NotificationType::Error => ERROR_NOTIFICATION_TTL,
                    NotificationType::Success | NotificationType::Warning => NOTIFICATION_TTL,
                };
                self.shown = Some((current.clone(), now + ttl));
                self.since = Some(now);
            }
        }
    }

    /// How long the notification on screen has been showing; zero before
    /// its timer starts.
    #[must_use]
    pub(crate) fn age(&self) -> Duration {
        self.since.map_or(Duration::ZERO, |since| since.elapsed())
    }

    /// Keep the notification on screen until it has scrolled for `read`,
    /// and a moment after.
    pub(crate) fn hold_for(&mut self, read: Duration) {
        if let (Some((_, until)), Some(since)) = (&mut self.shown, self.since) {
            *until = (*until).max(since + read + NOTIFICATION_TAIL);
        }
    }
}
