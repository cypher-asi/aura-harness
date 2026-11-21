//! Whether the tests or build a turn ran last passed: the strongest
//! automatic signal of how a turn went.
//!
//! A check is a `run_command` call whose command line runs a test suite,
//! a build or a type check (see [`is_check_command`]). Its result counts
//! only when the command ran to its end: a passing call, or a failing one
//! that reports its exit code. Calls the policy denied, that could not
//! start, or that went to the background say nothing.

use aura_model::{ContentBlock, Message, ToolResultContent};
use serde_json::Value;

/// Name of the tool that runs commands.
const RUN_COMMAND: &str = "run_command";

/// Field (in a JSON result) or line prefix (in a text one) with the exit
/// code of a failed `run_command` call that ran to its end.
const EXIT_CODE: &str = "exit_code";

/// A check that ran to its end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckSeen {
    /// The command line it ran.
    pub command: String,
    /// Whether it passed.
    pub passed: bool,
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
    // Each check's call id, its command and, once it ran to its end,
    // whether it passed and what it printed.
    type Ran = Option<(bool, String)>;
    let mut checks: Vec<(&str, String, Ran)> = Vec::new();
    for message in messages {
        for block in &message.content {
            match block {
                ContentBlock::ToolUse { id, name, input } if name == RUN_COMMAND => {
                    let command = command_line(input);
                    if input["background"].as_bool() != Some(true) && is_check_command(&command) {
                        checks.push((id.as_str(), command, None));
                    }
                }
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } => {
                    if let Some(check) = checks.iter_mut().find(|(id, ..)| id == tool_use_id) {
                        check.2 = verdict(content, *is_error).map(|passed| {
                            let output = match content {
                                ToolResultContent::Text(text) => text.clone(),
                                ToolResultContent::Json(value) => value.to_string(),
                            };
                            (passed, output)
                        });
                    }
                }
                _ => {}
            }
        }
    }
    checks
        .into_iter()
        .filter_map(|(_, command, ran)| {
            ran.map(|(passed, output)| CheckSeen {
                command,
                passed,
                output,
            })
        })
        .collect()
}

/// Pass or fail from a check's result; `None` when the command did not
/// run to its end.
fn verdict(content: &ToolResultContent, is_error: bool) -> Option<bool> {
    if !is_error {
        return Some(true);
    }
    let exited = match content {
        ToolResultContent::Json(value) => has_exit_code(value),
        ToolResultContent::Text(text) => serde_json::from_str::<Value>(text).map_or_else(
            |_| {
                text.trim_start()
                    .strip_prefix(EXIT_CODE)
                    .is_some_and(|rest| rest.starts_with(':'))
            },
            |value| has_exit_code(&value),
        ),
    };
    exited.then_some(false)
}

fn has_exit_code(result: &Value) -> bool {
    result.get(EXIT_CODE).is_some_and(Value::is_i64)
}

/// The command line a `run_command` call runs: its script, or its program
/// and arguments.
#[must_use]
pub fn command_line(input: &Value) -> String {
    if let Some(script) = ["shell_script", "command"]
        .iter()
        .find_map(|key| input[*key].as_str())
    {
        return script.to_string();
    }
    let mut words = vec![input["program"].as_str().unwrap_or_default()];
    if let Some(args) = input["args"].as_array() {
        words.extend(args.iter().filter_map(Value::as_str));
    }
    words.join(" ")
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

/// Whether one simple command (its words) is a check. Leading variable
/// assignments and wrappers (`env`, `time`, `timeout 600`, `npx`, ...)
/// are skipped. A shell handed a check is not one: the script it runs
/// may do anything.
pub(crate) fn is_check_part(words: Vec<&str>) -> bool {
    is_check_words(words, false)
}

/// [`is_check_part`], looking through a shell that runs a check when
/// `through_shells`.
fn is_check_words(mut words: Vec<&str>, through_shells: bool) -> bool {
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
            _ => break,
        }
    }
    let Some(&program) = words.first() else {
        return false;
    };
    let program = program.rsplit('/').next().unwrap_or(program);
    if through_shells && matches!(program, "sh" | "bash" | "zsh" | "dash") {
        return is_check_words(shell_command(&words[1..]), true);
    }
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
            result("c", "fine", false),
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
