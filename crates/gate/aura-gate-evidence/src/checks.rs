//! Whether the tests or build a turn ran last passed: the strongest
//! automatic signal of how a turn went.
//!
//! A check is a `run_command` call whose command line runs a test suite,
//! a build or a type check (see [`is_check_command`]). Its result counts
//! only when the command ran to its end: a passing call, or a failing one
//! that reports its exit code. An exit status that may be another
//! command's is decided by what the check printed instead: a passing one
//! when the check's output is piped on or a command after it runs
//! whatever it did (`pytest | tail`, `pytest; echo done`), a failing one
//! when any command follows the check (`pytest | grep Error`, `pytest;
//! pyflakes x`). Output that shows the tests never ran (`no tests ran`)
//! and none passed is a failure. Calls the policy denied, that could not
//! start, or that went to the background say nothing, as does an exit
//! status decided by output that shows neither failures nor a pass.
//!
//! A check run with the change stashed away (`git stash; pytest; git
//! stash pop`) ran the code as it was before, so it is not the change's
//! check. When it shows failures, a failing run of the same check with
//! the change in place is not counted either: those failures were there
//! before ([`Stashing`]).

use aura_model::{ContentBlock, Message, ToolResultContent};
use serde_json::Value;

use crate::evidence::{printed, shows_failures};

/// Name of the tool that runs commands.
const RUN_COMMAND: &str = "run_command";

/// Field (in a JSON result) or line prefix (in a text one) with the exit
/// code of a failed `run_command` call that ran to its end.
const EXIT_CODE: &str = "exit_code";

/// Text a check prints when its tests never ran: none were collected, a
/// path it was handed is not there, or the runner, a module the tests
/// import or the script is missing.
const NEVER_RAN: &[&str] = &[
    "no tests ran",
    "ERROR: file not found",
    "No module named",
    "ModuleNotFoundError",
    "command not found",
    "Missing script",
];

/// Zero-width characters a command may hold by a slip.
const ZERO_WIDTH: [char; 5] = ['\u{200b}', '\u{200c}', '\u{200d}', '\u{2060}', '\u{feff}'];

/// A check that ran to its end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckSeen {
    /// The command line it ran.
    pub command: String,
    /// Whether it passed.
    pub passed: bool,
    /// Whether its exit status may have been another command's, so that
    /// what it printed decided whether it passed.
    pub by_output: bool,
    /// What it printed, as the model was shown it.
    pub output: String,
}

/// Whether the last check in `messages` passed; `None` when they hold no
/// check that ran to its end.
#[must_use]
pub fn last_check(messages: &[Message]) -> Option<bool> {
    last_check_seen(messages).map(|check| check.passed)
}

/// The last check in `messages` that ran to its end, with its command
/// and output; `None` when they hold none.
#[must_use]
pub fn last_check_seen(messages: &[Message]) -> Option<CheckSeen> {
    checks_seen(messages).pop()
}

/// Every check in `messages` that ran to its end, in the order they were
/// called.
#[must_use]
pub fn checks_seen(messages: &[Message]) -> Vec<CheckSeen> {
    checks_placed(messages)
        .into_iter()
        .map(|(_, check)| check)
        .collect()
}

/// [`checks_seen`], each check with the place of its call among the tool
/// calls in `messages` (0 for the first).
#[must_use]
pub fn checks_placed(messages: &[Message]) -> Vec<(usize, CheckSeen)> {
    let mut checks: Vec<Called<'_>> = Vec::new();
    let mut calls = 0;
    // Whether the change is stashed away as of the latest call, and the
    // calls that changed that, with what it was before them.
    let mut stashed = false;
    let mut stashes: Vec<(&str, bool)> = Vec::new();
    for message in messages {
        for block in &message.content {
            match block {
                ContentBlock::ToolUse { id, name, input } => {
                    let at = calls;
                    calls += 1;
                    if name != RUN_COMMAND {
                        continue;
                    }
                    let command = command_line(input);
                    let stashing = Stashing::of(&command, stashed);
                    if stashing.stashed != stashed {
                        stashes.push((id.as_str(), stashed));
                        stashed = stashing.stashed;
                    }
                    if input["background"].as_bool() != Some(true) && is_check_command(&command) {
                        checks.push(Called {
                            id,
                            at,
                            command,
                            stashing,
                            result: None,
                        });
                    }
                }
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } => {
                    // A stash that failed stashed nothing.
                    if let Some((_, before)) = stashes
                        .iter()
                        .find(|(id, _)| *is_error && id == tool_use_id)
                    {
                        stashed = *before;
                    }
                    if let Some(check) = checks.iter_mut().find(|check| check.id == tool_use_id) {
                        let output = match content {
                            ToolResultContent::Text(text) => text.clone(),
                            ToolResultContent::Json(value) => value.to_string(),
                        };
                        check.result = Some((output, *is_error));
                    }
                }
                _ => {}
            }
        }
    }
    // The checks a run without the change showed failing.
    let failed_before: Vec<String> = checks
        .iter()
        .filter(|check| {
            check
                .result
                .as_ref()
                .is_some_and(|(output, _)| shows_failures(&printed(output)))
        })
        .flat_map(|check| check.stashing.without.iter().cloned())
        .collect();
    checks
        .into_iter()
        .filter(|check| check.stashing.without.is_empty())
        .filter_map(|check| {
            let (output, is_error) = check.result?;
            let (passed, by_output) = verdict(&output, is_error, &check.command)?;
            let with = &check.stashing.with;
            if !passed && !with.is_empty() && with.iter().all(|run| failed_before.contains(run)) {
                return None;
            }
            Some((
                check.at,
                CheckSeen {
                    command: check.command,
                    passed,
                    by_output,
                    output,
                },
            ))
        })
        .collect()
}

