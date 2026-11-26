//! Character-by-character walking through the file for word motions.

pub(super) fn is_keyword(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    Blank,
    /// A zero-length line: word motions stop on it.
    Empty,
    Punct,
    Word,
}

/// Steps through the file a character at a time. Each line also has a
/// position just past its end, standing for the newline.
pub(super) struct Walker<'a> {
    lines: &'a [&'a str],
    pub(super) line: usize,
    pub(super) col: usize,
    chars: Vec<char>,
}

impl<'a> Walker<'a> {
    pub(super) fn new(lines: &'a [&'a str], line: usize, col: usize) -> Self {
        let chars = lines[line].chars().collect();
        Self {
            lines,
            line,
            col,
            chars,
        }
    }

    pub(super) fn char(&self) -> Option<char> {
        self.chars.get(self.col).copied()
    }

    fn class(&self, big: bool) -> Class {
        match self.char() {
            None if self.chars.is_empty() => Class::Empty,
            None => Class::Blank,
            Some(c) if c.is_whitespace() => Class::Blank,
            Some(_) if big => Class::Word,
            Some(c) if is_keyword(c) => Class::Word,
            Some(_) => Class::Punct,
        }
    }

    fn seek(&mut self, line: usize, col: usize) {
        if line != self.line {
            self.line = line;
            self.chars = self.lines[line].chars().collect();
        }
        self.col = col;
    }

    pub(super) fn next(&mut self) -> bool {
        if self.col < self.chars.len() {
            self.col += 1;
            true
        } else if self.line + 1 < self.lines.len() {
            self.seek(self.line + 1, 0);
            true
        } else {
            false
        }
    }

    pub(super) fn prev(&mut self) -> bool {
        if self.col > 0 {
            self.col -= 1;
            true
        } else if self.line > 0 {
            let len = self.lines[self.line - 1].chars().count();
            self.seek(self.line - 1, len);
            true
        } else {
            false
        }
    }

    /// `w`/`W`.
    pub(super) fn word_start(&mut self, big: bool) {
        let start = self.class(big);
        if start == Class::Empty {
            self.next();
        } else if start != Class::Blank {
            while self.class(big) == start && self.next() {}
        }
        while self.class(big) == Class::Blank && self.next() {}
    }

    /// `e`/`E`.
    pub(super) fn word_end(&mut self, big: bool) {
        self.next();
        while matches!(self.class(big), Class::Blank | Class::Empty) && self.next() {}
        let class = self.class(big);
        loop {
            let (line, col) = (self.line, self.col);
            if !self.next() {
                break;
            }
            if self.class(big) != class {
                self.seek(line, col);
                break;
            }
        }
    }

    /// `b`/`B`.
    pub(super) fn word_back(&mut self, big: bool) {
        self.prev();
        while self.class(big) == Class::Blank && self.prev() {}
        let class = self.class(big);
        if class == Class::Empty {
            return;
        }
        loop {
            let (line, col) = (self.line, self.col);
            if !self.prev() {
                break;
            }
            if self.class(big) != class {
                self.seek(line, col);
                break;
            }
        }
    }

    /// `ge`/`gE`.
    pub(super) fn word_end_back(&mut self, big: bool) {
        let class = self.class(big);
        if matches!(class, Class::Blank | Class::Empty) {
            self.prev();
        } else {
            while self.class(big) == class && self.prev() {}
        }
        while self.class(big) == Class::Blank && self.prev() {}
    }
}
