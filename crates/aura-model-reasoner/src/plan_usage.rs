//! Reading a plan's usage from the response headers of a call made on it.
//!
//! Two header families are read; anything else, a missing header or a
//! value that does not parse means no reading for that window:
//!
//! - Claude plans (OAuth sign-in): `anthropic-ratelimit-unified-<window>-utilization`
//!   (fraction used, 0 to 1) and `anthropic-ratelimit-unified-<window>-reset`
//!   (Unix seconds), where `<window>` is `5h`, `7d`, `7d_opus`, ... These
//!   are what Claude Code reads; Anthropic does not document them. API-key
//!   headers (`anthropic-ratelimit-tokens-*`, ...) describe per-minute
//!   organization limits, not a plan, and are not read.
//! - ChatGPT plans: `x-codex-{primary,secondary}-used-percent` (0 to 100),
//!   `-window-minutes` and `-reset-at` (Unix seconds), or the older
//!   `-reset-after-seconds`, as the Codex CLI reads them.

use aura_core_types::{Plan, PlanUsage};
use reqwest::header::HeaderMap;

const UNIFIED_PREFIX: &str = "anthropic-ratelimit-unified-";
const UTILIZATION_SUFFIX: &str = "-utilization";

/// Most windows kept from one response, so a misbehaving upstream can't
/// grow the record.
const MAX_WINDOWS: usize = 8;

/// Every plan window `headers` report, in a stable order. `now_unix` turns
/// a relative reset into a time.
#[must_use]
pub fn from_headers(headers: &HeaderMap, now_unix: u64) -> Vec<PlanUsage> {
    let mut readings = claude(headers);
    readings.extend(chatgpt(headers, now_unix));
    readings.truncate(MAX_WINDOWS);
    readings
}

/// [`from_headers`] at the current time.
#[must_use]
pub fn from_headers_now(headers: &HeaderMap) -> Vec<PlanUsage> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    from_headers(headers, now)
}

fn text<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name)?.to_str().ok().map(str::trim)
}

fn fraction(raw: &str) -> Option<f64> {
    raw.parse::<f64>()
        .ok()
        .filter(|v| v.is_finite() && *v >= 0.0)
}

fn unix_seconds(raw: &str) -> Option<u64> {
    raw.parse::<u64>().ok().filter(|v| *v > 0)
}

fn claude(headers: &HeaderMap) -> Vec<PlanUsage> {
    let mut readings: Vec<PlanUsage> = headers
        .keys()
        .filter_map(|name| {
            let window = name
                .as_str()
                .strip_prefix(UNIFIED_PREFIX)?
                .strip_suffix(UTILIZATION_SUFFIX)?;
            let valid = !window.is_empty()
                && window
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_');
            if !valid {
                return None;
            }
            let used = fraction(text(headers, name.as_str())?)?;
            let resets_at =
                text(headers, &format!("{UNIFIED_PREFIX}{window}-reset")).and_then(unix_seconds);
            Some(PlanUsage {
                plan: Plan::Claude,
                window: window.to_string(),
                used,
                resets_at,
            })
        })
        .collect();
    readings.sort_by(|a, b| a.window.cmp(&b.window));
    readings
}

fn chatgpt(headers: &HeaderMap, now_unix: u64) -> Vec<PlanUsage> {
    ["primary", "secondary"]
        .into_iter()
        .filter_map(|slot| {
            let header = |field: &str| text(headers, &format!("x-codex-{slot}-{field}"));
            let percent = fraction(header("used-percent")?)?;
            let minutes = header("window-minutes").and_then(|v| v.parse::<u64>().ok());
            let resets_at = header("reset-at").and_then(unix_seconds).or_else(|| {
                header("reset-after-seconds")
                    .and_then(|v| v.parse::<u64>().ok())
                    .map(|after| now_unix.saturating_add(after))
            });
            // Codex treats an all-zero window as no data.
            let has_data = percent > 0.0 || minutes.is_some_and(|m| m > 0) || resets_at.is_some();
            has_data.then(|| PlanUsage {
                plan: Plan::Chatgpt,
                window: minutes
                    .filter(|m| *m > 0)
                    .map_or_else(|| slot.to_string(), window_name),
                used: percent / 100.0,
                resets_at,
            })
        })
        .collect()
}

