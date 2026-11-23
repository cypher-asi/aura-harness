//! The list of past comparisons, one line each.

use super::super::list::list;
use super::super::render;
use super::{examples, payload, replay, routed};
use aura_core_types::CompareStatus;

#[test]
fn applied_comparisons_offer_nothing_more_and_list_compactly() {
    let mut applied = payload();
    applied.status = CompareStatus::Applied("a-2".to_string());
    let text = render("Compare #1", &applied);
    assert!(text.contains("applied a-2"));
    assert!(!text.contains("/compare apply"));
    let listed = list(&[
        ("Compare #1".to_string(), Some(applied)),
        ("Compare #2".to_string(), None),
    ]);
    let first = listed.lines().nth(1).unwrap();
    assert!(first.contains("1. Compare #1"), "{first}");
    assert!(
        first.contains("gate vs aura-claude-opus-4-7 (cloud)"),
        "{first}"
    );
    assert!(first.contains("A $0.06 median, 2/3 pass"), "{first}");
    assert!(listed.contains("2. Compare #2 · no record"));
    assert!(list(&[]).starts_with("No comparisons yet"));
}

#[test]
fn the_list_is_pinned_byte_for_byte() {
    let mut running = payload();
    running.status = CompareStatus::Running;
    let mut applied = payload();
    applied.status = CompareStatus::Applied("a-2".to_string());
    let listed = list(&[
        ("Compare #1".to_string(), Some(payload())),
        ("Compare #2".to_string(), Some(routed())),
        ("Compare #3".to_string(), Some(replay())),
        ("Compare #4".to_string(), Some(examples())),
        ("Compare #5".to_string(), None),
        ("Compare #6".to_string(), Some(running)),
        ("Compare #7".to_string(), Some(applied)),
    ]);
    let at = crate::record_loader::format_local_time(0);
    let expected = [
        "Comparisons (/compare show <n> for one):".to_string(),
        format!("  1. Compare #1 · {at} · \"fix the flaky parser test\" · gate vs aura-claude-opus-4-7 (cloud) · done · A $0.06 median, 2/3 pass / B $0.45 median, 3/3 pass"),
        format!("  2. Compare #2 · {at} · \"fix the flaky parser test\" · gate vs aura-claude-opus-4-7 (cloud) · stopped · A $0.06 median, 2/3 pass / B $0.43 median, 2/2 pass"),
        format!("  3. Compare #3 · {at} · \"fix the flaky parser test\" · gate vs aura-claude-opus-4-7 (cloud) · done · A $0.06 median, 2/3 pass / B $0.45 median, 3/3 pass"),
        format!("  4. Compare #4 · {at} · \"2 example coding tasks\" · gate vs aura-claude-opus-4-7 (cloud) · done · A $0.06 median, 2/3 pass / B $0.45 median, 3/3 pass"),
        "  5. Compare #5 · no record".to_string(),
        format!("  6. Compare #6 · {at} · \"fix the flaky parser test\" · gate vs aura-claude-opus-4-7 (cloud) · running · A $0.06 median, 2/3 pass / B $0.45 median, 3/3 pass"),
        format!("  7. Compare #7 · {at} · \"fix the flaky parser test\" · gate vs aura-claude-opus-4-7 (cloud) · applied a-2 · A $0.06 median, 2/3 pass / B $0.45 median, 3/3 pass"),
    ];
    assert_eq!(listed, expected.join("\n"));
}
