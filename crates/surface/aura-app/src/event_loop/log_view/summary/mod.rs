//! Short texts for the LOG tab's step rows: a tool call as its tool and
//! the argument that says what it does, and a tool's output in a few
//! words.

use aura_core_types::trace::FileTouch;
use serde_json::Value;

use super::super::compare::view::{headline, tokens};

/// Longest argument shown for a call.
const ARGUMENT_CHARS: usize = 72;

/// Longest text quoted from an output.
const QUOTE_CHARS: usize = 72;

/// Arguments that say what a call of a tool not known here does, in the
/// order looked for.
const ARGUMENT_KEYS: &[&str] = &[
    "path",
    "file_path",
    "pattern",
    "query",
    "command",
    "url",
    "name",
    "prompt",
];

/// `read_file src/x.rs L10-40`, `search_code "fn foo" in src`,
/// `run_command cargo test`.
pub(super) fn call_label(name: &str, input: &Value) -> String {
    let argument = argument(name, input);
    if argument.is_empty() {
        name.to_string()
    } else {
        format!("{name} {argument}")
    }
}

fn argument(name: &str, input: &Value) -> String {
    let text = |key: &str| {
        input
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
    };
    let short = |text: &str| headline(text, ARGUMENT_CHARS);
    let in_path = |what: String| match text("path").filter(|path| *path != ".") {
        Some(path) => format!("{what} in {}", short(path)),
        None => what,
    };
    match name {
        "read_file" => {
            let path = text("path").map(short).unwrap_or_default();
            let line = |key: &str| input.get(key).and_then(Value::as_u64);
            match (line("start_line"), line("end_line")) {
                (Some(start), Some(end)) => format!("{path} L{start}-{end}"),
                (Some(start), None) => format!("{path} from L{start}"),
                (None, Some(end)) => format!("{path} L1-{end}"),
                (None, None) => path,
            }
        }
        "search_code" => in_path(
            text("pattern").map_or_else(String::new, |pattern| format!("\"{}\"", short(pattern))),
        ),
        "find_files" => in_path(text("pattern").map(short).unwrap_or_default()),
        "run_command" => short(&command(input)),
        "task" => {
            let prompt = text("prompt").map(short).unwrap_or_default();
            match text("subagent_type") {
                Some(kind) if prompt.is_empty() => format!("@{kind}"),
                Some(kind) => format!("@{kind} {prompt}"),
                None => prompt,
            }
        }
        _ => ARGUMENT_KEYS
            .iter()
            .find_map(|key| text(key))
            .or_else(|| {
                input
                    .as_object()?
                    .values()
                    .filter_map(Value::as_str)
                    .map(str::trim)
                    .find(|s| !s.is_empty())
            })
            .map(short)
            .unwrap_or_default(),
    }
}

/// The command a `run_command` call runs.
fn command(input: &Value) -> String {
    if let Some(script) = ["shell_script", "command"]
        .iter()
        .find_map(|key| input.get(*key).and_then(Value::as_str))
    {
        return script.to_string();
    }
    let program = input.get("program").and_then(Value::as_str);
    let args = input
        .get("args")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str);
    program
        .into_iter()
        .chain(args)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// A tool's output, as the log holds it.
pub(super) struct Output<'a> {
    pub tool: &'a str,
    /// Whole when `complete`, else its start
    pub text: &'a str,
    pub complete: bool,
    /// Length of the whole output
    pub chars: usize,
    pub is_error: bool,
    /// `executor`, `cache` or `blocked`
    pub source: &'a str,
    pub files: &'a [FileTouch],
}

/// `12 entries`, `340 lines`, `exit 0 · 3 lines`, `no matches`,
/// `error: …`. Counts read from the start of a longer output are
/// estimates, as `~340 lines`.
pub(super) fn result_summary(output: &Output<'_>) -> String {
    let text = if output.complete {
        output.text
    } else {
        output.text.strip_suffix('…').unwrap_or(output.text)
    };
    if output.source == "blocked" {
        return format!("blocked: {}", quote(text));
    }
    let said = what_it_said(output, text);
    if output.source == "cache" {
        format!("{said} · cached")
    } else {
        said
    }
}