/// A `run_command` call that runs a check.
struct Called<'a> {
    /// Its call id
    id: &'a str,
    /// Its place among the tool calls
    at: usize,
    command: String,
    stashing: Stashing,
    /// What it printed and whether it failed, once it has a result
    result: Option<(String, bool)>,
}

/// Which of a command line's checks ran on the change, and which with
/// the change stashed away: between a `git stash` and the `git stash
/// pop` or `apply` that brings it back, the working tree holds the code
/// as it was before, and a model runs its tests there to see which
/// failures were there already. Each check is kept as its own words
/// ([`check_run`]), so the same check with and without the change reads
/// the same.
#[derive(Debug, Default, PartialEq, Eq)]
struct Stashing {
    /// The checks run with the change in place
    with: Vec<String>,
    /// The checks run with the change stashed away
    without: Vec<String>,
    /// Whether the change is stashed away when the command line ends
    stashed: bool,
}

impl Stashing {
    /// What `command` runs, the change being `stashed` away or not when
    /// it starts.
    fn of(command: &str, mut stashed: bool) -> Self {
        let (mut with, mut without) = (Vec::new(), Vec::new());
        for (part, _) in parts(command) {
            let words = simple_command(words(part), true);
            if let ["git", "stash", rest @ ..] = words.as_slice() {
                match rest.first().copied() {
                    Some("pop" | "apply") => stashed = false,
                    Some("list" | "show" | "drop" | "clear" | "branch" | "create" | "store") => {}
                    _ => stashed = true,
                }
            } else if let Some(run) = check_run(&words) {
                if stashed {
                    without.push(run);
                } else {
                    with.push(run);
                }
            }
        }
        Self {
            with,
            without,
            stashed,
        }
    }
}

/// The check `words` (a simple command, as [`simple_command`] leaves it)
/// runs, without where its output is sent; `None` when it runs none.
fn check_run(words: &[&str]) -> Option<String> {
    runs_check(words).then(|| {
        words
            .iter()
            .filter(|word| !word.contains('>'))
            .copied()
            .collect::<Vec<_>>()
            .join(" ")
    })
}

/// Pass or fail from the result of a check that ran `command`, `output`
/// as the model was shown it, and whether what the check printed decided
/// that; `None` when the command did not run to its end. What the check
/// printed ([`output_verdict`]) decides a passing exit that is not the
/// check's own ([`exit_status_masked`]), and a failing exit that a
/// command after the check may have set ([`runs_after_check`]).
fn verdict(output: &str, is_error: bool, command: &str) -> Option<(bool, bool)> {
    if !is_error {
        if exit_status_masked(command) {
            return output_verdict(&printed(output)).map(|passed| (passed, true));
        }
        return Some((true, false));
    }
    let exited = serde_json::from_str::<Value>(output).map_or_else(
        |_| {
            output
                .trim_start()
                .strip_prefix(EXIT_CODE)
                .is_some_and(|rest| rest.starts_with(':'))
        },
        |value| has_exit_code(&value),
    );
    if !exited {
        return None;
    }
    if runs_after_check(command) {
        return output_verdict(&printed(output)).map(|passed| (passed, true));
    }
    Some((false, false))
}

/// What a check printed says of how it went, when its exit status may be
/// another command's: failed when it shows failures or errors
/// ([`shows_failures`]), or that the tests never ran ([`NEVER_RAN`]) and
/// no pass; passed when it reports a pass ([`reports_passes`]); `None`
/// when it shows neither.
fn output_verdict(printed: &str) -> Option<bool> {
    let passes = reports_passes(printed);
    if shows_failures(printed) || (!passes && NEVER_RAN.iter().any(|text| printed.contains(text))) {
        Some(false)
    } else {
        passes.then_some(true)
    }
}

