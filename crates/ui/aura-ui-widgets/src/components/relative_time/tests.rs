use super::*;
use chrono::{FixedOffset, NaiveDate};

/// The clock the tests read: Wed 27 Nov 2024, 20:31:00, one hour east.
fn zone() -> FixedOffset {
    FixedOffset::east_opt(3600).unwrap()
}

fn at(y: i32, m: u32, d: u32, h: u32, min: u32, s: u32) -> DateTime<FixedOffset> {
    let naive = NaiveDate::from_ymd_opt(y, m, d)
        .unwrap()
        .and_hms_opt(h, min, s)
        .unwrap();
    zone().from_local_datetime(&naive).unwrap()
}

fn now() -> DateTime<FixedOffset> {
    at(2024, 11, 27, 20, 31, 0)
}

/// What `relative_time` says of the time `secs` before `now()`.
fn ago(secs: i64) -> String {
    relative_time(&(now() - chrono::Duration::seconds(secs)), &now())
}

#[test]
fn under_a_minute_is_just_now() {
    assert_eq!(ago(0), "just now");
    assert_eq!(ago(59), "just now");
    assert_eq!(ago(-30), "just now", "a clock a little ahead");
}

#[test]
fn minutes_up_to_an_hour() {
    assert_eq!(ago(60), "1 minute ago");
    assert_eq!(ago(119), "1 minute ago");
    assert_eq!(ago(120), "2 minutes ago");
    assert_eq!(ago(59 * 60 + 59), "59 minutes ago");
}

#[test]
fn hours_up_to_a_day() {
    assert_eq!(ago(3600), "1 h ago");
    assert_eq!(ago(5 * 3600 + 59 * 60), "5 h ago");
    assert_eq!(ago(24 * 3600 - 1), "23 h ago");
    // 21:00 the day before: yesterday, but under a day, so hours.
    assert_eq!(
        relative_time(&at(2024, 11, 26, 21, 0, 0), &now()),
        "23 h ago"
    );
}

#[test]
fn past_a_day_the_day_before_is_yesterday() {
    assert_eq!(ago(24 * 3600), "yesterday 20:31");
    assert_eq!(
        relative_time(&at(2024, 11, 26, 0, 0, 0), &now()),
        "yesterday 00:00"
    );
}

#[test]
fn before_yesterday_a_date_and_another_years_with_its_year() {
    assert_eq!(
        relative_time(&at(2024, 11, 25, 23, 59, 0), &now()),
        "Nov 25, 23:59"
    );
    assert_eq!(
        relative_time(&at(2024, 1, 1, 0, 0, 0), &now()),
        "Jan 1, 00:00"
    );
    assert_eq!(
        relative_time(&at(2023, 12, 31, 23, 59, 0), &now()),
        "Dec 31 2023, 23:59"
    );
}

#[test]
fn yesterday_crosses_the_new_year_without_its_year() {
    let new_year = at(2025, 1, 1, 10, 0, 0);
    assert_eq!(
        relative_time(&at(2024, 12, 31, 9, 0, 0), &new_year),
        "yesterday 09:00"
    );
    assert_eq!(
        relative_time(&at(2024, 12, 30, 9, 0, 0), &new_year),
        "Dec 30 2024, 09:00"
    );
}

#[test]
fn the_exact_time_has_seconds_and_offset() {
    assert_eq!(
        exact_time(&at(2024, 11, 27, 20, 31, 5)),
        "2024-11-27 20:31:05 +01:00"
    );
}

#[test]
fn the_text_changes_at_the_next_minute_hour_or_midnight() {
    let changes = |secs: i64| {
        relative_time_changes(&(now() - chrono::Duration::seconds(secs)), &now())
            .map(|left| left.as_secs())
    };
    assert_eq!(changes(0), Some(60), "just now, then 1 minute ago");
    assert_eq!(changes(59), Some(1));
    assert_eq!(changes(90), Some(30), "1 minute, then 2 minutes");
    assert_eq!(changes(-30), Some(90), "ahead: just now until a minute old");
    assert_eq!(changes(3600 + 10), Some(3590), "1 h, then 2 h");
    assert_eq!(changes(24 * 3600 - 1), Some(1), "23 h, then yesterday");
    // Dated: the next midnight, 03:29 after 20:31.
    assert_eq!(changes(2 * 24 * 3600), Some(3 * 3600 + 29 * 60));
}

#[test]
fn a_time_is_parsed_from_rfc_3339() {
    assert!(parse_time("2024-11-27T20:31:00Z").is_some());
    assert!(parse_time(" 2024-11-27T20:31:00+01:00 ").is_some());
    assert!(parse_time("yesterday").is_none());
}
