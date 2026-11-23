//! What the replay estimates from the recorded routes.

use aura_core_types::{PromptCategory, RouteLevel};

use super::{as_recorded, estimate, Estimate, Logged};

fn logged(model: &str, propensity: f64, success: bool, cost_usd: f64) -> Logged {
    Logged {
        level: RouteLevel::Turn,
        category: PromptCategory::Debugging,
        difficulty: None,
        kind: None,
        model: model.to_string(),
        propensity,
        success,
        cost_usd,
    }
}

#[test]
fn the_recorded_routes_count_as_they_went() {
    let log = [logged("a", 0.5, true, 1.0), logged("a", 1.0, false, 3.0)];
    assert_eq!(
        as_recorded(&log),
        Some(Estimate {
            matched: 2,
            of: 2,
            success_rate: 0.5,
            mean_cost_usd: 2.0,
        })
    );
}

#[test]
fn rarely_picked_work_stands_for_the_times_it_was_not() {
    // "b" was explored on one in ten prompts: its one success weighs
    // like ten, against "a"'s nine.
    let mut log = vec![logged("b", 0.1, true, 0.1)];
    log.extend((0..9).map(|_| logged("a", 0.9, false, 1.0)));
    let all = estimate(&log, |_| true).unwrap();
    assert!((all.success_rate - 0.5).abs() < 1e-9, "{all:?}");
    let on_b = estimate(&log, |p| p.model == "b").unwrap();
    assert_eq!((on_b.matched, on_b.of), (1, 10));
    assert!((on_b.mean_cost_usd - 0.1).abs() < 1e-9);
}

#[test]
fn a_policy_that_matches_nothing_has_no_estimate() {
    let log = [logged("a", 1.0, true, 1.0)];
    assert_eq!(estimate(&log, |p| p.model == "z"), None);
}

#[test]
fn an_unlikely_pick_is_capped() {
    let log = [logged("a", 0.0, true, 1.0), logged("a", 1.0, false, 1.0)];
    let estimate = estimate(&log, |_| true).unwrap();
    assert!((estimate.success_rate - 100.0 / 101.0).abs() < 1e-9);
}
