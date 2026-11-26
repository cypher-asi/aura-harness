//! [`HighlightCache`]: lazily highlighted lines of one file, resumable
//! from saved parser states.

use super::{
    is_plain, token_spans, LineHighlighter, LineState, Token, CHECKPOINT_INTERVAL, WINDOW_MARGIN,
};
use crate::themes::Theme;
use ratatui::text::Span;
use std::ops::Range;
use syntect::parsing::SyntaxReference;

/// Lazily highlighted lines of one file, resumable anywhere.
///
/// Holds the parser state before every [`CHECKPOINT_INTERVAL`]th line
/// reached so far, and the tokens of the last window shown. Highlighting
/// line `n` parses from the nearest checkpoint at or before `n` (filling
/// in checkpoints on the way, without computing styles for skipped
/// lines), so the result always equals highlighting from the top.
///
/// The cache is keyed by the syntax and line count it was built for and
/// resets itself when asked about anything else.
#[derive(Default)]
pub struct HighlightCache {
    inner: Option<CacheState>,
}

struct CacheState {
    syntax: &'static SyntaxReference,
    line_count: usize,
    /// `checkpoints[k]` is the state before line `k * CHECKPOINT_INTERVAL`.
    checkpoints: Vec<LineState>,
    /// First line of `window`.
    window_start: usize,
    window: Vec<Vec<Token>>,
}

impl HighlightCache {
    /// Spans of `lines[range]`, highlighted as `syntax` in the context of
    /// every line above them. `prepare` turns a raw line into the text to
    /// highlight (e.g. tab expansion); the spans concatenate to it.
    pub fn spans(
        &mut self,
        syntax: &'static SyntaxReference,
        lines: &[&str],
        range: Range<usize>,
        prepare: impl Fn(&str) -> String,
        theme: &Theme,
    ) -> Vec<Vec<Span<'static>>> {
        let end = range.end.min(lines.len());
        let start = range.start.min(end);
        if is_plain(syntax) {
            return lines
                .iter()
                .take(end)
                .skip(start)
                .map(|line| {
                    let text = prepare(line);
                    LineHighlighter { state: None }.spans(&text, theme)
                })
                .collect();
        }
        let state = self.state_for(syntax, lines.len());
        state.fill(lines, start..end, &prepare);
        state
            .window
            .iter()
            .skip(start.saturating_sub(state.window_start))
            .take(end - start)
            .map(|tokens| token_spans(tokens, theme))
            .collect()
    }

    fn state_for(
        &mut self,
        syntax: &'static SyntaxReference,
        line_count: usize,
    ) -> &mut CacheState {
        let stale = self
            .inner
            .as_ref()
            .is_none_or(|state| state.syntax.name != syntax.name || state.line_count != line_count);
        if stale {
            self.inner = None;
        }
        self.inner.get_or_insert_with(|| CacheState {
            syntax,
            line_count,
            checkpoints: vec![LineState::new(syntax)],
            window_start: 0,
            window: Vec::new(),
        })
    }
}

impl CacheState {
    /// Make `window` cover `range`.
    fn fill(&mut self, lines: &[&str], range: Range<usize>, prepare: &impl Fn(&str) -> String) {
        let window_end = self.window_start + self.window.len();
        if range.start >= self.window_start && range.end <= window_end {
            return;
        }
        let start = range.start.saturating_sub(WINDOW_MARGIN);
        let end = (range.end + WINDOW_MARGIN).min(lines.len());
        let mut state = self.resume_at(lines, start, prepare);
        self.window = lines
            .iter()
            .enumerate()
            .take(end)
            .skip(start)
            .map(|(index, line)| {
                if index % CHECKPOINT_INTERVAL == 0 {
                    self.save_checkpoint(index, &state);
                }
                state.advance(&prepare(line), true)
            })
            .collect();
        self.window_start = start;
    }

    /// The state before line `target`, parsing forward from the nearest
    /// checkpoint and saving new checkpoints on the way.
    fn resume_at(
        &mut self,
        lines: &[&str],
        target: usize,
        prepare: &impl Fn(&str) -> String,
    ) -> LineState {
        let wanted = target / CHECKPOINT_INTERVAL;
        let known = wanted.min(self.checkpoints.len().saturating_sub(1));
        let Some(checkpoint) = self.checkpoints.get(known) else {
            return LineState::new(self.syntax);
        };
        let mut state = checkpoint.clone();
        let skipped = lines
            .iter()
            .enumerate()
            .take(target)
            .skip(known * CHECKPOINT_INTERVAL);
        for (index, line) in skipped {
            if index % CHECKPOINT_INTERVAL == 0 {
                self.save_checkpoint(index, &state);
            }
            state.advance(&prepare(line), false);
        }
        state
    }

    /// Record `state` as the state before line `index` (a multiple of the
    /// interval) if it extends the known checkpoints.
    fn save_checkpoint(&mut self, index: usize, state: &LineState) {
        if index / CHECKPOINT_INTERVAL == self.checkpoints.len() {
            self.checkpoints.push(state.clone());
        }
    }
}

/// Clones start empty: the cache is derived data, rebuilt on demand.
impl Clone for HighlightCache {
    fn clone(&self) -> Self {
        Self::default()
    }
}

/// Caches never make two views differ.
impl PartialEq for HighlightCache {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for HighlightCache {}

impl std::fmt::Debug for HighlightCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.inner {
            Some(state) => f
                .debug_struct("HighlightCache")
                .field("syntax", &state.syntax.name)
                .field("checkpoints", &state.checkpoints.len())
                .field(
                    "window",
                    &(state.window_start..state.window_start + state.window.len()),
                )
                .finish(),
            None => f.write_str("HighlightCache(empty)"),
        }
    }
}
