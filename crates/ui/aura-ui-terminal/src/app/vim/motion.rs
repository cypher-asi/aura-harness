//! Cursor motions: lines and columns, words, paragraphs, brackets and
//! `f`/`F`/`t`/`T` on the cursor line.

use super::walker::Walker;
use super::{Find, FindKind, Vim};

impl Vim {
    pub(super) fn set_line(&mut self, line: usize, lines: &[&str]) {
        self.line = line.min(lines.len().saturating_sub(1));
        self.col = self.want_col.min(last_col(lines, self.line));
    }

    pub(super) fn set_col(&mut self, col: usize, lines: &[&str]) {
        self.col = col.min(last_col(lines, self.line));
        self.want_col = if col == usize::MAX { col } else { self.col };
    }

    pub(super) fn go_first_non_blank(&mut self, lines: &[&str]) {
        let line = lines.get(self.line).copied().unwrap_or("");
        let col = line
            .chars()
            .position(|c| !c.is_whitespace())
            .unwrap_or(usize::MAX);
        self.set_col(col.min(last_col(lines, self.line)), lines);
    }

    /// Go to `line` and its first non-blank character.
    pub(super) fn go_line_start(&mut self, line: usize, lines: &[&str]) {
        self.set_line(line, lines);
        self.go_first_non_blank(lines);
    }

    /// Run a word motion `n` times from the cursor.
    pub(super) fn repeat(
        &mut self,
        n: usize,
        lines: &[&str],
        mut motion: impl FnMut(&mut Walker<'_>),
    ) {
        if lines.is_empty() {
            return;
        }
        let mut walker = Walker::new(lines, self.line, self.col);
        for _ in 0..n {
            motion(&mut walker);
        }
        self.line = walker.line;
        self.set_col(walker.col, lines);
    }

    pub(super) fn paragraph_forward(&mut self, lines: &[&str]) {
        let mut i = self.line;
        while i < lines.len() && lines[i].is_empty() {
            i += 1;
        }
        while i < lines.len() && !lines[i].is_empty() {
            i += 1;
        }
        if i >= lines.len() {
            self.set_line(usize::MAX, lines);
            self.set_col(usize::MAX, lines);
        } else {
            self.set_line(i, lines);
            self.set_col(0, lines);
        }
    }

    pub(super) fn paragraph_back(&mut self, lines: &[&str]) {
        let mut i = self.line;
        while i > 0 && lines[i].is_empty() {
            i -= 1;
        }
        while i > 0 && !lines[i].is_empty() {
            i -= 1;
        }
        self.set_line(i, lines);
        self.set_col(0, lines);
    }

    /// `%`: jump from the first bracket at or after the cursor on this
    /// line to its partner.
    pub(super) fn match_bracket(&mut self, lines: &[&str]) {
        const PAIRS: [(char, char); 3] = [('(', ')'), ('[', ']'), ('{', '}')];
        let Some(line) = lines.get(self.line) else {
            return;
        };
        let Some((col, c)) = line
            .chars()
            .enumerate()
            .skip(self.col)
            .find(|(_, c)| PAIRS.iter().any(|&(open, close)| *c == open || *c == close))
        else {
            return;
        };
        let (open, close, forward) = PAIRS
            .iter()
            .find_map(|&(open, close)| {
                (c == open || c == close).then_some((open, close, c == open))
            })
            .unwrap_or(('(', ')', true));
        let mut walker = Walker::new(lines, self.line, col);
        let mut depth = 0usize;
        loop {
            let here = walker.char();
            if here == Some(open) {
                depth = if forward {
                    depth + 1
                } else {
                    depth.saturating_sub(1)
                };
            } else if here == Some(close) {
                depth = if forward {
                    depth.saturating_sub(1)
                } else {
                    depth + 1
                };
            }
            if depth == 0 {
                self.line = walker.line;
                self.set_col(walker.col, lines);
                return;
            }
            let moved = if forward {
                walker.next()
            } else {
                walker.prev()
            };
            if !moved {
                return;
            }
        }
    }

    /// `f`/`F`/`t`/`T` on the cursor line. A repeated `t`/`T` skips the
    /// character right next to the cursor so it can move on.
    pub(super) fn find_char(&mut self, find: Find, n: usize, repeat: bool, lines: &[&str]) {
        let chars: Vec<char> = lines.get(self.line).unwrap_or(&"").chars().collect();
        let till = matches!(find.kind, FindKind::TillForward | FindKind::TillBackward);
        let skip = usize::from(repeat && till);
        let is_target = |i: &usize| chars[*i] == find.target;
        let hit = match find.kind {
            FindKind::Forward | FindKind::TillForward => (self.col + 1 + skip..chars.len())
                .filter(is_target)
                .nth(n - 1),
            FindKind::Backward | FindKind::TillBackward => (0..self.col.saturating_sub(skip))
                .rev()
                .filter(is_target)
                .nth(n - 1),
        };
        let Some(hit) = hit else {
            return;
        };
        let col = match find.kind {
            FindKind::Forward | FindKind::Backward => hit,
            FindKind::TillForward => hit - 1,
            FindKind::TillBackward => hit + 1,
        };
        self.set_col(col, lines);
    }
}

fn last_col(lines: &[&str], line: usize) -> usize {
    lines
        .get(line)
        .map_or(0, |l| l.chars().count().saturating_sub(1))
}
