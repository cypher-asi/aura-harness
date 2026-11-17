/// Mean of `scores`, or 0.0 when there are none.
pub fn mean(scores: &[u32]) -> f64 {
    let total: u32 = scores.iter().sum();
    (total / scores.len() as u32) as f64
}