fn what_it_said(output: &Output<'_>, text: &str) -> String {
    if output.tool == "run_command" || text.starts_with("{\"exit_code\"") {
        if let Some(said) = command_result(output, text) {
            return said;
        }
    }
    if output.is_error {
        return format!("error: {}", quote(text));
    }
    let count = |one: &str, many: &str| counted(output, text, one, many);
    match output.tool {
        "list_files" if text.trim().is_empty() => "empty".to_string(),
        "list_files" => count("entry", "entries"),
        "find_files" if text.starts_with("No files found") => "no files".to_string(),
        "find_files" => count("file", "files"),
        "search_code" if text.starts_with("No matches found") => "no matches".to_string(),
        "search_code" => count("match", "matches"),
        "read_file" if text.trim().is_empty() => "empty file".to_string(),
        "read_file" if text.starts_with('(') => quote(text),
        "read_file" => count("line", "lines"),
        "stat_file" => stat(text).unwrap_or_else(|| quote(text)),
        "write_file" | "edit_file" | "delete_file" => {
            changed(output.files).unwrap_or_else(|| quote(text))
        }
        "task" => task_result(text).unwrap_or_else(|| quote(text)),
        _ if text.trim().is_empty() => "no output".to_string(),
        _ if output.complete && text.trim().lines().count() == 1 => quote(text),
        _ => count("line", "lines"),
    }
}

/// `N lines` of `text`; `~N lines` estimated from the start of a longer
/// output; its length when not one line of it is whole.
fn counted(output: &Output<'_>, text: &str, one: &str, many: &str) -> String {
    match line_count(text, output.complete, output.chars) {
        Some((n, true)) => format!("{n} {}", if n == 1 { one } else { many }),
        Some((n, false)) => format!("~{n} {many}"),
        None => length(output.chars),
    }
}

/// Lines of `text` that hold something, without truncation notes, and
/// whether the count is exact. A start of a longer output is scaled up
/// by how much of it there is.
fn line_count(text: &str, complete: bool, chars: usize) -> Option<(usize, bool)> {
    let counts = |line: &&str| {
        let line = line.trim();
        !line.is_empty() && !line.starts_with("... [")
    };
    if complete {
        return Some((text.lines().filter(counts).count(), true));
    }
    let whole = &text[..text.rfind('\n')?];
    let seen = whole.lines().filter(counts).count();
    let seen_chars = whole.chars().count() + 1;
    if seen == 0 {
        return None;
    }
    Some((estimate(seen, seen_chars, chars), false))
}

/// `seen` things in `seen_chars` characters, scaled to `chars`.
fn estimate(seen: usize, seen_chars: usize, chars: usize) -> usize {
    (seen.saturating_mul(chars) / seen_chars.max(1)).max(seen)
}

fn length(chars: usize) -> String {
    format!("{} chars", tokens(u64::try_from(chars).unwrap_or(u64::MAX)))
}

/// `exit 0 · 3 lines`, `exit 1 · no output`, from a `run_command`
/// envelope, whole or its start.
fn command_result(output: &Output<'_>, text: &str) -> Option<String> {
    let exit = json_number(text, "exit_code");
    let lines = match serde_json::from_str::<Value>(text) {
        Ok(envelope) => {
            let field = |key: &str| envelope.get(key).and_then(Value::as_str).unwrap_or("");
            let lines = [field("stdout"), field("stderr")]
                .iter()
                .map(|s| s.lines().count())
                .sum::<usize>();
            match lines {
                0 => "no output".to_string(),
                1 => "1 line".to_string(),
                n => format!("{n} lines"),
            }
        }
        Err(_) if text.starts_with('{') && !output.complete => {
            let seen = text.matches("\\n").count();
            if seen == 0 {
                length(output.chars)
            } else {
                format!(
                    "~{} lines",
                    estimate(seen, text.chars().count(), output.chars)
                )
            }
        }
        Err(_) => return None,
    };
    let exit = match exit {
        Some(code) => format!("exit {code}"),
        None if output.is_error => "failed".to_string(),
        None => "done".to_string(),
    };
    Some(format!("{exit} · {lines}"))
}

