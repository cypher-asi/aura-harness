//! Normal-mode keys: scrolling, motions, and the keys that start a find,
//! a search, a `:` command or a two-key command.

use super::{Align, Find, FindKind, Pending, Prompt, Screen, Vim};
use crossterm::event::KeyCode;

/// What a normal-mode key acts on besides the key itself.
pub(super) struct Normal<'a> {
    pub(super) lines: &'a [&'a str],
    /// Count typed before the key, if any.
    pub(super) count: Option<usize>,
    /// The count, at least 1.
    pub(super) n: usize,
    /// What the last frame showed.
    pub(super) screen: Screen,
}

impl Normal<'_> {
    fn last(&self) -> usize {
        self.lines.len().saturating_sub(1)
    }

    /// Lines `^D`/`^U` move: the count, or half the screen.
    fn half(&self) -> usize {
        self.count.unwrap_or_else(|| (self.screen.shown / 2).max(1))
    }

    /// Lines a page moves: the screen less two lines of context.
    fn page(&self) -> usize {
        self.screen.shown.saturating_sub(2).max(1)
    }

    /// Line `H` goes to.
    fn screen_high(&self) -> usize {
        self.screen.first + (self.n - 1).min(self.screen.shown.saturating_sub(1))
    }

    /// Line `M` goes to.
    fn screen_middle(&self) -> usize {
        let shown = self
            .screen
            .shown
            .min(self.lines.len().saturating_sub(self.screen.first));
        self.screen.first + shown.saturating_sub(1) / 2
    }

    /// Line `L` goes to.
    fn screen_low(&self) -> usize {
        let bottom = self.screen.first + self.screen.shown.saturating_sub(1);
        bottom
            .min(self.last())
            .saturating_sub(self.n - 1)
            .max(self.screen.first)
    }
}

impl Vim {
    /// Keys held with Ctrl: scrolling, `^G` and `^N`/`^P`.
    pub(super) fn ctrl_key(&mut self, code: KeyCode, ctx: &Normal<'_>, scroll: &mut usize) {
        let (lines, n) = (ctx.lines, ctx.n);
        match code {
            KeyCode::Char('d') => self.half_page_down(ctx, scroll),
            KeyCode::Char('u') => self.half_page_up(ctx, scroll),
            KeyCode::Char('f') => self.page_down(ctx, scroll),
            KeyCode::Char('b') => self.page_up(ctx, scroll),
            KeyCode::Char('e') => {
                *scroll = scroll.saturating_add(n);
                self.align = Align::View;
            }
            KeyCode::Char('y') => {
                *scroll = scroll.saturating_sub(n);
                self.align = Align::View;
            }
            KeyCode::Char('g') => self.show_position(lines),
            KeyCode::Char('j' | 'n') => self.set_line(self.line.saturating_add(n), lines),
            KeyCode::Char('p') => self.set_line(self.line.saturating_sub(n), lines),
            _ => {}
        }
    }

    fn half_page_down(&mut self, ctx: &Normal<'_>, scroll: &mut usize) {
        let half = ctx.half();
        *scroll = scroll.saturating_add(half);
        self.set_line(self.line.saturating_add(half), ctx.lines);
        self.align = Align::View;
    }

    fn half_page_up(&mut self, ctx: &Normal<'_>, scroll: &mut usize) {
        let half = ctx.half();
        *scroll = scroll.saturating_sub(half);
        self.set_line(self.line.saturating_sub(half), ctx.lines);
        self.align = Align::View;
    }

    /// `^F` / PageDown.
    pub(super) fn page_down(&mut self, ctx: &Normal<'_>, scroll: &mut usize) {
        *scroll = scroll
            .saturating_add(ctx.page().saturating_mul(ctx.n))
            .min(ctx.last());
        self.set_line(*scroll, ctx.lines);
        self.align = Align::View;
    }

    /// `^B` / PageUp.
    pub(super) fn page_up(&mut self, ctx: &Normal<'_>, scroll: &mut usize) {
        *scroll = scroll.saturating_sub(ctx.page().saturating_mul(ctx.n));
        let bottom = scroll.saturating_add(ctx.screen.shown.saturating_sub(1));
        self.set_line(bottom.min(self.line), ctx.lines);
        self.align = Align::View;
    }

    /// `^G`: where the cursor is in the file.
    fn show_position(&mut self, lines: &[&str]) {
        let percent = (self.line + 1) * 100 / lines.len().max(1);
        self.message = Some(format!(
            "line {} of {} --{percent}%-- col {}",
            self.line + 1,
            lines.len(),
            self.col + 1
        ));
    }

    /// Keys without Ctrl.
    pub(super) fn plain_key(&mut self, code: KeyCode, ctx: &Normal<'_>) {
        if self.line_motion(code, ctx) || self.word_motion(code, ctx) || self.jump(code, ctx) {
            return;
        }
        self.command_key(code, ctx);
    }

