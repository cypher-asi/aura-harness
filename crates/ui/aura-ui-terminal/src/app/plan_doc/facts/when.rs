//! A plan's times as the view shows them: `14:32 · 2h ago`, with the
//! date in front when it is not today, and the year when it is not this
//! one. Both take `now`, so the same instant always reads the same.

use chrono::{DateTime, Datelike, FixedOffset, Utc};

/// `at` as seen at `now`, in `now`'s time zone.
pub(crate) fn when(at: DateTime<Utc>, now: &DateTime<FixedOffset>) -> String {
    let local = at.with_timezone(&now.timezone());
    let clock = if local.date_naive() == now.date_naive() {
        local.format("%H:%M").to_string()
    } else if local.year() == now.year() {
        local.format("%b %-d %H:%M").to_string()
    } else {
        local.format("%Y-%m-%d %H:%M").to_string()
    };
    let age = (now.with_timezone(&Utc) - at).num_seconds();
    format!("{clock} · {}", ago(age))
}

/// How long ago something `seconds` old was: `just now`, `5m ago`,
/// `2h ago`, `3d ago`. A time in the future reads `just now`.
pub(super) fn ago(seconds: i64) -> String {
    const MINUTE: i64 = 60;
    const HOUR: i64 = 60 * MINUTE;
    const DAY: i64 = 24 * HOUR;
    match seconds {
        s if s < MINUTE => "just now".to_string(),
        s if s < HOUR => format!("{}m ago", s / MINUTE),
        s if s < DAY => format!("{}h ago", s / HOUR),
        s => format!("{}d ago", s / DAY),
    }
}
