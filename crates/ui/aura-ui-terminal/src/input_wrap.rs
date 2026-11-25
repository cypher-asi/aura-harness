//! How the input box wraps its text into rows, shared by the renderer,
//! which draws the rows, and the key handling, which moves the cursor
//! between them.
//!
//! Offsets here are char offsets into the text, and columns are display
//! columns, so wide chars take two.

use unicode_width::UnicodeWidthChar;

/// Char indices where each row of `text` starts when wrapped at `width`;
/// the first is always 0. A newline ends its row, and the next row starts
/// just after it. A word that does not fit on the current row moves to
/// the next one whole, and is only broken mid-word when it is wider than
/// a full row. Whitespace never starts a wrapped row: spaces that do not
/// fit hang past the end of the row they follow (clipped when drawn), so
/// every wrapped row lines up on the left. Every char, whitespace and
/// newlines included, stays on some row, so a char offset maps straight
/// to a row and column.
pub(crate) fn row_starts(text: &str, width: usize) -> Vec<usize> {
    let chars: Vec<(char, usize)> = text.chars().map(|c| (c, c.width().unwrap_or(0))).collect();
    let mut starts = vec![0];
    let mut used = 0;
    let mut i = 0;
    while i < chars.len() {
        let (c, w) = chars[i];
        if c == '\n' {
            starts.push(i + 1);
            used = 0;
            i += 1;
            continue;
        }
        if c.is_whitespace() {
            used += w;
            i += 1;
            continue;
        }
        let end = i + chars[i..]
            .iter()
            .take_while(|(c, _)| !c.is_whitespace())
            .count();
        let word: usize = chars[i..end].iter().map(|(_, w)| w).sum();
        if used > 0 && used + word > width {
            starts.push(i);
            used = 0;
        }
        for (j, &(_, w)) in chars.iter().enumerate().take(end).skip(i) {
            if used + w > width && used > 0 {
                starts.push(j);
                used = 0;
            }
            used += w;
        }
        i = end;
    }
    starts
}

/// Row and column of the cursor `offset` chars into `text`, wrapped at
/// the row `starts` from [`row_starts`]. A full row puts the cursor at
/// the start of the next one when the text ends there. When a newline or
/// hanging whitespace still follows on the row, the next row's start is
/// where the cursor goes after it, so the cursor stays in the padding
/// column past the row's end to tell the two apart.
pub(crate) fn wrapped_cursor(
    text: &str,
    starts: &[usize],
    offset: usize,
    width: usize,
) -> (usize, usize) {
    let row = starts.iter().rposition(|&s| s <= offset).unwrap_or(0);
    let start = starts.get(row).copied().unwrap_or(0);
    let col: usize = text
        .chars()
        .skip(start)
        .take(offset.saturating_sub(start))
        .map(|c| c.width().unwrap_or(0))
        .sum();
    // Past a full row, the only chars still on it are a newline or
    // whitespace hanging off its end; text ending there wraps instead.
    let more_on_row = text.chars().nth(offset).is_some();
    if col >= width && !more_on_row {
        (row + 1, 0)
    } else {
        (row, col.min(width))
    }
}

/// Char offset of the cursor moved from `offset` to the drawn row above
/// (`up`) or below, in `text` wrapped at `width`. It keeps its column, or
/// goes to the end of a row too short for it. `None` when there is no row
/// that way.
pub(crate) fn row_move(text: &str, width: usize, offset: usize, up: bool) -> Option<usize> {
    let starts = row_starts(text, width);
    let (row, col) = wrapped_cursor(text, &starts, offset, width);
    let target = if up { row.checked_sub(1)? } else { row + 1 };
    let start = *starts.get(target)?;
    let chars: Vec<char> = text.chars().collect();
    // The last offset drawn on the target row: before the newline ending
    // it or the char the next row starts with, or the end of the text.
    let last = starts
        .get(target + 1)
        .map_or(chars.len(), |&next| next.saturating_sub(1));
    let mut best = start;
    let mut used = 0;
    for (i, c) in chars.iter().enumerate().take(last).skip(start) {
        used += c.width().unwrap_or(0);
        if used > col {
            break;
        }
        best = i + 1;
    }
    Some(best)
}

#[cfg(test)]
mod tests {
    use super::{row_move, row_starts, wrapped_cursor};

