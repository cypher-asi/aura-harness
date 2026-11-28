//! The days the Archive and TASKS list their rows under: the local day a
//! time falls on, and the header naming it.

use chrono::{DateTime, Local, NaiveDate};

/// The local day `ms` (Unix milliseconds) falls on; `None` when the time
/// is unknown.
pub(crate) fn day_of(ms: Option<u64>) -> Option<NaiveDate> {
    let at = ms
        .and_then(|ms| i64::try_from(ms).ok())
        .and_then(DateTime::from_timestamp_millis)?;
    Some(at.with_timezone(&Local).date_naive())
}

/// Header of `day` as seen on `today`: `Today`, `Yesterday`, else the
/// date in full, as `Wednesday, September 16 2026`. An unknown day reads
/// `Earlier`.
pub(crate) fn day_name(day: Option<NaiveDate>, today: NaiveDate) -> String {
    let Some(day) = day else {
        return "Earlier".to_string();
    };
    if day == today {
        "Today".to_string()
    } else if today.pred_opt() == Some(day) {
        "Yesterday".to_string()
    } else {
        day.format("%A, %B %-d %Y").to_string()
    }
}