/// The number after `"key":` in JSON text, whole or its start.
fn json_number(text: &str, key: &str) -> Option<i64> {
    let at = text.find(&format!("\"{key}\":"))? + key.len() + 3;
    let rest = text[at..].trim_start();
    let end = rest
        .char_indices()
        .find(|&(i, c)| !(c.is_ascii_digit() || (i == 0 && c == '-')))
        .map_or(rest.len(), |(i, _)| i);
    rest[..end].parse().ok()
}

/// The string after `"key":` in JSON text, whole or its start.
fn json_string(text: &str, key: &str) -> Option<String> {
    let at = text.find(&format!("\"{key}\":\""))? + key.len() + 4;
    let mut out = String::new();
    let mut chars = text[at..].chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => break,
            '\\' => match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some(other) => out.push(other),
                None => break,
            },
            c => out.push(c),
        }
    }
    Some(out)
}

/// `file · 114 KB` or `folder`, from `stat_file`'s `key=value` lines.
fn stat(text: &str) -> Option<String> {
    let value = |key: &str| {
        text.lines()
            .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
    };
    if value("is_dir") == Some("true") {
        return Some("folder".to_string());
    }
    let size: u64 = value("size")?.parse().ok()?;
    Some(format!("file · {}", bytes(size)))
}

fn bytes(n: u64) -> String {
    const KB: u64 = 1024;
    if n < KB {
        format!("{n} B")
    } else if n < KB * KB {
        format!("{} KB", n.div_ceil(KB))
    } else {
        format!("{} MB", n.div_ceil(KB * KB))
    }
}

/// `+12 -3`, `created +40`, `deleted`, `3 files +20 -8`: what a write
/// changed.
fn changed(files: &[FileTouch]) -> Option<String> {
    let changed: Vec<&FileTouch> = files.iter().filter(|f| f.kind != "read").collect();
    match changed.as_slice() {
        [] => None,
        [file] if file.kind == "delete" => Some("deleted".to_string()),
        [file] if file.kind == "create" => Some(format!("created +{}", file.added)),
        [file] => Some(format!("+{} -{}", file.added, file.removed)),
        many => {
            let added: u32 = many.iter().map(|f| f.added).sum();
            let removed: u32 = many.iter().map(|f| f.removed).sum();
            Some(format!("{} files +{added} -{removed}", many.len()))
        }
    }
}

/// A subagent's reply, or why it stopped.
fn task_result(text: &str) -> Option<String> {
    let Ok(outcome) = serde_json::from_str::<Value>(text) else {
        return json_string(text, "final_message").map(|reply| quote(&reply));
    };
    let reply = outcome.get("final_message")?.as_str()?;
    let exit = outcome.get("exit")?;
    let reason = exit.get("reason").and_then(Value::as_str).unwrap_or("");
    Some(match exit.get("kind")?.as_str()? {
        "completed" if reply.trim().is_empty() => "no reply".to_string(),
        "completed" => quote(reply),
        kind if reason.is_empty() => kind.to_string(),
        kind => format!("{kind}: {}", quote(reason)),
    })
}

/// The first line of `text` that holds something, without a `12|` line
/// number, its whitespace run together and cut short.
fn quote(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    let line = match line.split_once('|') {
        Some((number, rest)) if number.trim().parse::<u64>().is_ok() => rest,
        _ => line,
    };
    headline(
        &line.split_whitespace().collect::<Vec<_>>().join(" "),
        QUOTE_CHARS,
    )
}

#[cfg(test)]
mod tests;
