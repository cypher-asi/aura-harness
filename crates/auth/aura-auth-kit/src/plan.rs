//! Display names for provider plan ids.

/// `"pro"` → `"Pro"`, `"max_20x"` → `"Max 20x"`. `None` for an empty id.
#[must_use]
pub fn plan_label(raw: &str) -> Option<String> {
    let words: Vec<String> = raw
        .split(['_', '-', ' '])
        .filter(|w| !w.is_empty())
        .map(capitalize)
        .collect();
    (!words.is_empty()).then(|| words.join(" "))
}

fn capitalize(word: &str) -> String {
    let mut chars = word.chars();
    chars.next().map_or_else(String::new, |first| {
        first
            .to_uppercase()
            .chain(chars.flat_map(char::to_lowercase))
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels() {
        assert_eq!(plan_label("plus").as_deref(), Some("Plus"));
        assert_eq!(plan_label("max_20x").as_deref(), Some("Max 20x"));
        assert_eq!(plan_label("ENTERPRISE").as_deref(), Some("Enterprise"));
        assert_eq!(plan_label(" _ "), None);
    }
}
