//! Two-key commands: `g`, `z` and `Z` prefixes and `f`/`F`/`t`/`T`.

use super::{Align, Find, Outcome, Pending, Vim};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

impl Vim {
    pub(super) fn start_pending(&mut self, pending: Pending, count: Option<usize>) {
        self.pending = Some(pending);
        self.count = count;
    }

    pub(super) fn pending_key(
        &mut self,
        pending: Pending,
        count: Option<usize>,
        key: KeyEvent,
        lines: &[&str],
    ) -> Outcome {
        let KeyCode::Char(c) = key.code else {
            if pending == Pending::Z && key.code == KeyCode::Enter {
                self.align_cursor(count, Align::Top, true, lines);
            }
            return Outcome::Stay;
        };
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            return Outcome::Stay;
        }
        self.align = Align::Cursor;
        let n = count.unwrap_or(1).max(1);
        match (pending, c) {
            (Pending::G, 'g') => {
                self.set_line(count.map_or(0, |n| n - 1), lines);
                self.go_first_non_blank(lines);
            }
            (Pending::G, 'e') => self.repeat(n, lines, |w| w.word_end_back(false)),
            (Pending::G, 'E') => self.repeat(n, lines, |w| w.word_end_back(true)),
            (Pending::G, '_') => {
                self.set_line(self.line.saturating_add(n - 1), lines);
                let chars: Vec<char> = lines.get(self.line).unwrap_or(&"").chars().collect();
                let last = chars.iter().rposition(|c| !c.is_whitespace()).unwrap_or(0);
                self.set_col(last, lines);
            }
            (Pending::G, '0') => self.set_col(0, lines),
            (Pending::G, '$') => self.set_col(usize::MAX, lines),
            (Pending::Z, 't') => self.align_cursor(count, Align::Top, false, lines),
            (Pending::Z, 'z') => self.align_cursor(count, Align::Center, false, lines),
            (Pending::Z, 'b') => self.align_cursor(count, Align::Bottom, false, lines),
            (Pending::Z, '.') => self.align_cursor(count, Align::Center, true, lines),
            (Pending::Z, '-') => self.align_cursor(count, Align::Bottom, true, lines),
            (Pending::ShiftZ, 'Z' | 'Q') => return Outcome::Close,
            (Pending::Find(kind), target) => {
                let find = Find { kind, target };
                self.find = Some(find);
                self.find_char(find, n, false, lines);
            }
            _ => {}
        }
        Outcome::Stay
    }

    /// `zt`/`zz`/`zb` and friends: with a count, go to that line first.
    fn align_cursor(
        &mut self,
        count: Option<usize>,
        align: Align,
        line_start: bool,
        lines: &[&str],
    ) {
        if let Some(n) = count {
            self.set_line(n.saturating_sub(1), lines);
        }
        if line_start {
            self.go_first_non_blank(lines);
        }
        self.align = align;
    }
}
