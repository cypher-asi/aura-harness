//! Whether the tests or build a turn ran last passed: the strongest
//! automatic signal of how a turn went.
//!
//! A check is a `run_command` call whose command line runs a test suite,
//! a build or a type check (see [`is_check_command`]). Its result counts
//! only when the command ran to its end: a passing call, or a failing one
//! that reports its exit code. Calls the policy denied, that could not
//! start, or that went to the background say nothing.

use aura_model_reasoner::{ContentBlock, Message, ToolResultContent};
use serde_json::Value;

/// Name of the tool that runs commands.
const RUN_COMMAND: &str = "run_command";

/// Field (in a JSON result) or line prefix (in a text one) with the exit
/// code of a failed `run_command` call that ran to its end.
const EXIT_CODE: &str = "exit_code";

/// Whether the last check in `messages` passed; `None` when they hold no
/// check that ran to its end.
#[must_use]
pub fn last_check(messages: &[Message]) -> Option<bool> {
    let mut checks: Vec<(&str, Option<bool>)> = Vec::new();
    for message in messages {
        for block in &message.content {
            match block {
                ContentBlock::ToolUse { id, name, input } if name == RUN_COMMAND => {
                    if input["background"].as_bool() != Some(true)
                        && is_check_command(&command_line(input))
                    {
                        checks.push((id.as_str(), None));
                    }
                }
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } => {
                    if let Some(check) = checks.iter_mut().find(|(id, _)| id == tool_use_id) {
                        check.1 = verdict(content, *is_error);
                    }
                }
                _ => {}
            }
        }
    }
    checks.into_iter().rev().find_map(|(_, passed)| passed)
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
/// `&&`/`;`/`|` parts.
#[must_use]
pub fn is_check_command(command: &str) -> bool {
    command
        .split(['&', ';', '|', '\n'])
        .any(|part| is_check_part(part.split_whitespace().collect()))
}

/// Whether one simple command (its words) is a check. Leading variable
/// assignments and wrappers (`env`, `time`, `timeout 600`, `npx`, ...)
/// are skipped.
pub(crate) fn is_check_part(mut words: Vec<&str>) -> bool {
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

    fn call(id: &str, input: Value) -> Message {
        Message::new(
            aura_model_reasoner::Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: id.to_string(),
                name: RUN_COMMAND.to_string(),
                input,
            }],
        )
    }

    fn result(id: &str, content: &str, is_error: bool) -> Message {
        Message::new(
            aura_model_reasoner::Role::User,
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