    /// `hjkl`, `+-_` and `0^$|`. Returns whether `code` was one of them.
    fn line_motion(&mut self, code: KeyCode, ctx: &Normal<'_>) -> bool {
        let (lines, n) = (ctx.lines, ctx.n);
        match code {
            KeyCode::Char('h') | KeyCode::Left | KeyCode::Backspace => {
                self.set_col(self.col.saturating_sub(n), lines);
            }
            KeyCode::Char('l' | ' ') | KeyCode::Right => {
                self.set_col(self.col.saturating_add(n), lines);
            }
            KeyCode::Char('j') | KeyCode::Down => self.set_line(self.line.saturating_add(n), lines),
            KeyCode::Char('k') | KeyCode::Up => self.set_line(self.line.saturating_sub(n), lines),
            KeyCode::Char('+') | KeyCode::Enter => {
                self.go_line_start(self.line.saturating_add(n), lines);
            }
            KeyCode::Char('-') => self.go_line_start(self.line.saturating_sub(n), lines),
            KeyCode::Char('_') => self.go_line_start(self.line.saturating_add(n - 1), lines),
            KeyCode::Char('0') | KeyCode::Home => self.set_col(0, lines),
            KeyCode::Char('^') => self.go_first_non_blank(lines),
            KeyCode::Char('$') | KeyCode::End => {
                self.set_line(self.line.saturating_add(n - 1), lines);
                self.set_col(usize::MAX, lines);
            }
            KeyCode::Char('|') => self.set_col(n - 1, lines),
            _ => return false,
        }
        true
    }

    /// `wWeEbB` and `{}`. Returns whether `code` was one of them.
    fn word_motion(&mut self, code: KeyCode, ctx: &Normal<'_>) -> bool {
        let (lines, n) = (ctx.lines, ctx.n);
        match code {
            KeyCode::Char('w') => self.repeat(n, lines, |w| w.word_start(false)),
            KeyCode::Char('W') => self.repeat(n, lines, |w| w.word_start(true)),
            KeyCode::Char('e') => self.repeat(n, lines, |w| w.word_end(false)),
            KeyCode::Char('E') => self.repeat(n, lines, |w| w.word_end(true)),
            KeyCode::Char('b') => self.repeat(n, lines, |w| w.word_back(false)),
            KeyCode::Char('B') => self.repeat(n, lines, |w| w.word_back(true)),
            KeyCode::Char('}') => {
                for _ in 0..n {
                    self.paragraph_forward(lines);
                }
            }
            KeyCode::Char('{') => {
                for _ in 0..n {
                    self.paragraph_back(lines);
                }
            }
            _ => return false,
        }
        true
    }

    /// `G`, `%` and `HML`. Returns whether `code` was one of them.
    fn jump(&mut self, code: KeyCode, ctx: &Normal<'_>) -> bool {
        let lines = ctx.lines;
        match code {
            KeyCode::Char('G') => {
                self.go_line_start(ctx.count.map_or(ctx.last(), |n| n - 1), lines);
            }
            KeyCode::Char('%') => self.percent(ctx),
            KeyCode::Char('H') => self.go_line_start(ctx.screen_high(), lines),
            KeyCode::Char('M') => self.go_line_start(ctx.screen_middle(), lines),
            KeyCode::Char('L') => self.go_line_start(ctx.screen_low(), lines),
            _ => return false,
        }
        true
    }

    /// `%`: with a count, that far into the file; without, the bracket
    /// matching the one at or after the cursor.
    fn percent(&mut self, ctx: &Normal<'_>) {
        match ctx.count {
            Some(percent) => {
                let target = (percent.min(100) * ctx.lines.len()).div_ceil(100);
                self.go_line_start(target.saturating_sub(1), ctx.lines);
            }
            None => self.match_bracket(ctx.lines),
        }
    }

    /// Keys that find a character, search, open the prompt or start a
    /// two-key command.
    fn command_key(&mut self, code: KeyCode, ctx: &Normal<'_>) {
        let (lines, n, count) = (ctx.lines, ctx.n, ctx.count);
        match code {
            KeyCode::Char('f') => self.start_pending(Pending::Find(FindKind::Forward), count),
            KeyCode::Char('F') => self.start_pending(Pending::Find(FindKind::Backward), count),
            KeyCode::Char('t') => self.start_pending(Pending::Find(FindKind::TillForward), count),
            KeyCode::Char('T') => self.start_pending(Pending::Find(FindKind::TillBackward), count),
            KeyCode::Char(';') => self.repeat_find(false, n, lines),
            KeyCode::Char(',') => self.repeat_find(true, n, lines),
            KeyCode::Char('g') => self.start_pending(Pending::G, count),
            KeyCode::Char('z') => self.start_pending(Pending::Z, count),
            KeyCode::Char('Z') => self.start_pending(Pending::ShiftZ, count),
            KeyCode::Char('/') => self.open_search(true),
            KeyCode::Char('?') => self.open_search(false),
            KeyCode::Char(':') => self.prompt = Some(Prompt::Command(String::new())),
            KeyCode::Char('n') => self.search_again(n, false, lines),
            KeyCode::Char('N') => self.search_again(n, true, lines),
            KeyCode::Char('*') => self.search_word(true, n, lines),
            KeyCode::Char('#') => self.search_word(false, n, lines),
            _ => {}
        }
    }

    /// `;` (or `,` when `reverse`): the last `f`/`F`/`t`/`T` again.
    fn repeat_find(&mut self, reverse: bool, n: usize, lines: &[&str]) {
        let Some(find) = self.find else {
            return;
        };
        let find = if reverse {
            Find {
                kind: find.kind.reversed(),
                ..find
            }
        } else {
            find
        };
        self.find_char(find, n, true, lines);
    }

    /// `/` (forward) or `?`.
    fn open_search(&mut self, forward: bool) {
        self.prompt = Some(Prompt::Search {
            forward,
            text: String::new(),
        });
    }
}
