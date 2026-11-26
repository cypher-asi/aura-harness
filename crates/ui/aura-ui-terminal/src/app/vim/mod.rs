//! Vim normal-mode navigation for a read-only file view: a cursor moved
//! by vim motions (with counts), searches, and a `:` command line.
//!
//! Columns are character indices into the raw line, and the view's scroll
//! is in source lines.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// How the next frame places the view around the cursor.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Align {
    /// Scroll as little as possible to keep the cursor on screen, or
    /// center it after a long jump.
    #[default]
    Cursor,
    /// The view was scrolled; the cursor moves to stay on screen.
    View,
    /// Cursor line at the top of the view (`zt`).
    Top,
    /// Cursor line in the middle of the view (`zz`).
    Center,
    /// Cursor line at the bottom of the view (`zb`).
    Bottom,
}

/// The part of the file on screen in the last frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Screen {
    /// First source line shown.
    pub first: usize,
    /// Source lines shown, at least 1.
    pub shown: usize,
}

/// What the view should do after a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Stay open.
    Stay,
    /// Close the view (`q`, `:q`, `ZZ`, Esc).
    Close,
}

/// Text being typed on the bottom line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Prompt {
    /// `/` (forward) or `?` (backward) search.
    Search {
        /// Whether the search goes down the file.
        forward: bool,
        /// Pattern typed so far.
        text: String,
    },
    /// `:` command.
    Command(String),
}

impl Prompt {
    /// The line as shown, starting with its `/`, `?` or `:`.
    #[must_use]
    pub fn display(&self) -> String {
        match self {
            Self::Search {
                forward: true,
                text,
            } => format!("/{text}"),
            Self::Search {
                forward: false,
                text,
            } => format!("?{text}"),
            Self::Command(text) => format!(":{text}"),
        }
    }
}

/// First key of a two-key command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pending {
    G,
    Z,
    ShiftZ,
    Find(FindKind),
}

/// `f`, `F`, `t` or `T`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FindKind {
    Forward,
    Backward,
    TillForward,
    TillBackward,
}

impl FindKind {
    const fn key(self) -> char {
        match self {
            Self::Forward => 'f',
            Self::Backward => 'F',
            Self::TillForward => 't',
            Self::TillBackward => 'T',
        }
    }

