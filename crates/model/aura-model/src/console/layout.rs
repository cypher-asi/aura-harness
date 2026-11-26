//! Row layout and value formatting shared by every console transcript.
//!
//! The outbound model blocks in [`super`], the task / turn / tool blocks
//! in `aura-agent::console` and the inbound HTTP / WS blocks in
//! `aura-node::inbound_console` all render through these helpers, so a
//! row looks the same no matter which crate emitted it.
//!
//! Rows render as `│   <label padded to 14> <value>`. Long values are
//! pre-wrapped so continuation lines start under the value column
//! instead of letting the terminal soft-wrap them back to column 0. The
//! wrap is gated on TTY detection: when stdout is not a terminal (e.g.
//! piped to a file) every row stays on a single line so log files are
//! byte-identical to the pre-wrap layout.

use colored::Colorize;

/// Width of the padded label column in [`wrap_row`].
const LABEL_WIDTH: usize = 14;

/// Display columns before the value chunk starts:
/// `│` (1) + three spaces (3) + padded label (14) + separator space (1) = 19.
const ROW_INDENT_COLS: usize = 1 + 3 + LABEL_WIDTH + 1;

/// Minimum value column room before wrap is attempted. Narrower
/// terminals fall back to single-line rendering rather than emit a
/// wrap that's mostly continuation prefix.
const MIN_VALUE_ROOM: usize = 16;

/// Render `[destination · host]` (host optional) styled as dim cyan
/// so it sits subtly next to the bold header text without fighting
/// the status-family color.
#[must_use]
pub fn destination_tag(destination: &str, host: Option<&str>) -> String {
    let body = match host {
        Some(h) if !h.is_empty() => format!("[{destination} · {h}]"),
        _ => format!("[{destination}]"),
    };
    body.cyan().dimmed().to_string()
}

/// Render a labeled row, wrapping long values so continuation lines
/// stay indented under the value column (column 19).
///
/// When stdout is not a TTY (or `terminal_size` reports nothing),
/// this falls back to the single-line layout so log files stay
/// byte-identical to the pre-wrap layout.
#[must_use]
pub fn wrap_row(label: &str, value: &str) -> String {
    let padded_label = format!("{label:<LABEL_WIDTH$}");
    let first_prefix = format!("{}   {} ", "│".dimmed(), padded_label.bold());
    let cont_prefix = format!("{}{}", "│".dimmed(), " ".repeat(ROW_INDENT_COLS - 1));

    let Some(term_width) = detect_term_width() else {
        return format!("{first_prefix}{value}\n");
    };
    if term_width <= ROW_INDENT_COLS + MIN_VALUE_ROOM {
        return format!("{first_prefix}{value}\n");
    }
    let value_room = term_width - ROW_INDENT_COLS;

    if value.chars().count() <= value_room {
        return format!("{first_prefix}{value}\n");
    }

    let chunks = wrap_value_chunks(value, value_room);
    let mut out = String::new();
    for (i, chunk) in chunks.iter().enumerate() {
        if i == 0 {
            out.push_str(&first_prefix);
        } else {
            out.push_str(&cont_prefix);
        }
        out.push_str(chunk);
        out.push('\n');
    }
    out
}

/// Detect terminal width in columns. Returns `None` when stdout is
/// not a TTY (e.g. piped to a file) so callers can skip wrapping.
fn detect_term_width() -> Option<usize> {
    use std::io::IsTerminal;
    if !std::io::stdout().is_terminal() {
        return None;
    }
    terminal_size::terminal_size().map(|(terminal_size::Width(w), _)| w as usize)
}

/// Split `value` into chunks of at most `max` chars, preferring to
/// break after the last comma or whitespace in the window so that
/// comma-separated lists (e.g. the `headers` row) wrap cleanly
/// between items. Falls back to a hard split when no preferred
/// break point is found.
fn wrap_value_chunks(value: &str, max: usize) -> Vec<String> {
    if max == 0 {
        return vec![value.to_string()];
    }
    let chars: Vec<char> = value.chars().collect();
    let mut chunks: Vec<String> = Vec::new();
    let mut start = 0usize;
    while start < chars.len() {
        let remaining = chars.len() - start;
        if remaining <= max {
            chunks.push(chars[start..].iter().collect());
            break;
        }
        let window_end = start + max;
        // Prefer last comma inside the window (split *after* the comma
        // so the punctuation stays attached to the preceding token).
        let split_at = chars[start..window_end]
            .iter()
            .rposition(|c| *c == ',')
            .map(|pos| start + pos + 1)
            .or_else(|| {
                chars[start..window_end]
                    .iter()
                    .rposition(|c| c.is_whitespace())
                    .map(|pos| start + pos + 1)
            })
            .unwrap_or(window_end);
        // Defensive: never make zero progress.
        let split_at = split_at.max(start + 1);
        let chunk: String = chars[start..split_at].iter().collect();
        chunks.push(chunk.trim_end().to_string());
        // Skip any whitespace at the start of the next chunk so the
        // continuation lines up cleanly under the value column.
        let mut next = split_at;
        while next < chars.len() && chars[next].is_whitespace() {
            next += 1;
        }
        start = next;
    }
    chunks
}

/// Converts a count to `f64` for rounded display.
#[allow(
    clippy::cast_precision_loss,
    reason = "transcript formatting rounds to one or two decimals"
)]
fn display_f64(n: u64) -> f64 {
    n as f64
}

