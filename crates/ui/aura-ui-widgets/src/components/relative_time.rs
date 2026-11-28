//! Times as people say them: `just now`, `2 minutes ago`, `5 h ago`, then
//! `yesterday 14:32`, `Nov 26, 14:32` and, from another year,
//! `Nov 26 2023, 14:32`. One helper for every panel that shows when
//! something happened (a channel's QUEUE now; HISTORY, plan details and
//! MEMBERS can take it too), so a time reads the same everywhere.
//!
//! The rules, from `then` to `now`:
//!
//! | how long ago                 | says                  |
//! |------------------------------|-----------------------|
//! | under a minute, or ahead     | `just now`            |
//! | under an hour                | `1 minute ago`, `N minutes ago` |
//! | under 24 hours               | `N h ago`             |
//! | the day before today         | `yesterday 14:32`     |
//! | earlier this year            | `Nov 26, 14:32`       |
//! | an earlier year              | `Nov 26 2023, 14:32`  |
//!
//! A time from the day before that is under 24 hours old still counts in
//! hours: the hours come first. [`relative_time_changes`] says when the
//! text next changes, so a view redraws then and not every frame.
//! Everything takes `now`, so nothing here reads the clock.

use chrono::{DateTime, Datelike, Local, TimeZone};
use std::fmt::Display;
use std::time::Duration;

const MINUTE: i64 = 60;
const HOUR: i64 = 60 * MINUTE;
const DAY: i64 = 24 * HOUR;

/// `then` as said at `now`, in `now`'s time zone.
#[must_use]
pub fn relative_time<Tz: TimeZone>(then: &DateTime<Tz>, now: &DateTime<Tz>) -> String
where
    Tz::Offset: Display,
{
    let ago = now
        .clone()
        .signed_duration_since(then.clone())
        .num_seconds();
    match ago {
        s if s < MINUTE => "just now".to_string(),
        s if s < 2 * MINUTE => "1 minute ago".to_string(),
        s if s < HOUR => format!("{} minutes ago", s / MINUTE),
        s if s < DAY => format!("{} h ago", s / HOUR),
        _ => dated(then, now),
    }
}

/// An older `then` by its date: `yesterday 14:32`, `Nov 26, 14:32`, or
/// `Nov 26 2023, 14:32` from another year.
fn dated<Tz: TimeZone>(then: &DateTime<Tz>, now: &DateTime<Tz>) -> String
where
    Tz::Offset: Display,
{
    let then = then.with_timezone(&now.timezone());
    let yesterday = now.date_naive().pred_opt();
    if Some(then.date_naive()) == yesterday {
        return then.format("yesterday %H:%M").to_string();
    }
    if then.year() == now.year() {
        then.format("%b %-d, %H:%M").to_string()
    } else {
        then.format("%b %-d %Y, %H:%M").to_string()
    }
}

/// `then` in full, to the second with its offset: `2024-11-27 20:31:05
/// +01:00`. What a view shows on hover.
#[must_use]
pub fn exact_time<Tz: TimeZone>(then: &DateTime<Tz>) -> String
where
    Tz::Offset: Display,
{
    then.format("%Y-%m-%d %H:%M:%S %:z").to_string()
}

/// How long after `now` [`relative_time`] says something else for
/// `then`: the next minute while it counts minutes, the next hour while
/// it counts hours, then the next midnight, when `yesterday` becomes a
/// date and a year's last day another year's. `None` only when the next
/// midnight can't be told.
#[must_use]
pub fn relative_time_changes<Tz: TimeZone>(
    then: &DateTime<Tz>,
    now: &DateTime<Tz>,
) -> Option<Duration> {
    let ago = now
        .clone()
        .signed_duration_since(then.clone())
        .num_seconds();
    let secs = match ago {
        s if s < 0 => MINUTE - s,
        s if s < HOUR => MINUTE - s % MINUTE,
        s if s < DAY => HOUR - s % HOUR,
        _ => return until_midnight(now),
    };
    u64::try_from(secs).ok().map(Duration::from_secs)
}

/// How long from `now` to the start of the next day in its time zone.
fn until_midnight<Tz: TimeZone>(now: &DateTime<Tz>) -> Option<Duration> {
    let midnight = now.date_naive().succ_opt()?.and_hms_opt(0, 0, 0)?;
    let midnight = now.timezone().from_local_datetime(&midnight).earliest()?;
    midnight
        .signed_duration_since(now.clone())
        .to_std()
        .ok()
        .map(|left| left.max(Duration::from_secs(1)))
}

/// An RFC 3339 time in local time; `None` when it does not parse.
#[must_use]
pub fn parse_time(rfc3339: &str) -> Option<DateTime<Local>> {
    DateTime::parse_from_rfc3339(rfc3339.trim())
        .ok()
        .map(|time| time.with_timezone(&Local))
}

#[cfg(test)]
mod tests;