    const fn reversed(self) -> Self {
        match self {
            Self::Forward => Self::Backward,
            Self::Backward => Self::Forward,
            Self::TillForward => Self::TillBackward,
            Self::TillBackward => Self::TillForward,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Find {
    kind: FindKind,
    target: char,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Search {
    pattern: String,
    forward: bool,
    /// Match only whole keywords (`*` and `#`).
    whole_word: bool,
}

/// Cursor and normal-mode state of an open file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Vim {
    line: usize,
    col: usize,
    /// Column vertical motions aim for; `usize::MAX` after `$`.
    want_col: usize,
    count: Option<usize>,
    pending: Option<Pending>,
    prompt: Option<Prompt>,
    search: Option<Search>,
    /// Whether matches of the last search are highlighted (`:noh` clears).
    highlight: bool,
    find: Option<Find>,
    align: Align,
    message: Option<String>,
}

impl Vim {
    /// Cursor line.
    #[must_use]
    pub const fn line(&self) -> usize {
        self.line
    }

    /// Cursor column, in characters.
    #[must_use]
    pub const fn col(&self) -> usize {
        self.col
    }

    /// Search or command being typed, if any.
    #[must_use]
    pub const fn prompt(&self) -> Option<&Prompt> {
        self.prompt.as_ref()
    }

    /// Message from the last command (not found, unknown command, ...).
    #[must_use]
    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }

    /// Count and first key typed so far, as vim's `showcmd` shows them.
    #[must_use]
    pub fn pending_keys(&self) -> String {
        let mut keys = self.count.map(|n| n.to_string()).unwrap_or_default();
        match self.pending {
            Some(Pending::G) => keys.push('g'),
            Some(Pending::Z) => keys.push('z'),
            Some(Pending::ShiftZ) => keys.push('Z'),
            Some(Pending::Find(kind)) => keys.push(kind.key()),
            None => {}
        }
        keys
    }

    /// Placement the next frame should use, resetting it to follow the
    /// cursor.
    pub fn take_align(&mut self) -> Align {
        std::mem::take(&mut self.align)
    }

    /// The view was scrolled from outside (the mouse wheel).
    pub fn scrolled(&mut self) {
        self.align = Align::View;
    }

    /// Put the cursor at the start of `line` (clamped to the file) and
    /// show that line at the top of the next frame.
    pub fn jump_to(&mut self, line: usize, lines: &[&str]) {
        self.want_col = 0;
        self.set_line(line, lines);
        self.align = Align::Top;
    }

    /// Move the cursor into lines `first..=last`, keeping its column goal.
    pub fn keep_within(&mut self, first: usize, last: usize, lines: &[&str]) {
        if self.line < first {
            self.set_line(first, lines);
        } else if self.line > last {
            self.set_line(last, lines);
        }
    }

    /// Character ranges in `line` to highlight as search matches.
    #[must_use]
    pub fn matches(&self, line: &str) -> Vec<(usize, usize)> {
        let (pattern, whole_word) = match (&self.prompt, &self.search) {
            (Some(Prompt::Search { text, .. }), _) => (text.as_str(), false),
            (Some(Prompt::Command(_)), _) | (None, None) => return Vec::new(),
            (None, Some(search)) if self.highlight => (search.pattern.as_str(), search.whole_word),
            (None, Some(_)) => return Vec::new(),
        };
        if pattern.is_empty() {
            return Vec::new();
        }
        let pattern: Vec<char> = pattern.chars().collect();
        let chars: Vec<char> = line.chars().collect();
        match_starts(&chars, &pattern, whole_word)
            .into_iter()
            .map(|start| (start, start + pattern.len()))
            .collect()
    }

    /// Apply `key` to the cursor and `scroll`, given what the last frame
    /// showed.
    pub fn handle_key(
        &mut self,
        key: KeyEvent,
        lines: &[&str],
        scroll: &mut usize,
        screen: Screen,
    ) -> Outcome {
        self.message = None;
        if self.prompt.is_some() {
            return self.prompt_key(key, lines);
        }
        if let Some(pending) = self.pending.take() {
            let count = self.count.take();
            return self.pending_key(pending, count, key, lines);
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if let KeyCode::Char(c @ '0'..='9') = key.code {
            if !ctrl && (c != '0' || self.count.is_some()) {
                let digit = c.to_digit(10).map_or(0, |d| d as usize);
                let count = self.count.unwrap_or(0).saturating_mul(10);
                self.count = Some(count.saturating_add(digit).min(999_999));
                return Outcome::Stay;
            }
        }
        let count = self.count.take();
        if key.code == KeyCode::Esc {
            return if count.is_some() {
                Outcome::Stay
            } else {
                Outcome::Close
            };
        }
        self.align = Align::Cursor;
        let n = count.unwrap_or(1).max(1);
        let last = lines.len().saturating_sub(1);
        let half = count.unwrap_or_else(|| (screen.shown / 2).max(1));
        let page = screen.shown.saturating_sub(2).max(1);
        match (key.code, ctrl) {
            (KeyCode::Char('q'), false) => return Outcome::Close,
            (KeyCode::Char('d'), true) => {
                *scroll = scroll.saturating_add(half);
                self.set_line(self.line.saturating_add(half), lines);
                self.align = Align::View;
            }
            (KeyCode::Char('u'), true) => {
                *scroll = scroll.saturating_sub(half);
                self.set_line(self.line.saturating_sub(half), lines);
                self.align = Align::View;
            }
            (KeyCode::Char('f'), true) | (KeyCode::PageDown, _) => {
                *scroll = scroll.saturating_add(page.saturating_mul(n)).min(last);
                self.set_line(*scroll, lines);
                self.align = Align::View;
            }
            (KeyCode::Char('b'), true) | (KeyCode::PageUp, _) => {
                *scroll = scroll.saturating_sub(page.saturating_mul(n));
                let bottom = scroll.saturating_add(screen.shown.saturating_sub(1));
                self.set_line(bottom.min(self.line), lines);
                self.align = Align::View;
            }
            (KeyCode::Char('e'), true) => {
                *scroll = scroll.saturating_add(n);
                self.align = Align::View;
            }
            (KeyCode::Char('y'), true) => {
                *scroll = scroll.saturating_sub(n);
                self.align = Align::View;
            }
            (KeyCode::Char('g'), true) => {
                let percent = (self.line + 1) * 100 / lines.len().max(1);
                self.message = Some(format!(
                    "line {} of {} --{percent}%-- col {}",
                    self.line + 1,
                    lines.len(),
                    self.col + 1
                ));
            }
            (KeyCode::Char('j' | 'n'), true) => self.set_line(self.line.saturating_add(n), lines),
            (KeyCode::Char('p'), true) => self.set_line(self.line.saturating_sub(n), lines),
            (_, true) => {}
            (KeyCode::Char('h') | KeyCode::Left | KeyCode::Backspace, _) => {
                self.set_col(self.col.saturating_sub(n), lines);
            }
            (KeyCode::Char('l' | ' ') | KeyCode::Right, _) => {
                self.set_col(self.col.saturating_add(n), lines);
            }
            (KeyCode::Char('j') | KeyCode::Down, _) => {
                self.set_line(self.line.saturating_add(n), lines);
            }
            (KeyCode::Char('k') | KeyCode::Up, _) => {
                self.set_line(self.line.saturating_sub(n), lines);
            }
            (KeyCode::Char('+') | KeyCode::Enter, _) => {
                self.set_line(self.line.saturating_add(n), lines);
                self.go_first_non_blank(lines);
            }
            (KeyCode::Char('-'), _) => {
                self.set_line(self.line.saturating_sub(n), lines);
                self.go_first_non_blank(lines);
            }
            (KeyCode::Char('_'), _) => {
                self.set_line(self.line.saturating_add(n - 1), lines);
                self.go_first_non_blank(lines);
            }
            (KeyCode::Char('0') | KeyCode::Home, _) => self.set_col(0, lines),
            (KeyCode::Char('^'), _) => self.go_first_non_blank(lines),
            (KeyCode::Char('$') | KeyCode::End, _) => {
                self.set_line(self.line.saturating_add(n - 1), lines);
                self.set_col(usize::MAX, lines);
            }
            (KeyCode::Char('|'), _) => self.set_col(n - 1, lines),
            (KeyCode::Char('w'), _) => self.repeat(n, lines, |w| w.word_start(false)),
            (KeyCode::Char('W'), _) => self.repeat(n, lines, |w| w.word_start(true)),
            (KeyCode::Char('e'), _) => self.repeat(n, lines, |w| w.word_end(false)),
            (KeyCode::Char('E'), _) => self.repeat(n, lines, |w| w.word_end(true)),
            (KeyCode::Char('b'), _) => self.repeat(n, lines, |w| w.word_back(false)),
            (KeyCode::Char('B'), _) => self.repeat(n, lines, |w| w.word_back(true)),
            (KeyCode::Char('}'), _) => {
                for _ in 0..n {
                    self.paragraph_forward(lines);
                }
            }
            (KeyCode::Char('{'), _) => {
                for _ in 0..n {
                    self.paragraph_back(lines);
                }
            }
            (KeyCode::Char('G'), _) => {
                self.set_line(count.map_or(last, |n| n - 1), lines);
                self.go_first_non_blank(lines);
            }
            (KeyCode::Char('%'), _) => match count {
                Some(percent) => {
                    let target = (percent.min(100) * lines.len()).div_ceil(100);
                    self.set_line(target.saturating_sub(1), lines);
                    self.go_first_non_blank(lines);
                }
                None => self.match_bracket(lines),
            },
            (KeyCode::Char('H'), _) => {
                let line = screen.first + (n - 1).min(screen.shown.saturating_sub(1));
                self.set_line(line, lines);
                self.go_first_non_blank(lines);
            }
            (KeyCode::Char('M'), _) => {
                let shown = screen.shown.min(lines.len().saturating_sub(screen.first));
                self.set_line(screen.first + shown.saturating_sub(1) / 2, lines);
                self.go_first_non_blank(lines);
            }
            (KeyCode::Char('L'), _) => {
                let bottom = screen.first + screen.shown.saturating_sub(1);
                let line = bottom.min(last).saturating_sub(n - 1).max(screen.first);
                self.set_line(line, lines);
                self.go_first_non_blank(lines);
            }
            (KeyCode::Char('f'), _) => self.start_pending(Pending::Find(FindKind::Forward), count),
            (KeyCode::Char('F'), _) => self.start_pending(Pending::Find(FindKind::Backward), count),
            (KeyCode::Char('t'), _) => {
                self.start_pending(Pending::Find(FindKind::TillForward), count);
            }
            (KeyCode::Char('T'), _) => {
                self.start_pending(Pending::Find(FindKind::TillBackward), count);
            }
            (KeyCode::Char(';'), _) => {
                if let Some(find) = self.find {
                    self.find_char(find, n, true, lines);
                }
            }
            (KeyCode::Char(','), _) => {
                if let Some(find) = self.find {
                    let reversed = Find {
                        kind: find.kind.reversed(),
                        ..find
                    };
                    self.find_char(reversed, n, true, lines);
                }
            }
            (KeyCode::Char('g'), _) => self.start_pending(Pending::G, count),
            (KeyCode::Char('z'), _) => self.start_pending(Pending::Z, count),
            (KeyCode::Char('Z'), _) => self.start_pending(Pending::ShiftZ, count),
            (KeyCode::Char('/'), _) => {
                self.prompt = Some(Prompt::Search {
                    forward: true,
                    text: String::new(),
                });
            }
            (KeyCode::Char('?'), _) => {
                self.prompt = Some(Prompt::Search {
                    forward: false,
                    text: String::new(),
                });
            }
            (KeyCode::Char(':'), _) => self.prompt = Some(Prompt::Command(String::new())),
            (KeyCode::Char('n'), _) => self.search_again(n, false, lines),
            (KeyCode::Char('N'), _) => self.search_again(n, true, lines),
            (KeyCode::Char('*'), _) => self.search_word(true, n, lines),
            (KeyCode::Char('#'), _) => self.search_word(false, n, lines),
            _ => {}
        }
        Outcome::Stay
    }

    fn start_pending(&mut self, pending: Pending, count: Option<usize>) {
        self.pending = Some(pending);
        self.count = count;
    }

    fn pending_key(
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

    fn prompt_key(&mut self, key: KeyEvent, lines: &[&str]) -> Outcome {
        let Some(prompt) = self.prompt.as_mut() else {
            return Outcome::Stay;
        };
        let text = match prompt {
            Prompt::Search { text, .. } | Prompt::Command(text) => text,
        };
        match key.code {
            KeyCode::Esc => self.prompt = None,
            KeyCode::Backspace if text.is_empty() => self.prompt = None,
            KeyCode::Backspace => {
                text.pop();
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => text.clear(),
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => text.push(c),
            KeyCode::Enter => {
                self.align = Align::Cursor;
                match self.prompt.take() {
                    Some(Prompt::Search { forward, text }) => {
                        if !text.is_empty() {
                            self.search = Some(Search {
                                pattern: text,
                                forward,
                                whole_word: false,
                            });
                        } else if let Some(search) = self.search.as_mut() {
                            search.forward = forward;
                        }
                        self.search_again(1, false, lines);
                    }
                    Some(Prompt::Command(command)) => return self.run_command(&command, lines),
                    None => {}
                }
            }
            _ => {}
        }
        Outcome::Stay
    }

    fn run_command(&mut self, command: &str, lines: &[&str]) -> Outcome {
        let command = command.trim();
        match command {
            "" => {}
            "q" | "q!" | "qa" | "qa!" | "quit" | "x" | "wq" | "close" => return Outcome::Close,
            "noh" | "nohl" | "nohlsearch" => self.highlight = false,
            "$" => {
                self.set_line(usize::MAX, lines);
                self.go_first_non_blank(lines);
            }
            _ => match command.parse::<usize>() {
                Ok(n) => {
                    self.set_line(n.saturating_sub(1), lines);
                    self.go_first_non_blank(lines);
                }
                Err(_) => self.message = Some(format!("Not an editor command: {command}")),
            },
        }
        Outcome::Stay
    }

    fn set_line(&mut self, line: usize, lines: &[&str]) {
        self.line = line.min(lines.len().saturating_sub(1));
        self.col = self.want_col.min(last_col(lines, self.line));
    }

    fn set_col(&mut self, col: usize, lines: &[&str]) {
        self.col = col.min(last_col(lines, self.line));
        self.want_col = if col == usize::MAX { col } else { self.col };
    }

    fn go_first_non_blank(&mut self, lines: &[&str]) {
        let line = lines.get(self.line).copied().unwrap_or("");
        let col = line
            .chars()
            .position(|c| !c.is_whitespace())
            .unwrap_or(usize::MAX);
        self.set_col(col.min(last_col(lines, self.line)), lines);
    }

    /// Run a word motion `n` times from the cursor.
    fn repeat(&mut self, n: usize, lines: &[&str], mut motion: impl FnMut(&mut Walker<'_>)) {
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

    fn paragraph_forward(&mut self, lines: &[&str]) {
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

    fn paragraph_back(&mut self, lines: &[&str]) {
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
    fn match_bracket(&mut self, lines: &[&str]) {
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
    fn find_char(&mut self, find: Find, n: usize, repeat: bool, lines: &[&str]) {
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

    /// `n` (or `N` when `reverse`) for the last search.
    fn search_again(&mut self, n: usize, reverse: bool, lines: &[&str]) {
        let Some(search) = self.search.clone() else {
            self.message = Some("No previous search".to_string());
            return;
        };
        self.highlight = true;
        let forward = search.forward != reverse;
        let pattern: Vec<char> = search.pattern.chars().collect();
        let mut at = (self.line, self.col);
        let mut wrapped = false;
        for _ in 0..n {
            match find_match(lines, at, &pattern, search.whole_word, forward) {
                Some((found, wrap)) => {
                    at = found;
                    wrapped |= wrap;
                }
                None => {
                    self.message = Some(format!("Pattern not found: {}", search.pattern));
                    return;
                }
            }
        }
        self.line = at.0;
        self.set_col(at.1, lines);
        if wrapped {
            self.message = Some(
                if forward {
                    "search hit BOTTOM, continuing at TOP"
                } else {
                    "search hit TOP, continuing at BOTTOM"
                }
                .to_string(),
            );
        }
    }

    /// `*`/`#`: search for the keyword under or after the cursor.
    fn search_word(&mut self, forward: bool, n: usize, lines: &[&str]) {
        let chars: Vec<char> = lines.get(self.line).unwrap_or(&"").chars().collect();
        let Some(start) = (self.col..chars.len()).find(|&i| is_keyword(chars[i])) else {
            self.message = Some("No string under cursor".to_string());
            return;
        };
        let begin = (0..start)
            .rev()
            .take_while(|&i| is_keyword(chars[i]))
            .last()
            .unwrap_or(start);
        let end = (start..chars.len())
            .find(|&i| !is_keyword(chars[i]))
            .unwrap_or(chars.len());
        self.search = Some(Search {
            pattern: chars[begin..end].iter().collect(),
            forward,
            whole_word: true,
        });
        // Start from the word's first character so `#` skips this occurrence.
        self.col = begin;
        self.search_again(n, false, lines);
    }
}

fn last_col(lines: &[&str], line: usize) -> usize {
    lines
        .get(line)
        .map_or(0, |l| l.chars().count().saturating_sub(1))
}

fn is_keyword(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Smart case: a pattern without capitals ignores case.
fn match_starts(chars: &[char], pattern: &[char], whole_word: bool) -> Vec<usize> {
    if pattern.is_empty() || pattern.len() > chars.len() {
        return Vec::new();
    }
    let ignore_case = !whole_word && !pattern.iter().any(|c| c.is_uppercase());
    let same = |a: char, b: char| a == b || (ignore_case && a.to_lowercase().eq(b.to_lowercase()));
    (0..=chars.len() - pattern.len())
        .filter(|&i| pattern.iter().zip(&chars[i..]).all(|(&p, &c)| same(p, c)))
        .filter(|&i| {
            !whole_word
                || ((i == 0 || !is_keyword(chars[i - 1]))
                    && chars.get(i + pattern.len()).is_none_or(|&c| !is_keyword(c)))
        })
        .collect()
}

/// Next match after (or before) `at`, wrapping around the file. Returns
/// the match and whether the search wrapped.
fn find_match(
    lines: &[&str],
    at: (usize, usize),
    pattern: &[char],
    whole_word: bool,
    forward: bool,
) -> Option<((usize, usize), bool)> {
    let total = lines.len();
    if total == 0 {
        return None;
    }
    let starts = |line: usize| {
        let chars: Vec<char> = lines[line].chars().collect();
        match_starts(&chars, pattern, whole_word)
    };
    for step in 0..=total {
        let line = if forward {
            (at.0 + step) % total
        } else {
            (at.0 + total * 2 - step) % total
        };
        let wrapped = if forward {
            at.0 + step >= total
        } else {
            step > at.0
        };
        let hits = starts(line);
        let hit = match (step, forward) {
            (0, true) => hits.into_iter().find(|&c| c > at.1),
            (0, false) => hits.into_iter().rev().find(|&c| c < at.1),
            (s, true) if s == total => hits.into_iter().find(|&c| c <= at.1),
            (s, false) if s == total => hits.into_iter().rev().find(|&c| c >= at.1),
            (_, true) => hits.into_iter().next(),
            (_, false) => hits.into_iter().next_back(),
        };
        if let Some(col) = hit {
            return Some(((line, col), wrapped));
        }
    }
    None
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
struct Walker<'a> {
    lines: &'a [&'a str],
    line: usize,
    col: usize,
    chars: Vec<char>,
}

impl<'a> Walker<'a> {
    fn new(lines: &'a [&'a str], line: usize, col: usize) -> Self {
        let chars = lines[line].chars().collect();
        Self {
            lines,
            line,
            col,
            chars,
        }
    }

    fn char(&self) -> Option<char> {
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

    fn next(&mut self) -> bool {
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

    fn prev(&mut self) -> bool {
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
    fn word_start(&mut self, big: bool) {
        let start = self.class(big);
        if start == Class::Empty {
            self.next();
        } else if start != Class::Blank {
            while self.class(big) == start && self.next() {}
        }
        while self.class(big) == Class::Blank && self.next() {}
    }

    /// `e`/`E`.
    fn word_end(&mut self, big: bool) {
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
    fn word_back(&mut self, big: bool) {
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
    fn word_end_back(&mut self, big: bool) {
        let class = self.class(big);
        if matches!(class, Class::Blank | Class::Empty) {
            self.prev();
        } else {
            while self.class(big) == class && self.prev() {}
        }
        while self.class(big) == Class::Blank && self.prev() {}
    }
}

#[cfg(test)]
mod tests;
