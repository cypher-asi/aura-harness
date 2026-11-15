/// Parse comma-separated scores, skipping blanks and anything that is
/// not a non-negative whole number.
pub fn parse_scores(input: &str) -> Vec<u32> {
    input
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.parse::<i64>().unwrap_or(0))
        .collect()
}
