use scores::{mean, parse_scores};

#[test]
fn skips_what_is_not_a_score() {
    assert_eq!(parse_scores("1,x,,2, -3 ,4.5"), vec![1, 2]);
}

#[test]
fn mean_of_nothing_is_zero() {
    assert_eq!(mean(&[]), 0.0);
}
