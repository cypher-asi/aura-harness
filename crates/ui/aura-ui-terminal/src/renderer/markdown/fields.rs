//! Fields: list items that start with a label, as in `- Path: src/lib.rs`
//! or `- **Name:** parse`.
//!
//! Written loose, with a blank line between every item, a list of fields
//! reads as one long run of rows. The blank lines between two fields are
//! not drawn, and when the first label of a run of fields comes back, a
//! blank row sets the record it starts apart from the one before.
//!
//! Every choice depends on an item and the items above it, never on those
//! below, so while a reply streams in nothing above the line still being
//! written moves. Only lists at the top of a text segment are looked at:
//! lists in quotes and lists nested in an item keep their blank lines.
//!
//! The plan also marks where each item of such a list starts, so the
//! renderer can set an item apart from one before it that took several
//! rows (see [`Item`]).

use super::{indent, is_list_item, is_rule, list_item, table};

/// Longest label, in characters.
const MAX_LABEL_CHARS: usize = 32;

/// Most words in a label.
const MAX_LABEL_WORDS: usize = 4;

/// Characters a label may hold besides letters and digits.
const LABEL_PUNCTUATION: &str = " -_/.'()+#&";

/// How fields change the way one line is drawn.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct LinePlan {
    /// A blank line between two fields, which is not drawn
    pub(super) skip: bool,
    /// A field that starts a record right after the one before, so a
    /// blank row goes above it
    pub(super) gap: bool,
    /// An item of a list at the top of the text, and how it follows the
    /// item before it
    pub(super) item: Option<Item>,
}

/// Where a list item stands among the items of its list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Item {
    /// The list's first item
    First,
    /// A later item, which a blank row sets apart from the item before
    /// it when that one took more than one row: a wall of wrapped items
    /// reads as one paragraph otherwise, while short items stay tight
    Next,
    /// A field after a field, which stays tight to it (or is parted by
    /// [`LinePlan::gap`] when it starts a record)
    Field,
}

/// The label a field item's body starts with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Label<'a> {
    /// The label as written, without its colon or emphasis: `Path`
    pub(super) text: &'a str,
    /// Whether it is bold (`**Path:**`), which inline styling draws
    pub(super) bold: bool,
    /// The body after the label, its colon and its closing emphasis
    pub(super) rest: &'a str,
}

/// The label `body`, a list item's text, starts with, if it has one: a
/// few words then a colon, or the same in bold, followed by whitespace or
/// nothing.
pub(super) fn label(body: &str) -> Option<Label<'_>> {
    plain_label(body).or_else(|| bold_label(body))
}

/// Whether `trimmed`, a line without its indent, is a field: a list item,
/// not a task, whose text starts with a label.
pub(super) fn is_field(trimmed: &str) -> bool {
    field_label(trimmed).is_some()
}

/// How each of `lines` is drawn, as far as fields go: one entry per line.
pub(super) fn plan(lines: &[&str]) -> Vec<LinePlan> {
    let mut plan = vec![LinePlan::default(); lines.len()];
    let mut i = 0;
    while i < lines.len() {
        if table::starts_at(lines, i) {
            i = table::end(lines, i);
        } else if is_list_item(lines[i].trim_start()) {
            let (items, end) = list_at(lines, i);
            plan_list(lines, &items, &mut plan);
            i = end;
        } else {
            i += 1;
        }
    }
    plan
}

/// `L:`, where `L` is everything before the body's first colon.
fn plain_label(body: &str) -> Option<Label<'_>> {
    let (text, rest) = body.split_once(':')?;
    (is_label(text) && ends_label(rest)).then_some(Label {
        text,
        bold: false,
        rest,
    })
}

/// `**L:**` or `**L**:`, or the same with `__`.
fn bold_label(body: &str) -> Option<Label<'_>> {
    ["**", "__"].into_iter().find_map(|marks| {
        let (head, tail) = body.strip_prefix(marks)?.split_once(':')?;
        let (text, rest) = match head.strip_suffix(marks) {
            Some(text) => (text, tail),
            None => (head, tail.strip_prefix(marks)?),
        };
        (is_label(text) && ends_label(rest)).then_some(Label {
            text,
            bold: true,
            rest,
        })
    })
}

/// Whether `text` reads as the name of a field rather than the start of
/// a sentence, a time or a URL: a few short words of letters, digits and
/// the punctuation names use, starting with a letter.
fn is_label(text: &str) -> bool {
    text.chars().next().is_some_and(char::is_alphabetic)
        && text
            .chars()
            .all(|c| c.is_alphanumeric() || LABEL_PUNCTUATION.contains(c))
        && !text.ends_with(char::is_whitespace)
        && text.split_whitespace().count() <= MAX_LABEL_WORDS
        && text.chars().count() <= MAX_LABEL_CHARS
}

