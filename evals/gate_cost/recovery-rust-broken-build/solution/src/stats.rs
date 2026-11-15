/// Mean of `scores`, or 0.0 when there are none.
pub fn mean(scores: &[u32]) -> f64 {
    if scores.is_empty() {
        return 0.0;
    }
    let total: u64 = scores.iter().map(|&s| u64::from(s)).sum();
    total as f64 / scores.len() as f64
}
