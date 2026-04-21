//! Redaction helper for error strings propagated to users or logs.
//!
//! Centralises the scrub we want before a raw [`anyhow::Error`] /
//! [`std::io::Error`] / upstream API message crosses a trust boundary.
//! The existing [`crate::zos_client`] truncation covered the zOS body;
//! this module generalises it and adds a handful of cheap pattern
//! strippers so absolute paths, session UUIDs, and long token / digest
//! blobs don't escape into logs or HTTP error bodies.
//!
//! Intentionally regex-free — `aura-auth` already compiles on a minimal
//! dep surface and we don't want to pull `regex` in just for this.
//! The four patterns are narrow enough to hand-roll cleanly and the
//! helper runs only on error paths, so the scan cost is immaterial.
//!
//! Covered:
//! - Windows home dirs: `C:\Users\<name>` → `<USER_HOME>`
//! - Unix home dirs: `/Users/<name>`, `/home/<name>` → `<USER_HOME>`
//! - Canonical UUIDs (8-4-4-4-12) → `<UUID>`
//! - Long hex blobs (≥ 32 chars) → `<HEX>`
//! - Hard cap at [`MAX_LEN`] chars, with `…` suffix on overflow

/// Hard cap on the returned string length (counted in `char`s, not
/// bytes, so we never slice mid-codepoint).
const MAX_LEN: usize = 200;

/// Scrub and cap an error-like string.
///
/// Safe to call on any `&str` / `String` — on short, already-clean
/// input it's effectively a copy. See module docs for the patterns
/// this covers.
pub fn redact_error(msg: impl AsRef<str>) -> String {
    let s = msg.as_ref();
    // Order matters: home dirs first (so paths don't get partially
    // re-matched by the hex rule on their drive letters), then UUIDs
    // (which otherwise trip the long-hex rule with their dashes
    // straddling a 32-char run), then long hex blobs.
    let s = redact_home_dirs(s);
    let s = redact_uuids(&s);
    let s = redact_long_hex(&s);
    truncate(&s, MAX_LEN)
}

fn redact_home_dirs(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < bytes.len() {
        if let Some(end) = match_windows_home(bytes, i) {
            out.push_str("<USER_HOME>");
            i = end;
            continue;
        }
        if let Some(end) = match_unix_home(bytes, i) {
            out.push_str("<USER_HOME>");
            i = end;
            continue;
        }
        // Copy the full UTF-8 codepoint at position `i` so we don't
        // corrupt multi-byte characters. `chars().next()` on `s[i..]`
        // is safe because `i` sat on a char boundary before entering
        // this branch.
        let c = s[i..].chars().next().expect("bytes remain => char");
        let step = c.len_utf8();
        out.push_str(&s[i..i + step]);
        i += step;
    }
    out
}

fn match_windows_home(bytes: &[u8], start: usize) -> Option<usize> {
    // `<letter>:\Users\<name>` — consume name up to the next `\`
    // (or the end of the string) so longer paths collapse to the
    // home-dir prefix. Drive-letter + "Users" are matched
    // case-insensitively to cover `c:\users\...` as well.
    const PREFIX_LEN: usize = 9; // letter + ':' + '\' + "Users" + '\'
    if bytes.len() < start + PREFIX_LEN {
        return None;
    }
    if !bytes[start].is_ascii_alphabetic() {
        return None;
    }
    if bytes[start + 1] != b':' || bytes[start + 2] != b'\\' {
        return None;
    }
    if !bytes[start + 3..start + 8].eq_ignore_ascii_case(b"Users") {
        return None;
    }
    if bytes[start + 8] != b'\\' {
        return None;
    }
    let name_start = start + PREFIX_LEN;
    let mut i = name_start;
    while i < bytes.len() && bytes[i] != b'\\' {
        i += 1;
    }
    if i == name_start {
        // Pattern requires at least one name character.
        return None;
    }
    Some(i)
}

fn match_unix_home(bytes: &[u8], start: usize) -> Option<usize> {
    if bytes.get(start) != Some(&b'/') {
        return None;
    }
    let rest = &bytes[start + 1..];
    let prefix_len = if rest.len() >= 6 && rest[..6].eq_ignore_ascii_case(b"Users/") {
        6
    } else if rest.len() >= 5 && rest[..5].eq_ignore_ascii_case(b"home/") {
        5
    } else {
        return None;
    };
    let name_start = start + 1 + prefix_len;
    let mut i = name_start;
    while i < bytes.len() && bytes[i] != b'/' {
        i += 1;
    }
    if i == name_start {
        return None;
    }
    Some(i)
}

fn redact_uuids(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < bytes.len() {
        if let Some(end) = match_uuid(bytes, i) {
            out.push_str("<UUID>");
            i = end;
            continue;
        }
        let c = s[i..].chars().next().expect("bytes remain => char");
        let step = c.len_utf8();
        out.push_str(&s[i..i + step]);
        i += step;
    }
    out
}

fn match_uuid(bytes: &[u8], start: usize) -> Option<usize> {
    // 8-4-4-4-12 with `-` separators — 36 chars total.
    const GROUPS: [usize; 5] = [8, 4, 4, 4, 12];
    const TOTAL: usize = 8 + 4 + 4 + 4 + 12 + 4;
    if bytes.len() < start + TOTAL {
        return None;
    }
    let mut i = start;
    for (idx, &len) in GROUPS.iter().enumerate() {
        for _ in 0..len {
            if !bytes[i].is_ascii_hexdigit() {
                return None;
            }
            i += 1;
        }
        if idx < GROUPS.len() - 1 {
            if bytes[i] != b'-' {
                return None;
            }
            i += 1;
        }
    }
    Some(i)
}

