//! How much of a read or a search the lead sees: the first
//! [`READ_LINES`] lines of a file or [`SEARCH_HITS`] hits of a search,
//! with a note saying how much was left out.

/// Lines of one `read_file` result the lead sees.
pub(crate) const READ_LINES: usize = 200;

/// Hits of one search the lead sees.
pub(crate) const SEARCH_HITS: usize = 50;

/// Tools whose results are file contents, capped at [`READ_LINES`].
const READ_TOOLS: &[&str] = &["read_file"];

/// Tools whose results are one hit per line, capped at [`SEARCH_HITS`].
const SEARCH_TOOLS: &[&str] = &["search_code", "find_files"];

/// How much of a tool's result the lead sees.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Cap {
    /// The first [`READ_LINES`] lines of a file
    Read,
    /// The first [`SEARCH_HITS`] hits of a search
    Search,
}

impl Cap {
    /// The cap on what `tool` shows the lead; `None` when it shows all.
    #[must_use]
    pub(crate) fn of(tool: &str) -> Option<Self> {
        if READ_TOOLS.contains(&tool) {
            Some(Self::Read)
        } else if SEARCH_TOOLS.contains(&tool) {
            Some(Self::Search)
        } else {
            None
        }
    }

    /// `content` cut to the lines this cap keeps, with a note naming what
    /// was left out; `None` when it fits.
    #[must_use]
    pub(crate) fn apply(self, content: &str) -> Option<String> {
        let (limit, what, advice) = match self {
            Self::Read => (
                READ_LINES,
                "lines",
                "Read a narrower start_line/end_line range, or delegate heavy reading to an \
                 explorer subtask.",
            ),
            Self::Search => (
                SEARCH_HITS,
                "hits",
                "Narrow the search, or delegate the search to an explorer subtask.",
            ),
        };
        let total = content.lines().count();
        if total <= limit {
            return None;
        }
        let kept: Vec<&str> = content.lines().take(limit).collect();
        Some(format!(
            "{}\n… [{what} capped at {limit} for the lead; {} more left out. {advice}]",
            kept.join("\n"),
            total - limit,
        ))
    }
}
