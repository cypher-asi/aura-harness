//! Vim normal-mode navigation for a read-only file view: a cursor moved
//! by vim motions (with counts), searches, and a `:` command line.
//!
//! Columns are character indices into the raw line, and the view's scroll
//! is in source lines.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

mod motion;
mod normal;
mod pending;
mod prompt;
mod search;
mod walker;

use normal::Normal;

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
        if !ctrl && self.push_count_digit(key.code) {
            return Outcome::Stay;
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
        let ctx = Normal {
            lines,
            count,
            n: count.unwrap_or(1).max(1),
            screen,
        };
        match (key.code, ctrl) {
            (KeyCode::Char('q'), false) => return Outcome::Close,
            (KeyCode::PageDown, _) => self.page_down(&ctx, scroll),
            (KeyCode::PageUp, _) => self.page_up(&ctx, scroll),
            (code, true) => self.ctrl_key(code, &ctx, scroll),
            (code, false) => self.plain_key(code, &ctx),
        }
        Outcome::Stay
    }

    /// A digit adds to the count, except a `0` that starts one (the `0`
    /// motion). Returns whether the digit was taken.
    fn push_count_digit(&mut self, code: KeyCode) -> bool {
        let KeyCode::Char(c @ '0'..='9') = code else {
            return false;
        };
        if c == '0' && self.count.is_none() {
            return false;
        }
        let digit = c.to_digit(10).map_or(0, |d| d as usize);
        let count = self.count.unwrap_or(0).saturating_mul(10);
        self.count = Some(count.saturating_add(digit).min(999_999));
        true
    }
}

#[cfg(test)]
mod tests;
