/// Parse comma-separated scores, skipping blanks and anything that is
/// not a non-negative whole number.
pub fn parse_scores(input: &str) -> Vec<u32> {
    input
        .split(',')
        .map(str::trim)
        .filter_map(|s| s.parse::<u32>().ok())
        .collect()
}