/// Whether a label may end before `after`: `Note:foo` is not a label.
fn ends_label(after: &str) -> bool {
    after.is_empty() || after.starts_with(char::is_whitespace)
}

/// The label of the list item line `line` if it is a field item; task
/// items are not.
fn field_label(line: &str) -> Option<Label<'_>> {
    let trimmed = line.trim_start();
    if is_rule(trimmed) {
        return None;
    }
    let item = list_item(trimmed)?;
    if item.task.is_some() {
        return None;
    }
    label(item.body)
}

/// The list that starts with the item at `lines[start]`: the lines of its
/// items, and the line just past its last line that is not blank.
///
/// Blank lines and lines indented past the first item's are part of the
/// list, the latter as content of the item above them. Items are the
/// list item lines as indented as the first. The list ends at a table, at
/// a line indented less, and at a line as indented that is not an item.
fn list_at(lines: &[&str], start: usize) -> (Vec<usize>, usize) {
    let depth = indent(lines[start]);
    let mut items = vec![start];
    let mut end = start + 1;
    for (i, line) in lines.iter().enumerate().skip(start + 1) {
        if line.trim().is_empty() {
            continue;
        }
        if table::starts_at(lines, i) {
            break;
        }
        let line_indent = indent(line);
        if line_indent == depth && is_list_item(line.trim_start()) {
            items.push(i);
        } else if line_indent <= depth {
            break;
        }
        end = i + 1;
    }
    (items, end)
}

/// Plan the items of one list, at `items`, top to bottom. The blank lines
/// between two fields are skipped. A field starts a record when its label
/// is the first of its run of fields and the record before it has two
/// labels or more, so a list of notes, every one `Note:`, stays one.
fn plan_list(lines: &[&str], items: &[usize], plan: &mut [LinePlan]) {
    let mut run: Option<Run<'_>> = None;
    for (n, &item) in items.iter().enumerate() {
        plan[item].item = Some(if n == 0 { Item::First } else { Item::Next });
        let Some(label) = field_label(lines[item]) else {
            run = None;
            continue;
        };
        match &mut run {
            None => run = Some(Run::new(label.text)),
            Some(current) if current.restarts_with(label.text) => {
                // A field above in the list puts `item` past the first line.
                // The blank lines the text has before a record are kept.
                plan[item].gap = !lines[item - 1].trim().is_empty();
                plan[item].item = Some(Item::Field);
                current.record = vec![label.text];
            }
            Some(current) => {
                skip_blanks_before(lines, item, plan);
                plan[item].item = Some(Item::Field);
                current.add(label.text);
            }
        }
    }
}

/// Fields one after another in a list, as far as they have gone.
struct Run<'a> {
    /// The label of the run's first field, which starts every record
    first: &'a str,
    /// The labels of the record so far, each once
    record: Vec<&'a str>,
}

impl<'a> Run<'a> {
    fn new(first: &'a str) -> Self {
        Self {
            first,
            record: vec![first],
        }
    }

    /// Whether a field labelled `label` starts the next record.
    fn restarts_with(&self, label: &str) -> bool {
        same_key(label, self.first) && self.record.len() >= 2
    }

    fn add(&mut self, label: &'a str) {
        if !self.record.iter().any(|seen| same_key(seen, label)) {
            self.record.push(label);
        }
    }
}

/// Mark the blank lines right above `lines[item]` as not drawn.
fn skip_blanks_before(lines: &[&str], item: usize, plan: &mut [LinePlan]) {
    let mut blank = item;
    while blank > 0 && lines[blank - 1].trim().is_empty() {
        blank -= 1;
        plan[blank].skip = true;
    }
}