fn redact_long_hex(s: &str) -> String {
    const MIN_HEX_RUN: usize = 32;
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_hexdigit() {
            let run_start = i;
            while i < bytes.len() && bytes[i].is_ascii_hexdigit() {
                i += 1;
            }
            if i - run_start >= MIN_HEX_RUN {
                out.push_str("<HEX>");
            } else {
                out.push_str(&s[run_start..i]);
            }
        } else {
            let c = s[i..].chars().next().expect("bytes remain => char");
            let step = c.len_utf8();
            out.push_str(&s[i..i + step]);
            i += step;
        }
    }
    out
}

fn truncate(s: &str, max_chars: usize) -> String {
    let count = s.chars().count();
    if count <= max_chars {
        return s.to_string();
    }
    let short: String = s.chars().take(max_chars).collect();
    format!("{short}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_is_empty() {
        assert_eq!(redact_error(""), "");
    }

    #[test]
    fn short_clean_passes_through() {
        assert_eq!(redact_error("bad request"), "bad request");
    }

    #[test]
    fn redacts_windows_home() {
        let got = redact_error(r"failed to read C:\Users\alice\docs\x.txt: not found");
        assert!(
            got.contains("<USER_HOME>"),
            "expected USER_HOME marker in {got:?}"
        );
        assert!(
            !got.contains("alice"),
            "username should be stripped in {got:?}"
        );
        assert!(got.contains(r"\docs\x.txt"), "tail should survive: {got:?}");
    }

    #[test]
    fn redacts_windows_home_case_insensitive() {
        let got = redact_error(r"d:\users\bob\file");
        assert!(got.contains("<USER_HOME>"));
        assert!(!got.contains("bob"));
    }

    #[test]
    fn redacts_unix_home_users() {
        let got = redact_error("open /Users/alice/.config/aura/creds.json failed");
        assert!(got.contains("<USER_HOME>"));
        assert!(!got.contains("alice"));
        assert!(got.contains("/.config/aura/creds.json"));
    }

    #[test]
    fn redacts_unix_home_slash_home() {
        let got = redact_error("stat /home/carol/proj/x: not found");
        assert!(got.contains("<USER_HOME>"));
        assert!(!got.contains("carol"));
    }

    #[test]
    fn unix_home_needs_name_segment() {
        // Bare `/home/` or `/Users/` with nothing after should NOT match —
        // without a username the pattern isn't really leaking anything.
        let got = redact_error("/home/");
        assert_eq!(got, "/home/");
    }

    #[test]
    fn redacts_uuid() {
        let got = redact_error("agent 550e8400-e29b-41d4-a716-446655440000 failed");
        assert!(got.contains("<UUID>"), "got={got:?}");
        assert!(!got.contains("550e8400"));
    }

    #[test]
    fn redacts_uppercase_uuid() {
        let got = redact_error("550E8400-E29B-41D4-A716-446655440000");
        assert_eq!(got, "<UUID>");
    }

    #[test]
    fn redacts_long_hex_blob() {
        let digest = "a".repeat(64);
        let got = redact_error(format!("blake3 digest {digest} mismatch"));
        assert!(got.contains("<HEX>"), "got={got:?}");
        assert!(!got.contains(&digest));
    }

    #[test]
    fn short_hex_is_preserved() {
        // 16 chars — below the 32-char threshold, so it's legitimate
        // short output (e.g. an error code) and should pass through.
        let got = redact_error("error code deadbeefcafebabe please");
        assert!(got.contains("deadbeefcafebabe"), "got={got:?}");
    }

    #[test]
    fn caps_at_200_chars_with_ellipsis() {
        let raw = "x".repeat(500);
        let got = redact_error(&raw);
        assert_eq!(got.chars().count(), 201); // 200 + `…`
        assert!(got.ends_with('…'));
    }

    #[test]
    fn truncation_respects_codepoint_boundaries() {
        // 4-byte chars — naive byte-slicing would panic on a
        // non-codepoint boundary. `chars().take()` keeps us safe.
        let raw: String = "𝔸".repeat(300);
        let got = redact_error(&raw);
        assert!(got.ends_with('…'));
        // Must still be valid UTF-8 (inherent to `String`) and have
        // exactly 201 codepoints.
        assert_eq!(got.chars().count(), 201);
    }

    #[test]
    fn combines_all_patterns() {
        let raw = format!(
            "write to C:\\Users\\n3o\\.aura\\sessions\\{} failed: token {} rejected",
            "11111111-2222-3333-4444-555555555555",
            "f".repeat(64),
        );
        let got = redact_error(&raw);
        assert!(got.contains("<USER_HOME>"));
        assert!(got.contains("<UUID>"));
        assert!(got.contains("<HEX>"));
        assert!(!got.contains("n3o"));
    }

    #[test]
    fn home_dir_inside_longer_sentence_leaves_tail() {
        let got = redact_error(r"prefix C:\Users\eve\x and more");
        // Name is stripped, but the path tail after the username
        // and the trailing "and more" should remain.
        assert!(got.contains("<USER_HOME>"));
        assert!(got.contains(r"\x and more"));
        assert!(!got.contains("eve"));
    }
}
