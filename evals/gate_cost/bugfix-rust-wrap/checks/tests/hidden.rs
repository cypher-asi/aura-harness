use wrap::wrap;

#[test]
fn empty_input_has_no_lines() {
    assert!(wrap("", 5).is_empty());
    assert!(wrap("   ", 5).is_empty());
}

#[test]
fn collapses_whitespace_and_fills() {
    assert_eq!(wrap("a   b\tc d", 3), vec!["a b", "c d"]);
    assert_eq!(wrap("one two three", 13), vec!["one two three"]);
}
