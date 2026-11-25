//! Opening a tool call from the chat in the file view: the file a
//! `read_file` call read, or the full output of a call too long to read
//! inline (searches, commands). A plan that `create_plan` saved or
//! `read_file` read opens as a document instead.

use super::file_view::FileView;
use super::format::{format_tool_label, parse_tool_args};
use super::plan_doc::is_plan_file;
use super::{App, DetailView};
use crate::components::{Activity, ToolStatus, ToolStep};
use std::path::{Path, PathBuf};

/// Output longer than this many lines opens in the file view instead of
/// expanding in the chat.
const LONG_RESULT_LINES: usize = 8;

/// Start of the first output line of `create_plan`, followed by the
/// workspace-relative plan path, as
/// `aura_exec_tools::plan_tools::SAVED_PREFIX` writes it (this crate does
/// not depend on the tools crate).
const PLAN_SAVED_PREFIX: &str = "Plan saved: ";

impl App {
    /// Show the tool call of chat message `id` in the file view when it
    /// read a file or printed long output, or as a plan document when it
    /// saved or read a plan. Returns whether it opened.
    pub(super) fn open_tool_step(&mut self, id: u64) -> bool {
        let Some(Activity::Tool(step)) = self
            .messages
            .iter()
            .find(|m| m.version().0 == id)
            .and_then(|m| m.activity_step())
        else {
            return false;
        };
        if let Some(plan) = plan_file(step, self.sandbox.root()) {
            self.open_plan_doc(&plan, None);
            return true;
        }
        let Some(view) = tool_view(step, self.sandbox.root()) else {
            return false;
        };
        self.open_detail(DetailView::File(Box::new(view)));
        true
    }
}

/// The existing plan file a successful call saved (`create_plan`) or read
/// (`read_file` of a `.plan.md`), whatever the length of its output.
fn plan_file(step: &ToolStep, root: Option<&Path>) -> Option<PathBuf> {
    if step.status != ToolStatus::Success {
        return None;
    }
    let path = match step.name.as_str() {
        "create_plan" => {
            let result = crate::ansi::strip_ansi(step.result.as_deref()?);
            let first = result.lines().next()?;
            first.strip_prefix(PLAN_SAVED_PREFIX)?.trim().to_string()
        }
        "read_file" => parse_tool_args(&step.args)
            .get("path")
            .and_then(serde_json::Value::as_str)?
            .to_string(),
        _ => return None,
    };
    let root = root?;
    let full = resolve(&path, Some(root))?;
    if !is_plan_file(&full) || !full.is_file() {
        return None;
    }
    // Compared resolved, so `..` or a symlink cannot reach outside the
    // sandbox; `full` itself stays as given so its title is relative.
    let inside = match (full.canonicalize(), root.canonicalize()) {
        (Ok(file), Ok(root)) => file.starts_with(root),
        _ => false,
    };
    inside.then_some(full)
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

    #[test]
    fn a_saved_or_read_plan_opens_as_a_document() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        std::fs::create_dir_all(ws.join(".aura/plans")).unwrap();
        std::fs::write(ws.join(".aura/plans/a.plan.md"), "# A\n").unwrap();
        std::fs::write(ws.join("notes.md"), "# Notes\n").unwrap();
        std::fs::write(dir.path().join("x.plan.md"), "# X\n").unwrap();
        let root = Some(ws.as_path());
        let expected = ws.join(".aura/plans/a.plan.md");

        let saved = step(
            "create_plan",
            "{}",
            "Plan saved: .aura/plans/a.plan.md\nCreated plan 'A' with 0 todos:",
        );
        assert_eq!(plan_file(&saved, root), Some(expected.clone()));
        let read = step("read_file", r#"{"path":".aura/plans/a.plan.md"}"#, "1|# A");
        assert_eq!(plan_file(&read, root), Some(expected));

        let other = step("read_file", r#"{"path":"notes.md"}"#, "1|# Notes");
        assert_eq!(plan_file(&other, root), None, "only plan files");
        let gone = step("create_plan", "{}", "Plan saved: .aura/plans/gone.plan.md");
        assert_eq!(plan_file(&gone, root), None, "the file has to exist");
        let mut failed = saved;
        failed.status = ToolStatus::Error;
        assert_eq!(
            plan_file(&failed, root),
            None,
            "a failed call opens nothing"
        );
        let outside = step("read_file", r#"{"path":"../x.plan.md"}"#, "1|# X");
        assert_eq!(plan_file(&outside, root), None, "not outside the sandbox");
    }
}
