//! Parsing the `run_command` JSON arguments into [`RunArgs`].

use super::policy::is_shell_program;
use crate::tool::ToolContext;

/// Parsed `run_command` arguments after pulling them out of the JSON
/// envelope. Pure data — every policy / allow-list decision lives in
/// [`enforce_command_policy`](super::policy::enforce_command_policy) so the parser never returns a partially
/// validated request.
///
/// Field semantics:
/// - `cwd`: alias-resolved (`cwd` / `working_dir`); `None` ⇒ workspace root.
/// - `timeout_ms`: `timeout_secs * 1000` if provided, else `timeout_ms`,
///   else `ctx.config.sync_threshold_ms`.
/// - `allow_shell`: per-call override or `ctx.config.command.allow_shell`.
/// - `shell_script`: alias-resolved (`shell_script` / legacy `command`).
/// - `program` + `cmd_args`: direct-spawn form. Mutually exclusive with
///   `shell_script` (enforced in policy).
/// - `tty_cols`: run in a pseudo-terminal of that width instead of pipes.
/// - `background`: hand the process to the context's background hook.
pub(super) struct RunArgs {
    pub(super) cwd: Option<String>,
    pub(super) timeout_ms: u64,
    pub(super) allow_shell: bool,
    pub(super) shell_script: Option<String>,
    pub(super) program: Option<String>,
    pub(super) cmd_args: Vec<String>,
    pub(super) tty_cols: Option<u16>,
    pub(super) background: bool,
}

/// Pull every `run_command` field out of the JSON envelope without
/// applying any policy. Defaults match the original inline behavior:
/// `cwd` / `working_dir` are aliased, `timeout_secs` overrides
/// `timeout_ms`, and the legacy `command` field is treated as a
/// `shell_script` alias.
pub(super) fn parse_run_args(args: &serde_json::Value, ctx: &ToolContext) -> RunArgs {
    let cwd = args["cwd"]
        .as_str()
        .or_else(|| args["working_dir"].as_str())
        .map(String::from);

    let timeout_ms = if let Some(secs) = args["timeout_secs"].as_u64() {
        secs * 1000
    } else {
        args["timeout_ms"]
            .as_u64()
            .unwrap_or(ctx.config.sync_threshold_ms)
    };

    let allow_shell = args["allow_shell"]
        .as_bool()
        .unwrap_or(ctx.config.command.allow_shell);

    // The legacy `command` field is treated as a shell_script alias
    // so the same gate applies. Callers shouldn't rely on it; the
    // field remains only to avoid breaking older tool proposals.
    let shell_script = args["shell_script"]
        .as_str()
        .or_else(|| args["command"].as_str())
        .map(without_stray_zero_width);

    let cmd_args: Vec<String> = args["args"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(without_stray_zero_width))
                .collect()
        })
        .unwrap_or_default();
    // The script already runs in a shell, so a bare shell `program` next
    // to it adds nothing.
    let program = args["program"]
        .as_str()
        .map(without_stray_zero_width)
        .filter(|p| !(shell_script.is_some() && cmd_args.is_empty() && is_shell_program(p)));

    let tty_cols = args["tty_cols"]
        .as_u64()
        .and_then(|cols| u16::try_from(cols).ok())
        .filter(|cols| *cols > 0);

    RunArgs {
        cwd,
        timeout_ms,
        allow_shell,
        shell_script,
        program,
        cmd_args,
        tty_cols,
        background: args["background"].as_bool().unwrap_or(false),
    }
}

/// Characters that print as nothing.
const ZERO_WIDTH: [char; 5] = ['\u{200b}', '\u{200c}', '\u{200d}', '\u{2060}', '\u{feff}'];

/// `text` without the zero-width characters next to ASCII or at its ends.
/// There they can only be slips, and one in a command word makes another
/// command (`python\u{200b}: command not found`). Between other characters
/// they join emoji or shape a script's letters, so they stay.
pub(super) fn without_stray_zero_width(text: &str) -> String {
    if !text.contains(&ZERO_WIDTH[..]) {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let ascii_or_end =
        |at: Option<usize>| at.and_then(|at| chars.get(at)).is_none_or(char::is_ascii);
    chars
        .iter()
        .enumerate()
        .filter(|&(at, c)| {
            !ZERO_WIDTH.contains(c)
                || !(ascii_or_end(at.checked_sub(1)) || ascii_or_end(Some(at + 1)))
        })
        .map(|(_, c)| *c)
        .collect()
}
