use super::context_window_for_model;

#[test]
fn context_window_reexport_resolves_via_engine() {
    // The engine owns the comprehensive table tests; this one
    // smoke test confirms the local re-export still resolves so
    // a future refactor that drops the `pub(crate) use` line
    // breaks loudly here.
    assert_eq!(context_window_for_model("claude-opus-4-7"), 1_000_000);
    assert_eq!(context_window_for_model("unknown-model-xyz"), 200_000);
}
