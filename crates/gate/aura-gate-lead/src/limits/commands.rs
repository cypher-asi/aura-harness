//! What the lead may run with `run_command`: checks, linters and
//! formatters in check mode, and commands that only look at the
//! workspace or its history, each optionally piped into filters.

use aura_gate_evidence::checks::is_check_part;

/// Name of the tool whose commands [`allows_command`] limits.
pub(crate) const RUN_COMMAND: &str = "run_command";

/// What a command the lead runs may not contain anywhere: command and
/// process substitution.
const SUBSTITUTIONS: [&str; 4] = ["`", "$(", "<(", ">("];

/// What [`allows_command`] puts where one command of a sequence ends and
/// the next starts: at `&&`, `||`, `;` and new lines.
const SEQUENCE: &str = "\u{1}";

/// What [`allows_command`] puts at each pipe between two stages of a
/// pipeline.
const PIPE: &str = "\u{2}";

/// Programs that may read a pipeline's output (never a file of their
/// own choosing first).
const FILTERS: &[&str] = &["head", "tail", "grep", "wc", "sort", "uniq", "cut"];

/// `git` subcommands that only look.
const GIT_LOOKS: &[&str] = &[
    "status",
    "diff",
    "log",
    "show",
    "rev-parse",
    "ls-files",
    "blame",
];

/// Whether the lead may run `command`: every command in it, as split at
/// `&&`, `||`, `;` and new lines, is a check (tests, builds, linters,
/// type checks) or an inspection command (`git status`, `git diff`,
/// `git log`, `git show`, `ls`, `pwd`, `cd`), optionally piped into
/// filters such as `tail` or `grep`. Command substitution, file
/// redirection and background jobs are refused.
#[must_use]
pub(crate) fn allows_command(command: &str) -> bool {
    if SUBSTITUTIONS.iter().any(|s| command.contains(s)) {
        return false;
    }
    let marked = command
        .replace("&&", SEQUENCE)
        .replace("||", SEQUENCE)
        .replace([';', '\n'], SEQUENCE)
        .replace('|', PIPE);
    let mut sequences = marked
        .split(SEQUENCE)
        .filter(|sequence| !sequence.trim().is_empty())
        .peekable();
    sequences.peek().is_some() && sequences.all(allows_pipeline)
}

/// Whether one command of a sequence may run: its first stage checks or
/// inspects, and every stage after it is a filter.
fn allows_pipeline(pipeline: &str) -> bool {
    pipeline.split(PIPE).enumerate().all(|(i, stage)| {
        stage_words(stage).is_some_and(|words| {
            if i == 0 {
                checks_or_inspects(&words)
            } else {
                is_filter(&words)
            }
        })
    })
}

/// The words of one pipeline stage without its harmless redirections
/// (`2>&1`, `2>/dev/null`); `None` when it redirects to or from a file,
/// runs in the background, or is empty.
fn stage_words(stage: &str) -> Option<Vec<&str>> {
    let mut words = Vec::new();
    for word in stage.split_whitespace() {
        if is_harmless_redirection(word) {
            continue;
        }
        if word.contains(['>', '<', '&']) {
            return None;
        }
        words.push(word);
    }
    (!words.is_empty()).then_some(words)
}

/// Whether `word` only sends output to another descriptor (`2>&1`) or to
/// `/dev/null`.
fn is_harmless_redirection(word: &str) -> bool {
    let Some(target) = word
        .trim_start_matches(|c: char| c.is_ascii_digit() || c == '&')
        .strip_prefix('>')
    else {
        return false;
    };
    target == "/dev/null"
        || target
            .strip_prefix('&')
            .is_some_and(|fd| !fd.is_empty() && fd.bytes().all(|b| b.is_ascii_digit()))
}

/// Whether the first stage of a pipeline, `words`, runs a check (tests,
/// builds, type checks), a linter or formatter in check mode, or an
/// inspection command.
fn checks_or_inspects(words: &[&str]) -> bool {
    is_lint(words) || is_inspection(words) || is_check_part(words)
}

/// Whether a later stage of a pipeline, `words`, only filters what comes
/// before it.
fn is_filter(words: &[&str]) -> bool {
    words
        .first()
        .is_some_and(|word| FILTERS.contains(&program(word)))
}

/// The program `word` names, without its directory.
fn program(word: &str) -> &str {
    word.rsplit('/').next().unwrap_or(word)
}

/// Whether `words` run a linter or formatter in check mode.
fn is_lint(words: &[&str]) -> bool {
    let words = match words {
        ["npx" | "bunx", rest @ ..] => rest,
        _ => words,
    };
    let [first, rest @ ..] = words else {
        return false;
    };
    let checking = rest.contains(&"--check");
    match program(first) {
        "eslint" | "flake8" | "pylint" | "golangci-lint" | "shellcheck" | "hadolint" => true,
        "ruff" => rest.first() == Some(&"check"),
        "cargo" => rest.first() == Some(&"fmt") && checking,
        "black" | "prettier" | "rustfmt" | "gofmt" => checking || rest.contains(&"-l"),
        _ => false,
    }
}

/// Whether `words` only look at the workspace or its history.
fn is_inspection(words: &[&str]) -> bool {
    let [first, args @ ..] = words else {
        return false;
    };
    match program(first) {
        "ls" | "pwd" | "cd" => true,
        "git" => is_git_look(args),
        _ => false,
    }
}

/// Whether `git` run with `args` only looks: its subcommand, after the
/// global options, is one of [`GIT_LOOKS`], and it writes no file
/// (`--output`).
fn is_git_look(args: &[&str]) -> bool {
    let mut args = args.iter();
    while let Some(&arg) = args.next() {
        match arg {
            "-C" | "-c" => {
                args.next();
            }
            flag if flag.starts_with('-') => {}
            subcommand => {
                return GIT_LOOKS.contains(&subcommand)
                    && args.all(|arg| !arg.starts_with("--output"));
            }
        }
    }
    false
}

/// What the lead is told when it runs a command it may not.
#[must_use]
pub(crate) fn refused_command(command: &str) -> String {
    format!(
        "Not run: under Gate your own run_command only runs checks (tests, builds, linters, \
         type checks) and inspection commands (git status, git diff, git log, git show, ls, \
         pwd). Delegate `{command}` with the task tool: an implement subtask for changes, \
         an explorer subtask for reading."
    )
}
