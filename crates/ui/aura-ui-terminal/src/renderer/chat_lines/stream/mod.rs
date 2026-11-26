//! A reply laid out as it streams.
//!
//! Laying the whole reply out again for every chunk makes a frame cost
//! more the longer the reply gets. A reply is laid out a line at a time,
//! though, and little in it reaches across a blank line, and nothing out
//! of a closed code block. Fields do: the blank lines between two list
//! items that start with a label are not drawn, and where a record starts
//! depends on the fields above. So what comes before the last break that
//! no field reaches across is laid out once and kept, and only the rest,
//! the paragraph, list, table or code block still being written, is laid
//! out again.

use super::super::markdown::{may_continue_list, may_join_list};
use crate::app::CodeCopyHit;
use crate::components::{Message, MessageRole};
use ratatui::text::Line;

/// The part of a streaming reply that is laid out for good.
#[derive(Debug, Default)]
pub(super) struct Settled {
    /// The reply's text up to the break the rest starts after
    text: String,
    /// How many of the block's lines that text is drawn as
    lines: usize,
}

/// Lines, whether each is prose (see `BlockLines::prose`), and the copy
/// buttons of the code blocks among them, `line` counted in the lines.
pub(super) type Laid = (Vec<Line<'static>>, Vec<Option<u16>>, Vec<CodeCopyHit>);

/// The block's laid out lines, prose flags and copy buttons, to add to.
type LaidMut<'a> = (
    &'a mut Vec<Line<'static>>,
    &'a mut Vec<Option<u16>>,
    &'a mut Vec<CodeCopyHit>,
);

/// Whether `message` is a reply still being written, which is laid out a
/// part at a time.
pub(super) fn applies(message: &Message) -> bool {
    message.is_streaming()
        && message.role() == MessageRole::Assistant
        && message.activity_step().is_none()
        && message.shell_run().is_none()
        && message.diff_block().is_none()
}

impl Settled {
    /// Lay `content` out as `render` would in one go, calling it only for
    /// what was not settled before. `laid` is the block's as the last call
    /// left it, with whatever was put after it since.
    pub(super) fn extend(
        &mut self,
        content: &str,
        laid: LaidMut<'_>,
        render: impl Fn(&str) -> Laid,
    ) {
        if !content.starts_with(self.text.as_str()) {
            // The reply was replaced, not added to
            *self = Self::default();
        }
        let (lines, prose, codes) = laid;
        lines.truncate(self.lines);
        prose.truncate(self.lines);
        let settled = self.lines;
        codes.retain(|hit| hit.line < settled);
        let mut rest = &content[self.text.len()..];
        if let Some(found) = last_break(rest) {
            add((lines, prose, codes), render(&rest[..found.before]));
            let gap = vec![Line::default(); found.blank];
            add(
                (lines, prose, codes),
                (gap, vec![Some(0); found.blank], Vec::new()),
            );
            self.lines = lines.len();
            self.text.push_str(&rest[..found.after]);
            rest = &rest[found.after..];
        }
        add((lines, prose, codes), render(rest));
    }
}

/// Put `more` lines after `lines`, whose last may have no entry in `prose`,
/// with their copy buttons moved down to match.
pub(super) fn add((lines, prose, codes): LaidMut<'_>, (more, flags, hits): Laid) {
    prose.resize(lines.len(), None);
    prose.extend(flags);
    let start = lines.len();
    codes.extend(hits.into_iter().map(|hit| CodeCopyHit {
        line: start + hit.line,
        ..hit
    }));
    lines.extend(more);
}

/// A place a reply can be cut in two that are laid out apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Break {
    /// Where the first part ends, after its last line's newline
    before: usize,
    /// Blank lines drawn between the parts
    blank: usize,
    /// Where the second part starts, at a line of text
    after: usize,
}

/// A line the text before a break can end with.
#[derive(Debug, Clone, Copy)]
struct Ending {
    /// Where the line ends, after its newline
    at: usize,
    /// Whether the line closes a code block
    fence: bool,
    /// Whether the line may be part of a list, which a gap after it may
    /// not end before a field
    list: bool,
    /// Empty lines after it so far
    blank: usize,
}

/// The last break in `text`, if it has one. `text` starts at a line
/// outside any code block. A break goes before a whole line of text that
/// follows either the end of a code block or empty lines after other text,
/// unless the line before may be in a list that the line after may join.
///
/// The line has to be whole because until then it may yet turn out to
/// open a code block. Text has to be parted from text by an empty line
/// because a table's rows and the gap before a heading depend on the line
/// before them. Fields depend on the list items above them, across empty
/// lines. Empty lines after a code block are not drawn, so none go
/// between the parts there.
fn last_break(text: &str) -> Option<Break> {
    let mut found = None;
    let mut ending: Option<Ending> = None;
    let mut in_code = false;
    let mut at = 0;
    for whole in text.split_inclusive('\n') {
        let Some(line) = whole.strip_suffix('\n') else {
            break;
        };
        let line = line.strip_suffix('\r').unwrap_or(line);
        let start = at;
        at += whole.len();
        let fence = line.trim().starts_with("```");
        if fence {
            in_code = !in_code;
        }
        if in_code {
            ending = None;
        } else if line.is_empty() {
            if let Some(ending) = &mut ending {
                ending.blank += 1;
            }
        } else {
            let joins = may_join_list(line);
            let parted = |last: &Ending| last.fence || (last.blank > 0 && !(last.list && joins));
            if let Some(last) = ending.filter(parted) {
                found = Some(Break {
                    before: last.at,
                    blank: if last.fence { 0 } else { last.blank },
                    after: start,
                });
            }
            ending = Some(Ending {
                at,
                fence,
                list: may_continue_list(line),
                blank: 0,
            });
        }
    }
    found
}

#[cfg(test)]
mod tests;
