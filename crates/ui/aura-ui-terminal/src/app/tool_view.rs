//! Opening a tool call from the chat in the file view: the file a
//! `read_file` call read, or the full output of a call too long to read
//! inline (searches, commands).

use super::file_view::FileView;
use super::format::{format_tool_label, parse_tool_args};
use super::{App, DetailView};
use crate::components::{Activity, ToolStatus, ToolStep};
use std::path::{Path, PathBuf};

/// Output longer than this many lines opens in the file view instead of
/// expanding in the chat.
const LONG_RESULT_LINES: usize = 8;

impl App {
    /// Show the tool call of chat message `id` in the file view when it
    /// read a file or printed long output. Returns whether it opened.
    pub(super) fn open_tool_step(&mut self, id: u64) -> bool {
        let Some(Activity::Tool(step)) = self
            .messages
            .iter()
            .find(|m| m.version().0 == id)
            .and_then(|m| m.activity_step())
        else {
            return false;
        };
        let Some(view) = tool_view(step, self.sandbox.root()) else {
            return false;
        };
        self.open_detail(DetailView::File(Box::new(view)));
        true
    }
}

/// The view a finished tool call opens, `None` when it is better read
/// inline (still running, a diff, or short output).
pub(super) fn tool_view(step: &ToolStep, root: Option<&Path>) -> Option<FileView> {
    if step.status == ToolStatus::Running || step.diff.is_some() || step.name == "edit_file" {
        return None;
    }
    let result = crate::ansi::strip_ansi(step.result.as_deref()?);
    let result = result.trim_end();
    if result.is_empty() {
        return None;
    }
    let args = parse_tool_args(&step.args);
    let title = format_tool_label(&step.name, &step.args);
    let title = title.lines().next().unwrap_or(&step.name).to_string();

    if step.name == "read_file" && step.status == ToolStatus::Success {
        let path = args.get("path").and_then(serde_json::Value::as_str)?;
        let start = args
            .get("start_line")
            .and_then(serde_json::Value::as_u64)
            .and_then(|n| usize::try_from(n).ok())
            .unwrap_or(1);
        if let Some(full) = resolve(path, root).filter(|p| p.is_file()) {
            let mut view = FileView::load(&full, root);
            view.jump_to(start.saturating_sub(1));
            return Some(view);
        }
        let (text, first) = strip_line_numbers(result).unwrap_or((result.to_string(), start));
        let language = Path::new(path)
            .extension()
            .map(|ext| ext.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        return Some(FileView::snapshot(path, &text, &language, first));
    }

    if result.lines().count() <= LONG_RESULT_LINES {
        return None;
    }
    let language = match step.name.as_str() {
        "search_code" | "find_files" | "list_files" => "grep",
        _ if serde_json::from_str::<serde_json::Value>(result).is_ok() => "json",
        _ => "log",
    };
    Some(FileView::snapshot(&title, result, language, 1))
}

/// `path` as given to a tool, made absolute against the sandbox root.
fn resolve(path: &str, root: Option<&Path>) -> Option<PathBuf> {
    let path = Path::new(path);
    if path.is_absolute() {
        return Some(path.to_path_buf());
    }
    root.map(|root| root.join(path))
}

/// `read_file` output with its `   740|` prefixes removed, and the number
/// of its first line. `None` unless every line has one.
fn strip_line_numbers(text: &str) -> Option<(String, usize)> {
    let mut first = None;
    let mut out = Vec::new();
    for line in text.lines() {
        let (number, rest) = line.split_once('|')?;
        let number: usize = number.trim().parse().ok()?;
        first.get_or_insert(number);
        out.push(rest);
    }
    Some((out.join("\n"), first?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::file_view::FileBody;

    fn step(name: &str, args: &str, result: &str) -> ToolStep {
        let Activity::Tool(mut step) = Activity::tool("t", name, args) else {
            unreachable!()
        };
        step.status = ToolStatus::Success;
        step.result = Some(result.to_string());
        step
    }

    #[test]
    fn read_file_opens_the_file_at_its_start_line() {
        let dir = tempfile::tempdir().unwrap();
        let body: String = (1..=50).map(|n| format!("line {n}\n")).collect();
        std::fs::write(dir.path().join("a.rs"), body).unwrap();
        let step = step(
            "read_file",
            r#"{"path":"a.rs","start_line":20,"end_line":30}"#,
            "    20|line 20",
        );
        let view = tool_view(&step, Some(dir.path())).unwrap();
        assert_eq!(view.title, "a.rs");
        assert_eq!(view.vim.line(), 19);
        assert_eq!(view.first_line, 1);
    }

    #[test]
    fn missing_file_shows_what_was_read() {
        let step = step(
            "read_file",
            r#"{"path":"gone.rs","start_line":7}"#,
            "     7|fn a() {}\n     8|fn b() {}",
        );
        let view = tool_view(&step, None).unwrap();
        assert_eq!(view.first_line, 7);
        assert_eq!(view.extension(), "rs");
        assert_eq!(
            view.body,
            FileBody::Text("fn a() {}\nfn b() {}".to_string())
        );
    }

    #[test]
    fn long_search_opens_and_short_one_expands() {
        let long: String = (1..=20).map(|n| format!("src/a.rs:{n}:x\n")).collect();
        let args = r#"{"pattern":"x","path":"src"}"#;
        let view = tool_view(&step("search_code", args, &long), None).unwrap();
        assert_eq!(view.extension(), "grep");
        assert_eq!(view.title, "Grepped x in src");
        assert!(tool_view(&step("search_code", args, "src/a.rs:1:x"), None).is_none());
    }

    #[test]
    fn running_and_edits_stay_inline() {
        let long = "a\n".repeat(30);
        let mut running = step("run_command", r#"{"program":"ls"}"#, &long);
        running.status = ToolStatus::Running;
        assert!(tool_view(&running, None).is_none());
        let edit = step("edit_file", r#"{"path":"a"}"#, &long);
        assert!(tool_view(&edit, None).is_none());
        let ran = step("run_command", r#"{"program":"ls"}"#, &long);
        assert_eq!(tool_view(&ran, None).unwrap().extension(), "log");
    }
}
