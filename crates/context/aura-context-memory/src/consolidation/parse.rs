//! Parsers for the line-oriented evolution actions the model returns.

/// Parse two 1-based indices from "N M ..." into 0-based, validating bounds.
pub(super) fn parse_merge_indices(text: &str, max: usize) -> Option<(usize, usize)> {
    let mut parts = text.split_whitespace();
    let src = parts.next()?.parse::<usize>().ok()?.checked_sub(1)?;
    let dst = parts.next()?.parse::<usize>().ok()?.checked_sub(1)?;
    if src < max && dst < max && src != dst {
        Some((src, dst))
    } else {
        None
    }
}

/// Parse a single 1-based index from "N ..." into 0-based, validating bounds.
pub(super) fn parse_single_index(text: &str, max: usize) -> Option<usize> {
    let idx = text
        .split_whitespace()
        .next()?
        .parse::<usize>()
        .ok()?
        .checked_sub(1)?;
    if idx < max {
        Some(idx)
    } else {
        None
    }
}

/// Extract a quoted or bare value after the given `prefix=` marker.
pub(super) fn extract_quoted_value(text: &str, prefix: &str) -> Option<String> {
    let start = text.find(prefix)? + prefix.len();
    let rest = &text[start..];
    if let Some(stripped) = rest.strip_prefix('"') {
        let end = stripped.find('"')?;
        Some(stripped[..end].to_string())
    } else {
        let end = rest.find(|c: char| c.is_whitespace()).unwrap_or(rest.len());
        Some(rest[..end].to_string())
    }
}

/// Extract a float value after the given `prefix=` marker.
pub(super) fn extract_float_value(text: &str, prefix: &str) -> Option<f32> {
    let start = text.find(prefix)? + prefix.len();
    let end = text[start..]
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .map_or(text.len(), |e| start + e);
    text[start..end].parse().ok()
}