/// `300` → `5h`, `10080` → `7d`, `90` → `90m`.
fn window_name(minutes: u64) -> String {
    const DAY: u64 = 24 * 60;
    if minutes % DAY == 0 {
        format!("{}d", minutes / DAY)
    } else if minutes % 60 == 0 {
        format!("{}h", minutes / 60)
    } else {
        format!("{minutes}m")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::{HeaderName, HeaderValue};

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(
                HeaderName::from_bytes(name.as_bytes()).unwrap(),
                HeaderValue::from_str(value).unwrap(),
            );
        }
        map
    }

    fn window(plan: Plan, window: &str, used: f64, resets_at: Option<u64>) -> PlanUsage {
        PlanUsage {
            plan,
            window: window.to_string(),
            used,
            resets_at,
        }
    }

    #[test]
    fn claude_unified_windows_are_read() {
        let map = headers(&[
            ("anthropic-ratelimit-unified-status", "allowed"),
            ("anthropic-ratelimit-unified-reset", "1764554400"),
            ("anthropic-ratelimit-unified-7d-utilization", "0.53"),
            ("anthropic-ratelimit-unified-5h-utilization", "0.018"),
            ("anthropic-ratelimit-unified-5h-reset", "1764554400"),
            ("anthropic-ratelimit-unified-5h-status", "allowed"),
            ("anthropic-ratelimit-unified-7d_opus-utilization", "0.2"),
            ("anthropic-ratelimit-tokens-remaining", "40000"),
            ("x-request-id", "req_1"),
        ]);
        assert_eq!(
            from_headers(&map, 0),
            [
                window(Plan::Claude, "5h", 0.018, Some(1_764_554_400)),
                window(Plan::Claude, "7d", 0.53, None),
                window(Plan::Claude, "7d_opus", 0.2, None),
            ]
        );
    }

    #[test]
    fn chatgpt_codex_windows_are_read() {
        let map = headers(&[
            ("x-codex-primary-used-percent", "12.5"),
            ("x-codex-primary-window-minutes", "300"),
            ("x-codex-primary-reset-at", "1704069000"),
            ("x-codex-secondary-used-percent", "80"),
            ("x-codex-secondary-window-minutes", "10080"),
            ("x-codex-secondary-reset-after-seconds", "600"),
        ]);
        assert_eq!(
            from_headers(&map, 1_000),
            [
                window(Plan::Chatgpt, "5h", 0.125, Some(1_704_069_000)),
                window(Plan::Chatgpt, "7d", 0.8, Some(1_600)),
            ]
        );
        let no_window = headers(&[("x-codex-primary-used-percent", "3")]);
        assert_eq!(
            from_headers(&no_window, 0),
            [window(Plan::Chatgpt, "primary", 0.03, None)]
        );
        assert_eq!(window_name(90), "90m");
        assert_eq!(window_name(1440), "1d");
    }

    #[test]
    fn missing_headers_give_no_reading() {
        assert!(from_headers(&HeaderMap::new(), 0).is_empty());
        let api_key_only = headers(&[
            ("anthropic-ratelimit-tokens-limit", "80000"),
            ("anthropic-ratelimit-requests-remaining", "49"),
            ("x-ratelimit-remaining-tokens", "1000"),
            ("retry-after", "3"),
        ]);
        assert!(from_headers(&api_key_only, 0).is_empty());
        let reset_without_usage = headers(&[
            ("anthropic-ratelimit-unified-5h-reset", "1764554400"),
            ("x-codex-primary-window-minutes", "300"),
        ]);
        assert!(from_headers(&reset_without_usage, 0).is_empty());
        let all_zero = headers(&[("x-codex-primary-used-percent", "0")]);
        assert!(from_headers(&all_zero, 0).is_empty());
    }

    #[test]
    fn malformed_values_give_no_reading_for_that_window() {
        let map = headers(&[
            ("anthropic-ratelimit-unified-5h-utilization", "lots"),
            ("anthropic-ratelimit-unified-7d-utilization", "-0.1"),
            ("anthropic-ratelimit-unified-1d-utilization", "NaN"),
            ("anthropic-ratelimit-unified--utilization", "0.5"),
            ("anthropic-ratelimit-unified-a-b-utilization", "0.5"),
            ("anthropic-ratelimit-unified-7d_sonnet-utilization", "0.4"),
            ("anthropic-ratelimit-unified-7d_sonnet-reset", "soon"),
            ("x-codex-primary-used-percent", "12%"),
            ("x-codex-secondary-used-percent", "inf"),
        ]);
        assert_eq!(
            from_headers(&map, 0),
            [window(Plan::Claude, "7d_sonnet", 0.4, None)],
            "a bad reset drops only the reset"
        );
    }
}