/// Whether `printed` reports tests that passed or a build that finished:
/// a count above zero before `passed` or `passing` (`12 passed in 0.46s`,
/// `Tests: 3 passed`, `5 passing`), cargo's `test result: ok` or its
/// `Finished … target(s) in` line, or go's `ok` line for a package.
fn reports_passes(printed: &str) -> bool {
    if printed.contains("test result: ok")
        || printed.lines().any(|line| {
            line.starts_with("ok  ")
                || line.starts_with("ok\t")
                || (line.trim_start().starts_with("Finished ") && line.contains("target(s) in"))
        })
    {
        return true;
    }
    let mut previous = "";
    for word in printed.split_whitespace() {
        let bare = word.trim_matches(|c: char| !c.is_alphanumeric());
        if matches!(bare, "passed" | "passing") && previous.parse::<u32>().is_ok_and(|n| n > 0) {
            return true;
        }
        previous = bare;
    }
    false
}

fn has_exit_code(result: &Value) -> bool {
    result.get(EXIT_CODE).is_some_and(Value::is_i64)
}

/// The command line a `run_command` call runs: its script, or its program
/// and arguments, without the stray zero-width characters that
/// `run_command` drops before running them ([`without_stray_zero_width`]).
#[must_use]
pub fn command_line(input: &Value) -> String {
    if let Some(script) = ["shell_script", "command"]
        .iter()
        .find_map(|key| input[*key].as_str())
    {
        return without_stray_zero_width(script);
    }
    let mut words = vec![input["program"].as_str().unwrap_or_default()];
    if let Some(args) = input["args"].as_array() {
        words.extend(args.iter().filter_map(Value::as_str));
    }
    without_stray_zero_width(&words.join(" "))
}

/// `text` without the zero-width characters next to ASCII or at its ends,
/// where they can only be slips (`python\u{200b} -m pytest`). Between
/// other characters they join emoji or shape a script's letters, so they
/// stay. `run_command` drops the same ones, so the two must agree.
fn without_stray_zero_width(text: &str) -> String {
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

/// Whether `command` runs tests, a build or a type check in any of its
/// `&&`/`;`/`|` parts, counting a shell that runs the check for it:
/// `bash -c '<check>'`, or a script handed the check, as in `sh .swe/run
/// '<check>'` (which runs it in the task's container).
#[must_use]
pub fn is_check_command(command: &str) -> bool {
    command
        .split(['&', ';', '|', '\n'])
        .any(|part| is_check_words(words(part), true))
}

/// What joins one simple command of a command line to the next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Joint {
    /// `|`: the next command reads this one's output
    Pipe,
    /// `&&`: the next runs only when this one succeeded
    And,
    /// `||`: the next runs only when this one failed
    Or,
    /// `;`, a new line or `&`: the next runs whatever this one did
    Then,
    /// Nothing follows
    End,
}

/// The simple commands of `command`, each with what joins it to the
/// next. Quotes are not followed, as in [`is_check_command`]: a wrapper
/// such as `sh .swe/run '<check> | tail'` is read as its script. A `&`
/// that is part of a redirection (`2>&1`, `&>`) joins nothing.
fn parts(command: &str) -> Vec<(&str, Joint)> {
    let bytes = command.as_bytes();
    let mut out = Vec::new();
    let (mut start, mut i) = (0, 0);
    while i < bytes.len() {
        let two = bytes.get(i + 1).copied();
        let (joint, width) = match bytes[i] {
            b'|' if two == Some(b'|') => (Joint::Or, 2),
            b'|' => (Joint::Pipe, 1),
            b'&' if two == Some(b'&') => (Joint::And, 2),
            b'&' if two == Some(b'>') || (i > 0 && bytes[i - 1] == b'>') => {
                i += 1;
                continue;
            }
            b'&' | b';' | b'\n' => (Joint::Then, 1),
            _ => {
                i += 1;
                continue;
            }
        };
        out.push((&command[start..i], joint));
        i += width;
        start = i;
    }
    out.push((&command[start..], Joint::End));
    out
}

fn words(part: &str) -> Vec<&str> {
    part.split_whitespace()
        .map(|word| word.trim_matches(['\'', '"']))
        .filter(|word| !word.is_empty())
        .collect()
}

/// Whether the exit status of `command` is not that of the last check it
/// runs, so that its passing says nothing of the check: the check's
/// output is piped on (`pytest | tail -20` exits as `tail` does, unless
/// `pipefail` is set), or another command runs after it whatever it did
/// (`pytest; echo done`, `pytest || true`). `false` for a command that
/// runs no check.
#[must_use]
pub fn exit_status_masked(command: &str) -> bool {
    let parts = parts(command);
    let Some(last) = parts
        .iter()
        .rposition(|(part, _)| is_check_words(words(part), true))
    else {
        return false;
    };
    let follows = parts[last + 1..]
        .iter()
        .any(|(part, _)| !part.trim().is_empty());
    match parts[last].1 {
        Joint::Pipe => !command.contains("pipefail"),
        Joint::Or | Joint::Then => follows,
        Joint::And | Joint::End => false,
    }
}

