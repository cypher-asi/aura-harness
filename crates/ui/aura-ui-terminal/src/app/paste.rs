//! Text pasted into the input: inserted whole, however big, up to
//! [`MAX_INPUT_BYTES`]; past that refused whole, and said so. A paste is
//! never cut short.

use super::{App, NotificationType};

/// Most bytes the input holds. Far past any model's context window, so
/// it only stops a paste no one meant (a log file, a binary as text)
/// from swamping the box.
pub(crate) const MAX_INPUT_BYTES: usize = 4 * 1024 * 1024;

/// Pastes longer than this many chars are confirmed with a notice: the
/// box shows a few rows, so it says the whole paste arrived.
const NOTED_CHARS: usize = 2_000;

impl App {
    /// Insert pasted `text` at the cursor in one step, or refuse it whole
    /// when the input would grow past [`MAX_INPUT_BYTES`]. Returns
    /// whether it went in.
    pub(super) fn insert_pasted(&mut self, text: &str) -> bool {
        let after = self.input.text.len().saturating_add(text.len());
        if after > MAX_INPUT_BYTES {
            self.notifications.current = Some((
                format!(
                    "That paste is {}; the input holds at most {}. Nothing was pasted.",
                    megabytes(text.len()),
                    megabytes(MAX_INPUT_BYTES)
                ),
                NotificationType::Warning,
            ));
            return false;
        }
        let pos = self.input.cursor_pos.min(self.input.text.len());
        self.input.text.insert_str(pos, text);
        self.input.cursor_pos = pos + text.len();
        self.input.reset_slash_menu();
        let chars = text.chars().count();
        if chars > NOTED_CHARS {
            let lines = text.lines().count();
            self.notifications.current = Some((
                format!(
                    "Pasted {} characters ({} line{})",
                    thousands(chars),
                    thousands(lines),
                    if lines == 1 { "" } else { "s" }
                ),
                NotificationType::Success,
            ));
        }
        true
    }
}

/// `bytes` as megabytes, rounded to one decimal: `5.2 MB`.
fn megabytes(bytes: usize) -> String {
    const MB: usize = 1024 * 1024;
    let tenths = bytes.saturating_mul(10).saturating_add(MB / 2) / MB;
    format!("{}.{} MB", tenths / 10, tenths % 10)
}

/// `n` with thousands separators: `51,234`.
fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_as_people_write_them() {
        assert_eq!(megabytes(MAX_INPUT_BYTES), "4.0 MB");
        assert_eq!(megabytes(5_452_595), "5.2 MB");
        assert_eq!(thousands(51_234), "51,234");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1_000_000), "1,000,000");
    }
}
