//! Seeded draws.

use super::{Random, SplitMix64};

#[test]
fn seeded_draws_repeat_and_stay_in_range() {
    let mut a = SplitMix64::seeded(42);
    let mut b = SplitMix64::seeded(42);
    for _ in 0..1_000 {
        let x = a.unit();
        assert_eq!(x, b.unit());
        assert!((0.0..1.0).contains(&x));
    }
    let mut clock = SplitMix64::from_clock();
    assert!((0.0..1.0).contains(&clock.unit()));
}