/// Whether another command runs after the last check in `command`,
/// joined to it in any way (`|`, `&&`, `||`, `;`), so that a failing exit
/// status may be that command's rather than the check's: `pytest | grep
/// Error` exits 1 when `grep` finds nothing, and `pytest && pyflakes x`
/// when `pyflakes` is not installed. `false` for a command that runs no
/// check.
fn runs_after_check(command: &str) -> bool {
    let parts = parts(command);
    parts
        .iter()
        .rposition(|(part, _)| is_check_words(words(part), true))
        .is_some_and(|last| {
            parts[last + 1..]
                .iter()
                .any(|(part, _)| !part.trim().is_empty())
        })
}

/// Whether one simple command (its words) is a check. Leading variable
/// assignments and wrappers (`env`, `time`, `timeout 600`, `npx`, ...)
/// are skipped. A shell handed a check is not one: the script it runs
/// may do anything.
#[must_use]
pub fn is_check_part(words: Vec<&str>) -> bool {
    is_check_words(words, false)
}

/// [`is_check_part`], looking through a shell that runs a check when
/// `through_shells`.
fn is_check_words(words: Vec<&str>, through_shells: bool) -> bool {
    runs_check(&simple_command(words, through_shells))
}

/// The command `words` come down to: without leading variable
/// assignments and wrappers (`env`, `time`, `timeout 600`, `npx`,
/// `python -m`, ...), and, when `through_shells`, what a shell is handed
/// to run in place of the shell.
fn simple_command(mut words: Vec<&str>, through_shells: bool) -> Vec<&str> {
    loop {
        match words.first().copied() {
            Some(word) if word.contains('=') && !word.starts_with('-') => {
                words.remove(0);
            }
            Some("env" | "time" | "nice" | "npx" | "bunx" | "exec" | "sudo") => {
                words.remove(0);
            }
            Some("timeout") => {
                words.drain(..words.len().min(2));
            }
            Some("uv" | "poetry" | "pipenv" | "pdm") if words.get(1) == Some(&"run") => {
                words.drain(..2);
            }
            Some("python" | "python3") if words.get(1) == Some(&"-m") => {
                words.drain(..2);
            }
            Some(program)
                if through_shells
                    && matches!(
                        program.rsplit('/').next().unwrap_or(program),
                        "sh" | "bash" | "zsh" | "dash"
                    ) =>
            {
                words = shell_command(&words[1..]);
            }
            _ => return words,
        }
    }
}

/// Whether `words`, a simple command as [`simple_command`] leaves it,
/// runs tests, a build or a type check.
fn runs_check(words: &[&str]) -> bool {
    let Some(&program) = words.first() else {
        return false;
    };
    let program = program.rsplit('/').next().unwrap_or(program);
    let sub = words.get(1).copied().unwrap_or_default();
    let script = if sub == "run" {
        words.get(2).copied().unwrap_or_default()
    } else {
        sub
    };
    match program {
        "cargo" => matches!(
            sub,
            "test" | "t" | "build" | "b" | "check" | "c" | "clippy" | "nextest"
        ),
        "npm" | "pnpm" | "yarn" | "bun" => matches!(sub, "test" | "t") || is_check_script(script),
        "go" => matches!(sub, "test" | "build" | "vet"),
        "make" | "just" => sub.is_empty() || is_check_script(sub),
        "gradle" | "gradlew" | "mvn" | "mvnw" | "dotnet" | "swift" | "bazel" | "zig" | "mix"
        | "deno" => is_check_script(sub) || matches!(sub, "verify" | "package"),
        "cmake" => words.contains(&"--build"),
        "pytest" | "py.test" | "tox" | "nox" | "unittest" | "jest" | "vitest" | "mocha" | "tsc"
        | "mypy" | "pyright" | "ctest" | "ninja" | "rspec" | "phpunit" => true,
        _ => false,
    }
}

/// The command a shell given `args` runs: what follows `-c` (or a flag
/// cluster holding `c`, as in `-lc`), else what follows the script it
/// runs, which a wrapper script runs in turn. Empty for a bare script.
fn shell_command<'a>(args: &[&'a str]) -> Vec<&'a str> {
    let is_flag = |word: &str| word.starts_with('-') && !word.starts_with("--");
    if let Some(at) = args
        .iter()
        .position(|word| is_flag(word) && word.contains('c'))
    {
        return args[at + 1..].to_vec();
    }
    args.iter()
        .skip_while(|word| word.starts_with('-'))
        .skip(1)
        .copied()
        .collect()
}

