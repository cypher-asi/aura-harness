use super::report::{amount, dollars, judge_cost};

#[test]
fn eval_costs_say_when_some_prices_are_unknown() {
    assert_eq!(amount(0.12, 0), "0.1200");
    assert_eq!(amount(0.12, 3), "0.1200+?");
    assert_eq!(amount(0.0, 3), "unknown");
    assert_eq!(dollars(0.12, 0), "$0.1200");
    assert_eq!(dollars(0.12, 1), "$0.1200+?");
    assert_eq!(dollars(0.0, 1), "unknown cost");
    assert_eq!(judge_cost(0.02, 0), "$0.0200");
    assert_eq!(judge_cost(0.02, 1), "$0.0200+?");
    assert_eq!(judge_cost(0.0, 1), "an unknown amount");
}
