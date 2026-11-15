mod parse;
mod stats;

pub use parse::parse_scores;
pub use stats::mean;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_averages() {
        let scores = parse_scores("3, 4,5");
        assert_eq!(scores, vec![3, 4, 5]);
        assert!((mean(&scores) - 4.0).abs() < 1e-9);
    }

    #[test]
    fn mean_keeps_fractions() {
        assert!((mean(&[1, 2]) - 1.5).abs() < 1e-9);
    }
}
