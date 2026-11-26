//! A tool call's arguments as lines under its open block.

use super::{format_elapsed, Body, Theme, INLINE_ARG_MAX};
use crate::renderer::command::script_lines;
use ratatui::{style::Style, text::Line};
use serde_json::{Map, Value};
use std::time::Duration;

/// `args` as [`pretty_args`] lays them out, under the rule.
pub(super) fn push_args(lines: &mut Vec<Line<'static>>, args: &str, body: &Body, theme: &Theme) {
    let muted = Style::default().fg(theme.colors.muted);
    for line in pretty_args(args).lines() {
        if line.is_empty() {
            lines.push(body.line(String::new(), muted, theme));
        } else {
            body.push_wrapped(lines, line, muted, theme);
        }
    }
}

/// A `run_command` call's arguments: the command as `$ …` rows, then how
/// it ran (`in web · background · timeout 2m 00s`) and any other
/// arguments. `None` while they stream in or when they hold no command.
pub(super) fn command_lines(args: &str, body: &Body, theme: &Theme) -> Option<Vec<Line<'static>>> {
    let Ok(Value::Object(mut args)) = serde_json::from_str::<Value>(args) else {
        return None;
    };
    let command = take_command(&mut args)?;
    let mut lines: Vec<Line<'static>> = script_lines(&command, body.width, theme)
        .into_iter()
        .map(|line| body.wrap_line(line, theme))
        .collect();

    let how = take_how(&mut args);
    args.remove("allow_shell");
    let muted = Style::default().fg(theme.colors.muted);
    if !how.is_empty() {
        body.push_wrapped(&mut lines, &how.join(" · "), muted, theme);
    }
    if !args.is_empty() {
        push_args(&mut lines, &Value::Object(args).to_string(), body, theme);
    }
    Some(lines)
}

/// The command a `run_command` call runs, taken out of `args`.
fn take_command(args: &mut Map<String, Value>) -> Option<String> {
    let mut take = |key: &str| args.remove(key);
    let command = match (take("shell_script"), take("command")) {
        (Some(Value::String(script)), _) | (None, Some(Value::String(script))) => script,
        _ => {
            let program = take("program")?;
            let words = take("args")
                .and_then(|a| a.as_array().cloned())
                .unwrap_or_default();
            std::iter::once(program.as_str()?.to_string())
                .chain(words.iter().filter_map(Value::as_str).map(shell_word))
                .collect::<Vec<_>>()
                .join(" ")
        }
    };
    Some(command)
}

/// How a `run_command` call runs (`in web`, `background`, `timeout 2m 00s`),
/// taken out of `args`.
fn take_how(args: &mut Map<String, Value>) -> Vec<String> {
    let mut take = |key: &str| args.remove(key);
    let background = take("background").and_then(|v| v.as_bool()) == Some(true);
    let mut how = Vec::new();
    if let Some(Value::String(dir)) = take("cwd").or_else(|| take("working_dir")) {
        how.push(format!("in {dir}"));
    }
    if background {
        how.push("background".to_string());
    }
    let timeout_ms = take("timeout_ms").and_then(|v| v.as_u64()).or_else(|| {
        take("timeout_secs")
            .and_then(|v| v.as_u64())
            .map(|s| s * 1000)
    });
    if let Some(ms) = timeout_ms.filter(|_| !background) {
        how.push(format!(
            "timeout {}",
            format_elapsed(Duration::from_millis(ms))
        ));
    }
    if let Some(cols) = take("tty_cols").and_then(|v| v.as_u64()) {
        how.push(format!("tty {cols} columns"));
    }
    how
}

/// `arg` as one shell word, quoted when it needs to be.
fn shell_word(arg: &str) -> String {
    let plain = !arg.is_empty()
        && arg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./=:,+@%".contains(c));
    if plain {
        arg.to_string()
    } else {
        format!("'{}'", arg.replace('\'', r"'\''"))
    }
}

/// Complete JSON object arguments as `key: value` lines, strings unescaped
/// and multi-line values indented under their key; empty for `{}`. Other
/// arguments, such as a prefix still streaming in, are shown as they are.
fn pretty_args(args: &str) -> String {
    let map = match serde_json::from_str::<Value>(args) {
        Ok(Value::Object(map)) => map,
        Ok(value) => {
            return serde_json::to_string_pretty(&value).unwrap_or_else(|_| args.to_string())
        }
        Err(_) => return args.trim().to_string(),
    };
    let mut out = Vec::new();
    for (key, value) in &map {
        let text = match value {
            Value::String(text) => text.trim_end().to_string(),
            Value::Array(_) | Value::Object(_) if value.to_string().len() > INLINE_ARG_MAX => {
                serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
            }
            _ => value.to_string(),
        };
        if text.contains('\n') {
            out.push(format!("{key}:"));
            out.extend(text.lines().map(|line| {
                if line.trim().is_empty() {
                    String::new()
                } else {
                    format!("  {line}")
                }
            }));
        } else {
            out.push(format!("{key}: {text}"));
        }
    }
    out.join("\n")
}
