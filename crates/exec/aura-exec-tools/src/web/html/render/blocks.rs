//! Block elements: headings, paragraphs, lists, code, quotes and tables.

use super::super::text::tidy;
use super::super::tokenize::attr;
use super::{ListLevel, Renderer};

impl Renderer<'_> {
    pub(super) fn open_heading(&mut self, name: &str) {
        self.block_break();
        let level = usize::from(name.as_bytes()[1] - b'0');
        self.out.push_str(&"#".repeat(level));
        self.out.push(' ');
    }

    pub(super) fn open_paragraph(&mut self) {
        if self.lists.is_empty() {
            self.block_break();
        } else {
            self.line_break();
        }
    }

    /// Close a heading, paragraph or table.
    pub(super) fn close_block(&mut self, name: &str) {
        if name == "table" {
            self.table = None;
        }
        if name == "p" && !self.lists.is_empty() {
            self.line_break();
        } else {
            self.block_break();
        }
    }

    pub(super) fn open_line_break(&mut self) {
        // A line break inside a link or emphasis would close it early.
        if self.inline.is_empty() {
            self.line_break();
        } else {
            self.out.push(' ');
        }
    }

    pub(super) fn horizontal_rule(&mut self) {
        self.block_break();
        self.out.push_str("---");
        self.block_break();
    }

    pub(super) fn open_list(&mut self, name: &str, attrs: &[(String, String)]) {
        self.line_break();
        if self.lists.is_empty() {
            self.block_break();
        }
        let start = attr(attrs, "start")
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(1);
        self.lists.push(ListLevel {
            ordered: name == "ol",
            next: start,
        });
    }

    pub(super) fn close_list(&mut self) {
        self.lists.pop();
        self.line_break();
        if self.lists.is_empty() {
            self.block_break();
        }
    }

    pub(super) fn open_list_item(&mut self) {
        self.line_break();
        let depth = self.lists.len().saturating_sub(1);
        self.out.push_str(&"  ".repeat(depth));
        match self.lists.last_mut() {
            Some(level) if level.ordered => {
                self.out.push_str(&format!("{}. ", level.next));
                level.next += 1;
            }
            _ => self.out.push_str("- "),
        }
    }

    /// Replace the text since `<pre>` with a fenced code block.
    pub(super) fn close_pre(&mut self) {
        let Some((start, lang)) = self.pre.take() else {
            return;
        };
        let lang = lang.unwrap_or_default();
        let body = self.out.split_off(start);
        let body = body.strip_prefix('\n').unwrap_or(&body).trim_end();
        let fence = if body.contains("```") { "````" } else { "```" };
        self.out
            .push_str(&format!("{fence}{lang}\n{body}\n{fence}"));
        self.block_break();
    }

    /// Prefix the text since `<blockquote>` with `>` markers.
    pub(super) fn close_blockquote(&mut self) {
        let Some(start) = self.quotes.pop() else {
            return;
        };
        let body = self.out.split_off(start.min(self.out.len()));
        let quoted: Vec<String> = tidy(&body)
            .lines()
            .map(|line| {
                if line.is_empty() {
                    ">".to_string()
                } else {
                    format!("> {line}")
                }
            })
            .collect();
        self.out.push_str(&quoted.join("\n"));
        self.block_break();
    }

    pub(super) fn open_row(&mut self) {
        if let Some(table) = &mut self.table {
            table.row_cells = 0;
            self.out.push('\n');
            self.out.push('|');
        }
    }

    /// End a row; the first one is the header and gets the separator row.
    pub(super) fn close_row(&mut self) {
        if let Some(table) = &mut self.table {
            table.rows += 1;
            if table.rows == 1 && table.row_cells > 0 {
                self.out.push('\n');
                self.out.push('|');
                self.out.push_str(&" --- |".repeat(table.row_cells));
            }
        }
    }

    pub(super) fn open_cell(&mut self) {
        if let Some(table) = &mut self.table {
            table.row_cells += 1;
            self.out.push(' ');
            self.in_cell = true;
        }
    }

    pub(super) fn close_cell(&mut self) {
        self.in_cell = false;
        if self.table.is_some() {
            while self.out.ends_with(' ') {
                self.out.pop();
            }
            self.out.push_str(" |");
        }
    }
}