/// Whether a package script or build target named `name` runs checks.
fn is_check_script(name: &str) -> bool {
    let name = name.split(':').next().unwrap_or(name);
    matches!(
        name,
        "test" | "tests" | "check" | "build" | "typecheck" | "type-check" | "lint" | "ci"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_check_whose_output_is_piped_on_does_not_set_the_exit_status() {
        for command in [
            "pytest -q | tail -20",
            "python -m pytest -q test/units 2>&1 | grep -E 'passed|failed|FAILED|Error' | head -20",
            "sh .swe/run python -m pytest -q -p no:cacheprovider test/units/config 2>&1 | grep -E 'passed|failed' | head -20",
            "cd app && cargo test 2>&1 | tail -5",
            "npm test; echo done",
            "cargo test || true",
            "pytest -q\necho finished",
        ] {
            assert!(exit_status_masked(command), "{command}");
        }
        for command in [
            "pytest -q",
            "cargo test 2>&1",
            "cd app && cargo test",
            "cargo build && cargo test",
            "set -o pipefail; pytest -q | tail -20",
            "bash -c 'set -o pipefail && pytest | tee log'",
            "pytest -q && echo ok",
            "ls | head; pytest -q",
            "ls -la | head",
            "cargo test;",
        ] {
            assert!(!exit_status_masked(command), "{command}");
        }
    }

    #[test]
    fn every_check_that_ran_is_seen_in_order() {
        let messages = [
            call("a", json!({"command": "cargo test"})),
            result("a", "exit_code: 1\nboom", true),
            call("b", json!({"command": "ls"})),
            result("b", "src", false),
            call("c", json!({"command": "cargo test | tail -3"})),
            result("c", "test result: ok. 3 passed; 0 failed", false),
            call("d", json!({"command": "cargo test", "background": true})),
            result("d", "started", false),
        ];
        let seen = checks_seen(&messages);
        assert_eq!(
            seen.iter()
                .map(|check| (check.command.as_str(), check.passed))
                .collect::<Vec<_>>(),
            [("cargo test", false), ("cargo test | tail -3", true)]
        );
        assert_eq!(last_check_seen(&messages), seen.last().cloned());
    }

    #[test]
    fn test_and_build_commands_are_checks() {
        for command in [
            "cargo test -p aura-agent-gate",
            "cargo build --release",
            "cargo clippy --workspace -- -D warnings",
            "cargo nextest run",
            "npm test",
            "npm run build",
            "pnpm run test:unit",
            "yarn typecheck",
            "go test ./...",
            "pytest -x tests/",
            "python -m pytest",
            "uv run pytest -q",
            "npx vitest run",
            "npx tsc --noEmit",
            "make",
            "make check",
            "./gradlew test",
            "mvn verify",
            "cmake --build build",
            "RUST_LOG=debug cargo test",
            "timeout 600 cargo test",
            "cd crates/foo && cargo test",
            "cargo test 2>&1 | tail -20",
        ] {
            assert!(is_check_command(command), "{command}");
        }
    }

    #[test]
    fn other_commands_are_not_checks() {
        for command in [
            "ls -la",
            "git status",
            "cargo fmt",
            "cargo run --bin aura",
            "npm install",
            "npm run dev",
            "rg test src/",
            "cat tests/fixtures.rs",
            "echo cargo",
            "make install",
            "",
        ] {
            assert!(!is_check_command(command), "{command}");
        }
    }

    #[test]
    fn a_shell_running_a_check_is_a_check() {
        for command in [
            "sh .swe/run 'python -m pytest -q test/units/config'",
            "sh .swe/run \"go test ./lib/...\"",
            "sh .swe/run 'cd /app && npx jest src/foo.test.ts'",
            "bash -c 'cargo test -p parser'",
            "bash -lc \"npm test\"",
            "/bin/sh -c 'make check'",
        ] {
            assert!(is_check_command(command), "{command}");
        }
        for command in [
            "sh .swe/run 'ls -la'",
            "bash -c 'echo cargo test'",
            "bash deploy.sh",
            "sh",
        ] {
            assert!(!is_check_command(command), "{command}");
        }
    }

    #[test]
    fn a_check_handed_to_a_script_is_not_one_for_the_lead() {
        assert!(is_check_part(vec!["cargo", "test"]));
        assert!(!is_check_part(vec!["sh", "anything.sh", "cargo", "test"]));
        assert!(!is_check_part(vec!["bash", "-c", "cargo", "test"]));
    }

    #[test]
    fn a_check_run_through_the_task_container_decides() {
        let messages = [
            call(
                "1",
                json!({"shell_script": "sh .swe/run 'python -m pytest -q test/units'"}),
            ),
            result("1", r#"{"exit_code":1,"stdout":"1 failed"}"#, true),
        ];
        assert_eq!(last_check(&messages), Some(false));
    }

    #[test]
    fn a_failing_exit_a_later_command_may_have_set_goes_by_the_output() {
        let tests_then_grep = [
            call(
                "1",
                json!({"program": "sh", "args": ["-c", "sh .swe/run 'python -m pytest -q \
                    test/units/modules/test_mount_facts.py 2>&1 | tail -2; PYTHONPATH=lib python \
                    bin/ansible-doc -t module mount_facts 2>&1 | head -5; python -m pytest -q \
                    test/units/module_utils/facts/virtual/test_linux.py 2>&1 | grep -m2 Error'"]}),
            ),
            result(
                "1",
                r#"{"exit_code":1,"ok":false,"stderr":"exit_code: 1\nstdout:\n\n===== 12 passed in 0.46s =====\n[WARNING]: You are running the development version of Ansible.\n\nstderr:\n"}"#,
                true,
            ),
        ];
        let seen = last_check_seen(&tests_then_grep).unwrap();
        assert!(seen.passed && seen.by_output, "{seen:?}");

        let tests_then_missing_linters = [
            call(
                "1",
                json!({"program": "sh", "args": ["-c", "sh .swe/run 'python -m pytest -q \
                    test/units/modules/network/f5/test_bigip_message_routing_route.py 2>&1 | \
                    tail -20; python -m pycodestyle --max-line-length=160 \
                    lib/ansible/modules/network/f5/bigip_message_routing_route.py; python -m \
                    pyflakes lib/ansible/modules/network/f5/bigip_message_routing_route.py'"]}),
            ),
            result(
                "1",
                r#"{"exit_code":1,"ok":false,"stderr":"exit_code: 1\nstdout:\ncollected 7 items\n\ntest_bigip_message_routing_route.py .......  [100%]\n\n===== 7 passed in 0.52s =====\n/usr/local/bin/python: No module named pyflakes\n\nstderr:\n/usr/local/bin/python: No module named pycodestyle\n"}"#,
                true,
            ),
        ];
        assert_eq!(last_check(&tests_then_missing_linters), Some(true));

        let tests_then_linter = [
            call("1", json!({"command": "pytest -q && pyflakes src"})),
            result(
                "1",
                "exit_code: 127\nstdout:\n12 passed in 0.3s\nstderr:\nsh: pyflakes: not found",
                true,
            ),
        ];
        assert_eq!(last_check(&tests_then_linter), Some(true));

        let failures_piped_on = [
            call("1", json!({"command": "pytest -q 2>&1 | tail -3"})),
            result(
                "1",
                r#"{"exit_code":1,"stdout":"===== 19 failed, 251 passed in 1.55s ====="}"#,
                true,
            ),
        ];
        let seen = last_check_seen(&failures_piped_on).unwrap();
        assert!(!seen.passed && seen.by_output, "{seen:?}");

        let nothing_printed = [
            call("1", json!({"command": "cargo test"})),
            result("1", "test result: ok. 3 passed; 0 failed", false),
            call(
                "2",
                json!({"command": "cargo test 2>&1 | grep -c panicked"}),
            ),
            result("2", "exit_code: 1\nstdout:\n0", true),
        ];
        let seen = last_check_seen(&nothing_printed).unwrap();
        assert_eq!(seen.command, "cargo test", "the earlier check stands");

        let check_last = [
            call("1", json!({"command": "cd app && pytest -q"})),
            result("1", "exit_code: 1\nstdout:\n12 passed in 0.3s", true),
        ];
        let seen = last_check_seen(&check_last).unwrap();
        assert!(!seen.passed && !seen.by_output, "{seen:?}");
    }

    /// The checks of run a-2 of Compare #32 (Claude Haiku 5.5 on
    /// `ansible-b748edea`) with what they printed, as its attempt entry
    /// kept them. Each exited 0 through `tail`, and all three counted as
    /// passed.
    #[test]
    fn a_passing_exit_another_command_set_goes_by_the_output() {
        let ran = |command: &str, stdout: &str| {
            let output = json!({"exit_code": 0, "ok": true, "stderr": "", "stdout": stdout});
            last_check_seen(&[
                call("1", json!({"command": command})),
                result("1", &output.to_string(), false),
            ])
        };
        let broken_import = ran(
            "PYTHONPATH=lib python -c 'from ansible.module_utils.urls import prepare_multipart' ; \
             PYTHONPATH=lib python -m pytest -q test/units/module_utils/urls 2>&1 | tail -3",
            "/usr/bin/python: No module named pytest\n\nTraceback (most recent call last):\n  \
             File \"<string>\", line 2, in <module>\n    from ansible.module_utils.urls import \
             prepare_multipart\n  File \"/app/lib/ansible/module_utils/urls.py\", line 57, in \
             <module>\n    import ansible.module_utils.six.moves.http_cookiejar as cookiejar\n\
             ModuleNotFoundError: No module named 'ansible.module_utils.six.moves'\n",
        )
        .unwrap();
        assert!(
            !broken_import.passed && broken_import.by_output,
            "{broken_import:?}"
        );

        let missing_path = ran(
            "sh .swe/run printf 'hello file\\n' > /tmp/sample.txt && cd lib && python -c \
             \"from ansible.module_utils.urls import prepare_multipart\" ; python -m pytest -q \
             test/units/module_utils/urls 2>&1 | tail -3",
            "--===============2249448000726196501==--\r\n\n=============== no tests ran in \
             0.00s ===============\nERROR: file not found: test/units/module_utils/urls\n\n",
        )
        .unwrap();
        assert!(
            !missing_path.passed && missing_path.by_output,
            "{missing_path:?}"
        );

        let passing = ran(
            "sh .swe/run python -m pytest -q test/units/module_utils/urls 2>&1 | tail -3; cd lib \
             && python -c \"from ansible.module_utils.urls import prepare_multipart as p; ct, b \
             = p({'foo': 'bar'}); print(ct)\"",
            "\n-- Docs: https://docs.pytest.org/en/latest/warnings.html\n========== 64 passed, \
             1 warning in 0.34s ==========\nmultipart/form-data; \
             boundary=\"===============891502969508264522==\"\n",
        )
        .unwrap();
        assert!(passing.passed && passing.by_output, "{passing:?}");

        let built = ran(
            "cargo build 2>&1 | tail -1",
            "    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.10s\n",
        )
        .unwrap();
        assert!(built.passed && built.by_output, "{built:?}");
        assert_eq!(
            ran(
                "cargo test 2>&1 | tail -1",
                "warning: unused variable: `x`\n"
            ),
            None,
            "neither failures nor a pass"
        );
    }

    /// The last three test runs of a-4 of Compare #33 (Claude Sonnet 5.5
    /// on `ansible-d33bedc4`): the tests of the change pass, a wider
    /// suite fails, and the same suite fails with the change stashed.
    /// Gate counted the stashed run as the change's failing check.
    #[test]
    fn a_run_without_the_change_is_not_its_check_and_excuses_the_same_run_with_it() {
        const WIDE: &str = "python -m pytest -q -p no:cacheprovider test/units/plugins \
                            test/units/cli test/units/utils 2>&1";
        let ok = |stdout: &str| {
            json!({"exit_code": 0, "ok": true, "stderr": "", "stdout": stdout}).to_string()
        };
        let own = call(
            "1",
            json!({"program": "sh", "args": [".swe/run", "python tmp_check.py; python -m pytest \
                -q -p no:cacheprovider test/units/config/test_manager.py 2>&1 | tail -5"]}),
        );
        let own_passed = result("1", &ok("===== 116 passed in 1.02s =====\n"), false);
        let wide = call(
            "2",
            json!({"program": "sh", "args": [".swe/run", format!("{WIDE} | tail -5")]}),
        );
        let wide_failed = result(
            "2",
            &ok(
                "ERROR test/units/cli/test_galaxy.py::TestGalaxyInitSkeleton::test_role_dirs\n\
                 = 96 failed, 1539 passed, 2 skipped, 54 errors in 27.60s =\n",
            ),
            false,
        );
        let stashed = call(
            "3",
            json!({"program": "sh", "args": [".swe/run", format!(
                "git stash -q; {WIDE} | tail -1; git stash pop -q; git status --short"
            )]}),
        );
        let failed_before = |id: &str| {
            result(
                id,
                &ok("= 96 failed, 1539 passed, 2 skipped, 54 errors in 27.45s =\n M lib/x.py\n"),
                false,
            )
        };
        let messages = [
            own.clone(),
            own_passed.clone(),
            wide.clone(),
            wide_failed.clone(),
            stashed,
            failed_before("3"),
        ];
        let seen = checks_seen(&messages);
        assert_eq!(seen.len(), 1, "{seen:?}");
        assert!(seen[0].passed && seen[0].command.contains("test_manager.py"));
        assert_eq!(
            last_check(&messages[2..4]),
            Some(false),
            "nothing shows the failures were there before"
        );

        // The suite passes without the change: the change broke it.
        let passed_before = [
            wide.clone(),
            wide_failed.clone(),
            call(
                "3",
                json!({"command": format!("git stash; sh .swe/run {WIDE} | tail -1; git stash pop")}),
            ),
            result("3", &ok("===== 1689 passed in 27.45s =====\n"), false),
        ];
        assert_eq!(last_check(&passed_before), Some(false));

        // Stashed in one call, tested in the next, brought back in a third.
        let over_calls = [
            wide.clone(),
            wide_failed.clone(),
            call("3", json!({"command": "git stash -u"})),
            result("3", "Saved working directory and index state", false),
            call(
                "4",
                json!({"command": format!("sh .swe/run {WIDE} | tail -1")}),
            ),
            failed_before("4"),
            call("5", json!({"command": "git stash pop"})),
            result("5", "Dropped refs/stash@{0}", false),
            own.clone(),
            own_passed.clone(),
        ];
        let seen = checks_seen(&over_calls);
        assert_eq!(seen.len(), 1, "{seen:?}");
        assert!(seen[0].passed);

        // A stash that failed stashed nothing: the run after it is the
        // change's.
        let not_stashed = [
            call("1", json!({"command": "git stash -u"})),
            result(
                "1",
                "exit_code: 128\nstderr:\nfatal: not a git repository",
                true,
            ),
            wide,
            wide_failed,
        ];
        assert_eq!(last_check(&not_stashed), Some(false));
    }

    #[test]
    fn a_command_line_says_which_checks_ran_without_the_change() {
        let ran = Stashing::of(
            "sh .swe/run pytest -q a 2>&1 | tail -2; git stash -q; python -m pytest -q b | \
             grep passed; git stash pop -q; cargo test",
            false,
        );
        assert_eq!(ran.with, ["pytest -q a", "cargo test"]);
        assert_eq!(ran.without, ["pytest -q b"]);
        assert!(!ran.stashed);
        assert!(Stashing::of("git stash", false).stashed);
        assert!(Stashing::of("git stash push -u && pytest", false).stashed);
        assert!(!Stashing::of("git stash list", false).stashed);
        assert!(Stashing::of("git stash show -p", true).stashed);
        assert!(!Stashing::of("git stash apply", true).stashed);
        assert_eq!(Stashing::of("pytest -q", true).without, ["pytest -q"]);
    }

    #[test]
    fn a_stray_zero_width_character_does_not_hide_a_check() {
        let input = json!({"program": "sh", "args": [".swe/run", "python\u{200b} -m pytest -q \
            test/units/module_utils/urls/test_prepare_multipart.py 2>&1 | tail -60"]});
        assert_eq!(
            command_line(&input),
            "sh .swe/run python -m pytest -q \
             test/units/module_utils/urls/test_prepare_multipart.py 2>&1 | tail -60"
        );
        let messages = [
            call("1", input),
            result(
                "1",
                r#"{"exit_code":0,"ok":true,"stderr":"","stdout":"..............    [100%]\n14 passed in 0.31s\n"}"#,
                false,
            ),
        ];
        let seen = last_check_seen(&messages).unwrap();
        assert!(seen.passed && seen.by_output, "{seen:?}");
        assert_eq!(
            command_line(&json!({"shell_script": "\u{feff}cargo test\u{2060}"})),
            "cargo test"
        );
        for kept in ["echo 👩\u{200d}💻", "echo क्\u{200d}ष"] {
            assert_eq!(command_line(&json!({"command": kept})), kept);
        }
    }

    #[test]
    fn test_runner_summaries_report_passes() {
        for printed in [
            "===== 12 passed in 0.46s =====",
            "Tests:       3 passed, 3 total",
            "  5 passing (20ms)",
            "test result: ok. 3 passed; 0 failed; 0 ignored",
            "PASS\nok  \tgithub.com/acme/parser\t0.012s",
            "   Compiling parser v0.1.0\n    Finished `release` profile [optimized] target(s) in 4.2s",
        ] {
            assert!(reports_passes(printed), "{printed}");
        }
        for printed in [
            "0 passed",
            "all tests passed",
            "test_parse PASSED",
            "/usr/local/bin/python: No module named pyflakes",
            "Finished in 0.01 seconds (files took 0.1 seconds to load)",
            "",
        ] {
            assert!(!reports_passes(printed), "{printed}");
        }
    }

    fn call(id: &str, input: Value) -> Message {
        Message::new(
            aura_model::Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: id.to_string(),
                name: RUN_COMMAND.to_string(),
                input,
            }],
        )
    }

    fn result(id: &str, content: &str, is_error: bool) -> Message {
        Message::new(
            aura_model::Role::User,
            vec![ContentBlock::ToolResult {
                tool_use_id: id.to_string(),
                content: ToolResultContent::Text(content.to_string()),
                is_error,
            }],
        )
    }

    #[test]
    fn the_last_check_that_ran_decides() {
        let failed_then_passed = [
            call("1", json!({"shell_script": "cargo test"})),
            result("1", r#"{"exit_code":101,"ok":false,"stderr":"..."}"#, true),
            call("2", json!({"program": "cargo", "args": ["test"]})),
            result("2", "test result: ok", false),
        ];
        assert_eq!(last_check(&failed_then_passed), Some(true));

        let passed_then_failed = [
            call("1", json!({"command": "npm test"})),
            result("1", "ok", false),
            call("2", json!({"command": "npm run build"})),
            result("2", "exit_code: 1\nstderr:\nerror TS2304", true),
        ];
        assert_eq!(last_check(&passed_then_failed), Some(false));
        assert_eq!(
            last_check_seen(&passed_then_failed),
            Some(CheckSeen {
                command: "npm run build".to_string(),
                passed: false,
                by_output: false,
                output: "exit_code: 1\nstderr:\nerror TS2304".to_string(),
            })
        );
    }

    #[test]
    fn calls_that_did_not_run_to_the_end_say_nothing() {
        let messages = [
            call("1", json!({"command": "cargo test"})),
            result("1", "test result: ok", false),
            call("2", json!({"command": "cargo test"})),
            result("2", "denied: run_command needs approval", true),
            call("3", json!({"command": "cargo test", "background": true})),
            result("3", "started in the background", false),
            call("4", json!({"command": "ls"})),
            result("4", "exit_code: 2", true),
            call("5", json!({"command": "cargo test"})),
            result("5", r#"{"ok":false,"stderr":"command not allowed"}"#, true),
        ];
        assert_eq!(last_check(&messages), Some(true));
        assert_eq!(last_check(&messages[2..]), None);
        assert_eq!(last_check(&[]), None);
    }
}
