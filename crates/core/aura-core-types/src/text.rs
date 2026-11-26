//! Shared text utilities.

/// `text` cut to at most `max_chars` characters, with `…` appended when
/// it was cut. The `…` is not counted toward `max_chars`, so a cut result
/// has `max_chars + 1` characters.
#[must_use]
pub fn truncate_with_ellipsis(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut cut: String = text.chars().take(max_chars).collect();
    cut.push('…');
    cut
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_with_ellipsis_cases() {
        let cases = [
            ("", 0, ""),
            ("abc", 0, "…"),
            ("abc", 3, "abc"),
            ("abc", 5, "abc"),
            ("abcdef", 3, "abc…"),
            ("héllo wörld", 5, "héllo…"),
            ("日本語テキスト", 3, "日本語…"),
            ("日本語", 3, "日本語"),
        ];
        for (text, max, expected) in cases {
            assert_eq!(
                truncate_with_ellipsis(text, max),
                expected,
                "text={text:?} max={max}"
            );
        }
    }
}