/// Whether two labels name the same field, whatever their case.
fn same_key(a: &str, b: &str) -> bool {
    a.chars()
        .flat_map(char::to_lowercase)
        .eq(b.chars().flat_map(char::to_lowercase))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_are_a_few_words_then_a_colon() {
        for (body, text) in [
            ("Path: `a.py`", "Path"),
            ("Output: str or None", "Output"),
            ("Input (optional): x", "Input (optional)"),
            ("C++: a language", "C++"),
            ("Q&A:", "Q&A"),
            ("snake_case: value", "snake_case"),
        ] {
            let found = label(body);
            assert_eq!(
                found.map(|l| (l.text, l.bold)),
                Some((text, false)),
                "{body:?}"
            );
        }
    }

    #[test]
    fn bold_labels_close_before_or_after_the_colon() {
        for body in [
            "**Path:** a",
            "**Path**: a",
            "__Path:__ a",
            "__Path__: a",
            "**Path:**",
        ] {
            let found = label(body);
            assert_eq!(
                found.map(|l| (l.text, l.bold)),
                Some(("Path", true)),
                "{body:?}"
            );
        }
        assert_eq!(label("**Path:** a").map(|l| l.rest), Some(" a"));
    }

    #[test]
    fn text_that_only_has_a_colon_is_not_a_label() {
        for body in [
            "https://x.dev",
            "10:30 standup",
            "Note:foo",
            "the fix was simple and quick to make: one line",
            "`x`: code",
            "Path : spaced",
            ": nothing",
            "a label that is far too long to be one: x",
            "Supercalifragilisticexpialidocious: x",
            "**Path:**a",
            "**Path:__ a",
            "just text",
            "",
        ] {
            assert_eq!(label(body), None, "{body:?}");
        }
    }

    /// Per line of `lines`: whether it is skipped, and whether a blank row
    /// goes above it.
    fn planned(lines: &[&str]) -> Vec<(bool, bool)> {
        plan(lines)
            .iter()
            .map(|line| (line.skip, line.gap))
            .collect()
    }

    #[test]
    fn a_record_starts_once_the_one_before_has_two_labels() {
        let lines = ["- File: a", "- Change: x", "- file: b", "- Change: y"];
        let gaps: Vec<bool> = planned(&lines).iter().map(|&(_, gap)| gap).collect();
        assert_eq!(gaps, [false, false, true, false]);

        let lines = ["- Note: a", "", "- Note: b", "", "- Note: c"];
        let none = (false, false);
        let skip = (true, false);
        assert_eq!(planned(&lines), [none, skip, none, skip, none]);
    }

    #[test]
    fn only_blank_lines_between_two_fields_are_skipped() {
        let lines = ["- A: 1", "", "- plain", "", "- B: 2", "", "- C: 3"];
        let skipped: Vec<bool> = planned(&lines).iter().map(|&(skip, _)| skip).collect();
        assert_eq!(skipped, [false, false, false, false, false, true, false]);
    }

    #[test]
    fn nothing_is_planned_from_the_lines_below() {
        let lines = [
            "- Path: a",
            "",
            "- Name: f",
            "",
            "- Type: x",
            "",
            "- Path: b",
            "",
            "- Name: g",
            "",
            "- plain",
            "",
            "- Path: c",
            "  more of c",
            "",
            "- Name: h",
            "",
            "text",
            "",
            "- Note: n",
            "",
            "- Note: m",
        ];
        let whole = planned(&lines);
        for end in 1..=lines.len() {
            // Blank lines at the end wait on the item after them
            let settled = lines[..end]
                .iter()
                .rposition(|line| !line.trim().is_empty())
                .map_or(0, |last| last + 1);
            let part = planned(&lines[..end]);
            assert_eq!(part[..settled], whole[..settled], "first {end} lines");
        }
    }

    #[test]
    fn lists_inside_quotes_and_items_are_not_planned() {
        let lines = [
            "> - A: 1", ">", "> - B: 2", "- x", "  - A: 1", "", "  - B: 2",
        ];
        let planned = plan(&lines);
        assert!(planned.iter().all(|line| !line.skip && !line.gap));
        let items: Vec<Option<Item>> = planned.iter().map(|line| line.item).collect();
        assert_eq!(
            items,
            [None, None, None, Some(Item::First), None, None, None],
            "only the top list's items are marked"
        );
    }

    #[test]
    fn items_are_marked_first_next_or_field() {
        let lines = ["- one", "  more", "- A: 1", "", "- B: 2", "- plain"];
        let items: Vec<Option<Item>> = plan(&lines).iter().map(|line| line.item).collect();
        assert_eq!(
            items,
            [
                Some(Item::First),
                None,
                Some(Item::Next),
                None,
                Some(Item::Field),
                Some(Item::Next)
            ]
        );
    }

    #[test]
    fn a_list_ends_at_a_table_and_at_text_as_indented_as_it() {
        let lines = ["- A: 1", "", "- B: 2", "", "text", "", "- C: 3"];
        let skipped: Vec<bool> = planned(&lines).iter().map(|&(skip, _)| skip).collect();
        assert_eq!(skipped, [false, true, false, false, false, false, false]);

        let lines = ["- A: 1", "", "- B: 2", "| a |", "|---|", "", "- C: 3"];
        let skipped: Vec<bool> = planned(&lines).iter().map(|&(skip, _)| skip).collect();
        assert_eq!(skipped, [false, true, false, false, false, false, false]);
    }

    #[test]
    fn task_items_are_not_fields() {
        assert!(is_field("- Path: a"));
        assert!(is_field("1. **Name:** b"));
        assert!(!is_field("- [ ] Path: a"));
        assert!(!is_field("- - -"));
        assert!(!is_field("Path: a"));
    }
}