/// Format a byte count as `512 B`, `1.5 KB` or `2.00 MB`.
#[must_use]
pub fn human_bytes(n: usize) -> String {
    if n < 1024 {
        format!("{n} B")
    } else if n < 1024 * 1024 {
        let kb = display_f64(n as u64) / 1024.0;
        format!("{kb:.1} KB")
    } else {
        let mb = display_f64(n as u64) / (1024.0 * 1024.0);
        format!("{mb:.2} MB")
    }
}

/// Format a millisecond duration as `250 ms`, `4.20 s` or `1m 1.5s`.
#[must_use]
pub fn human_duration_ms(ms: u64) -> String {
    if ms < 1000 {
        format!("{ms} ms")
    } else if ms < 60_000 {
        let s = display_f64(ms) / 1000.0;
        format!("{s:.2} s")
    } else {
        let m = ms / 60_000;
        let s = display_f64(ms % 60_000) / 1000.0;
        format!("{m}m {s:.1}s")
    }
}

/// Collapse control chars, interior whitespace and double quotes into
/// a single inline preview clipped to `limit` chars (plus `…`). Used
/// for body / error rows where the upstream content (e.g. a Cloudflare
/// HTML page) would otherwise smash the box layout if rendered
/// verbatim.
#[must_use]
pub fn collapse_for_row(content: &str, limit: usize) -> String {
    let collapsed: String = content
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let trimmed = collapsed
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace('"', "'");
    if trimmed.chars().count() <= limit {
        trimmed
    } else {
        let head: String = trimmed.chars().take(limit).collect();
        format!("{head}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_bytes_units() {
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1536), "1.5 KB");
        assert_eq!(human_bytes(2 * 1024 * 1024), "2.00 MB");
    }

    #[test]
    fn human_duration_units() {
        assert_eq!(human_duration_ms(250), "250 ms");
        assert_eq!(human_duration_ms(4200), "4.20 s");
        assert_eq!(human_duration_ms(61_500), "1m 1.5s");
    }

    #[test]
    fn destination_tag_renders_host_when_present() {
        colored::control::set_override(false);
        let tag = destination_tag("aura-network", Some("aura-router.onrender.com"));
        assert_eq!(tag, "[aura-network · aura-router.onrender.com]");
        let tag = destination_tag("aura-network", None);
        assert_eq!(tag, "[aura-network]");
        colored::control::unset_override();
    }

    #[test]
    fn wrap_value_breaks_on_comma_preference() {
        for value in ["aaa,bbb,ccc,ddd,eee", "aaa,bbb,ccc,ddd,eee,fff,ggg"] {
            let chunks = wrap_value_chunks(value, 12);
            // Every non-final chunk should end with a comma (proves the
            // preferred break point was used).
            for chunk in &chunks[..chunks.len() - 1] {
                assert!(
                    chunk.ends_with(','),
                    "expected comma-terminated chunk, got {chunk:?}"
                );
            }
            assert_eq!(chunks.join(""), value);
        }
    }

    #[test]
    fn wrap_value_breaks_on_whitespace_when_no_comma() {
        for value in [
            "alpha beta gamma delta epsilon",
            "alpha beta gamma delta epsilon zeta",
        ] {
            let chunks = wrap_value_chunks(value, 12);
            assert!(chunks.len() > 1, "expected wrap, got {chunks:?}");
            for chunk in &chunks {
                assert!(chunk.chars().count() <= 12, "chunk over budget: {chunk:?}");
                // Leading whitespace is consumed at chunk boundaries.
                assert!(
                    !chunk.starts_with(' '),
                    "leading whitespace not trimmed: {chunk:?}"
                );
            }
        }
    }

    #[test]
    fn wrap_value_hard_splits_when_no_break() {
        let chunks = wrap_value_chunks("aaaaaaaaaaaaaaaaa", 5);
        for chunk in &chunks {
            assert!(chunk.chars().count() <= 5);
        }
        assert_eq!(chunks.concat(), "aaaaaaaaaaaaaaaaa");
    }

    #[test]
    fn wrap_row_no_tty_returns_single_line() {
        // In `cargo test` stdout is captured (not a TTY), so
        // `detect_term_width` returns None and we get the single-line
        // layout — with colors disabled so the assertion can match
        // plain bytes.
        colored::control::set_override(false);
        let out = wrap_row("model", "claude-opus-4-6");
        assert_eq!(out, "│   model          claude-opus-4-6\n");
        colored::control::unset_override();
    }

    #[test]
    fn collapse_for_row_strips_control_and_whitespace() {
        let raw = "<!DOCTYPE html>\n  <html>\r\n  <body>  blocked  </body>\n</html>";
        let collapsed = collapse_for_row(raw, 200);
        assert!(!collapsed.contains('\n'));
        assert!(!collapsed.contains('\r'));
        assert!(!collapsed.contains("  "));
        assert!(collapsed.contains("<html> <body> blocked </body> </html>"));
        assert_eq!(
            collapse_for_row("line1\nline2\r\nline3 ", 200),
            "line1 line2 line3"
        );
    }

    #[test]
    fn collapse_for_row_replaces_quotes_and_clips() {
        let s = "hello\n  world  \"quoted\" extra extra extra extra extra extra";
        let out = collapse_for_row(s, 30);
        assert!(out.starts_with("hello world 'quoted'"));
        assert!(out.ends_with('…') || out.len() <= 30);
    }

    #[test]
    fn collapse_for_row_truncates_with_ellipsis() {
        let raw = "a".repeat(200);
        let collapsed = collapse_for_row(&raw, 50);
        assert!(collapsed.ends_with('…'));
        assert_eq!(collapsed.chars().count(), 51);
    }
}
