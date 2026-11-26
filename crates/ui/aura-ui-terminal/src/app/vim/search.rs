//! Searching: `/`, `?`, `n`, `N`, `*`, `#` and the matches to highlight.

use super::walker::is_keyword;
use super::{Prompt, Search, Vim};

impl Vim {
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

    /// `n` (or `N` when `reverse`) for the last search.
    pub(super) fn search_again(&mut self, n: usize, reverse: bool, lines: &[&str]) {
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
    pub(super) fn search_word(&mut self, forward: bool, n: usize, lines: &[&str]) {
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