    #[test]
    fn cursor_follows_a_word_to_the_next_row() {
        let text = "hello world";
        let starts = row_starts(text, 8);
        assert_eq!(wrapped_cursor(text, &starts, text.len(), 8), (1, 5));
        assert_eq!(wrapped_cursor(text, &starts, 6, 8), (1, 0));
        assert_eq!(wrapped_cursor(text, &starts, 3, 8), (0, 3));
    }

    #[test]
    fn cursor_after_a_full_row_starts_the_next() {
        let text = "abcde";
        let starts = row_starts(text, 5);
        assert_eq!(wrapped_cursor(text, &starts, 5, 5), (1, 0));
    }

    #[test]
    fn cursor_after_a_newline_is_at_the_next_row_start() {
        let text = "ab\ncd";
        let starts = row_starts(text, 10);
        assert_eq!(wrapped_cursor(text, &starts, 2, 10), (0, 2));
        assert_eq!(wrapped_cursor(text, &starts, 3, 10), (1, 0));
        assert_eq!(wrapped_cursor(text, &starts, 5, 10), (1, 2));

        let trailing = "ab\n";
        let starts = row_starts(trailing, 10);
        assert_eq!(starts.len(), 2);
        assert_eq!(wrapped_cursor(trailing, &starts, 3, 10), (1, 0));
    }

    #[test]
    fn cursor_before_a_newline_ending_a_full_row_stays_on_it() {
        let text = "abcde\nf";
        let starts = row_starts(text, 5);
        assert_eq!(wrapped_cursor(text, &starts, 5, 5), (0, 5));
        assert_eq!(wrapped_cursor(text, &starts, 6, 5), (1, 0));
    }

    #[test]
    fn cursor_on_a_hanging_space_stays_at_the_row_end() {
        let text = "abcde fg";
        let starts = row_starts(text, 5);
        assert_eq!(wrapped_cursor(text, &starts, 5, 5), (0, 5));
        assert_eq!(wrapped_cursor(text, &starts, 6, 5), (1, 0));
        assert_eq!(wrapped_cursor(text, &starts, 7, 5), (1, 1));

        let trailing = "abcde ";
        let starts = row_starts(trailing, 5);
        assert_eq!(wrapped_cursor(trailing, &starts, 5, 5), (0, 5));
        assert_eq!(wrapped_cursor(trailing, &starts, 6, 5), (1, 0));
    }

    #[test]
    fn up_and_down_keep_the_column_across_wrapped_rows() {
        // Rows: "one two " / "three " / "four".
        let text = "one two three four";
        assert_eq!(row_starts(text, 8), vec![0, 8, 14]);
        assert_eq!(row_move(text, 8, 2, false), Some(10), "one|->thr|");
        assert_eq!(row_move(text, 8, 10, false), Some(16), "thr|->fo|");
        assert_eq!(row_move(text, 8, 16, true), Some(10));
        assert_eq!(row_move(text, 8, 10, true), Some(2));
        assert_eq!(row_move(text, 8, 2, true), None, "first row");
        assert_eq!(row_move(text, 8, 16, false), None, "last row");
    }

    #[test]
    fn a_short_row_takes_the_cursor_to_its_end() {
        // Rows: "abcdefg " / "hi " / "jklmnop".
        let text = "abcdefg hi jklmnop";
        assert_eq!(row_move(text, 8, 6, false), Some(10), "end of \"hi\"");
        assert_eq!(row_move(text, 8, 17, true), Some(10));
        // A row broken mid-word ends before the char the next row starts
        // with, which is drawn there.
        assert_eq!(row_move("abcdefghij", 5, 9, true), Some(4));
    }

    #[test]
    fn up_and_down_cross_newlines_and_count_display_columns() {
        assert_eq!(row_move("ab\ncdef", 10, 6, true), Some(2));
        assert_eq!(row_move("ab\ncdef", 10, 1, false), Some(4));
        assert_eq!(row_move("ab\n", 10, 1, false), Some(3), "empty last row");
        // "漢字" is four columns wide: column 2 is after its first char.
        assert_eq!(row_move("漢字\nabcd", 10, 5, true), Some(1));
    }

    #[test]
    fn up_from_past_a_full_last_row_goes_to_its_start() {
        assert_eq!(row_move("abcde", 5, 5, true), Some(0));
        assert_eq!(row_move("abcde", 5, 5, false), None);
    }
}
