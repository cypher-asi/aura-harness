use super::{row_starts, split_rows};
use ratatui::text::Span;

fn rows(text: &str, width: usize) -> Vec<String> {
    let starts = row_starts(text, width);
    split_rows(&[Span::raw(text)], &starts, 0..starts.len())
        .iter()
        .map(|row| row.iter().map(|s| s.content.as_ref()).collect())
        .collect()
}

#[test]
fn words_move_to_the_next_row_whole() {
    assert_eq!(rows("hello world foo", 8), vec!["hello ", "world ", "foo"]);
    assert_eq!(rows("ab cdefg", 5), vec!["ab ", "cdefg"]);
}

#[test]
fn words_wider_than_a_row_are_broken() {
    assert_eq!(rows("ab abcdefghij", 5), vec!["ab ", "abcde", "fghij"]);
}

#[test]
fn newlines_start_rows_and_are_not_drawn() {
    assert_eq!(rows("ab\ncd", 10), vec!["ab", "cd"]);
    assert_eq!(rows("ab\n", 10), vec!["ab", ""]);
    assert_eq!(rows("\n\nx", 10), vec!["", "", "x"]);
    assert_eq!(rows("abc de\nf", 4), vec!["abc ", "de", "f"]);
}

#[test]
fn wrapped_rows_never_start_with_a_space() {
    assert_eq!(rows("abcde fg", 5), vec!["abcde ", "fg"]);
    assert_eq!(rows("abcde   fg", 5), vec!["abcde   ", "fg"]);
    assert_eq!(rows("ab cd ef", 5), vec!["ab cd ", "ef"]);
}
