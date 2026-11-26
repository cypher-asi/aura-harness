use super::*;

// ============================================================================
// Wrapping
// ============================================================================

#[test]
fn wrap_spans_splits_words_longer_than_a_line() {
    let spans = vec![Span::styled("abcdefghij", base())];
    let lines = wrap_spans(&spans, 4, (&[], &[]), Measure::GRID);
    assert_eq!(texts(&lines), ["abcd", "efgh", "ij"]);
}

#[test]
fn wrap_spans_applies_prefixes() {
    let spans = vec![Span::raw("one two three")];
    let lines = wrap_spans(
        &spans,
        7,
        (&[Span::raw("> ")], &[Span::raw("  ")]),
        Measure::GRID,
    );
    assert_eq!(texts(&lines), ["> one", "  two", "  three"]);
}
