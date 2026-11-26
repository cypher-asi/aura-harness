use super::ThinkingEffort;

#[test]
fn from_wire_handles_minimal_and_legacy_xhigh() {
    assert_eq!(
        ThinkingEffort::from_wire("minimal"),
        Some(ThinkingEffort::Minimal)
    );
    assert_eq!(ThinkingEffort::from_wire("MAX"), Some(ThinkingEffort::Max));
    assert_eq!(
        ThinkingEffort::from_wire("xhigh"),
        Some(ThinkingEffort::XHigh)
    );
    assert_eq!(ThinkingEffort::from_wire("nope"), None);
}

#[test]
fn reasoning_effort_wire_preserves_tier() {
    assert_eq!(ThinkingEffort::Off.reasoning_effort_wire(), None);
    assert_eq!(
        ThinkingEffort::Minimal.reasoning_effort_wire(),
        Some("minimal")
    );
    assert_eq!(
        ThinkingEffort::Medium.reasoning_effort_wire(),
        Some("medium")
    );
    assert_eq!(ThinkingEffort::Max.reasoning_effort_wire(), Some("max"));
}

#[test]
fn as_str_round_trips_and_levels_are_ordered() {
    let levels = [
        ThinkingEffort::Off,
        ThinkingEffort::Minimal,
        ThinkingEffort::Low,
        ThinkingEffort::Medium,
        ThinkingEffort::High,
        ThinkingEffort::XHigh,
        ThinkingEffort::Max,
    ];
    for level in levels {
        assert_eq!(ThinkingEffort::from_wire(level.as_str()), Some(level));
    }
    assert!(levels.windows(2).all(|pair| pair[0] < pair[1]));
}
