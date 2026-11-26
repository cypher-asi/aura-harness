//! Shared time utilities.

/// Current wall-clock time as milliseconds since the Unix epoch.
#[must_use]
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// Seconds as `2h 14m`, `4m`, `35s`, `3d 2h`.
#[must_use]
pub fn format_duration(secs: u64) -> String {
    let (d, h, m) = (secs / 86_400, (secs % 86_400) / 3600, (secs % 3600) / 60);
    if d > 0 {
        format!("{d}d {h}h")
    } else if h > 0 {
        format!("{h}h {m}m")
    } else if m > 0 {
        format!("{m}m")
    } else {
        format!("{secs}s")
    }
}

#[cfg(test)]
mod tests {
    use super::format_duration;

    #[test]
    fn durations_show_their_two_largest_units() {
        assert_eq!(format_duration(35), "35s");
        assert_eq!(format_duration(240), "4m");
        assert_eq!(format_duration(8040), "2h 14m");
        assert_eq!(format_duration(3 * 86_400 + 7200), "3d 2h");
    }
}
