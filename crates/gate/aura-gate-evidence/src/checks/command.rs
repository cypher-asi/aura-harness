//! Which commands of a command line run a check, and what runs after the
//! last one.

/// What joins one simple command of a command line to the next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Joint {
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

/// Whether `command` runs tests, a build or a type check in any of its
/// `&&`/`;`/`|` parts, counting a shell that runs the check for it:
/// `bash -c '<check>'`, or a script handed the check, as in `sh .swe/run
/// '<check>'` (which runs it in the task's container).
pub(super) fn is_check_command(command: &str) -> bool {
    command
        .split(['&', ';', '|', '\n'])
        .any(|part| is_check_words(&words(part), true))
}

/// Whether the exit status of `command` is not that of the last check it
/// runs, so that its passing says nothing of the check: the check's
/// output is piped on (`pytest | tail -20` exits as `tail` does, unless
/// `pipefail` is set), or another command runs after it whatever it did
/// (`pytest; echo done`, `pytest || true`). `false` for a command that
/// runs no check.
pub(crate) fn exit_status_masked(command: &str) -> bool {
    let parts = parts(command);
    let Some(last) = last_check_part(&parts) else {
        return false;
    };
    match parts[last].1 {
        Joint::Pipe => !command.contains("pipefail"),
        Joint::Or | Joint::Then => runs_after(&parts, last),
        Joint::And | Joint::End => false,
    }
}

/// Whether another command runs after the last check in `command`,
/// joined to it in any way (`|`, `&&`, `||`, `;`), so that a failing exit
/// status may be that command's rather than the check's: `pytest | grep
/// Error` exits 1 when `grep` finds nothing, and `pytest && pyflakes x`
/// when `pyflakes` is not installed. `false` for a command that runs no
/// check.
pub(super) fn runs_after_check(command: &str) -> bool {
    let parts = parts(command);
    last_check_part(&parts).is_some_and(|last| runs_after(&parts, last))
}

/// The place of the last of `parts` that is a check.
fn last_check_part(parts: &[(&str, Joint)]) -> Option<usize> {
    parts
        .iter()
        .rposition(|(part, _)| is_check_words(&words(part), true))
}

/// Whether a command follows the one of `parts` at `at`.
fn runs_after(parts: &[(&str, Joint)], at: usize) -> bool {
    parts[at + 1..]
        .iter()
        .any(|(part, _)| !part.trim().is_empty())
}

/// The simple commands of `command`, each with what joins it to the
/// next. Quotes are not followed, as in [`is_check_command`]: a wrapper
/// such as `sh .swe/run '<check> | tail'` is read as its script. A `&`
/// that is part of a redirection (`2>&1`, `&>`) joins nothing.
pub(super) fn parts(command: &str) -> Vec<(&str, Joint)> {
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

/// The words of a simple command, without the quotes around them.
pub(super) fn words(part: &str) -> Vec<&str> {
    part.split_whitespace()
        .map(|word| word.trim_matches(['\'', '"']))
        .filter(|word| !word.is_empty())
        .collect()
}

/// Whether one simple command (its words) is a check. Leading variable
/// assignments and wrappers (`env`, `time`, `timeout 600`, `npx`, ...)
/// are skipped. A shell handed a check is not one: the script it runs
/// may do anything.
#[must_use]
pub fn is_check_part(words: &[&str]) -> bool {
    is_check_words(words, false)
}

/// [`is_check_part`], looking through a shell that runs a check when
/// `through_shells`.
fn is_check_words(words: &[&str], through_shells: bool) -> bool {
    runs_check(simple_command(words, through_shells))
}

/// The command `words` come down to: without leading variable
/// assignments and wrappers (`env`, `time`, `timeout 600`, `npx`,
/// `python -m`, ...), and, when `through_shells`, what a shell is handed
/// to run in place of the shell.
pub(super) fn simple_command<'w, 'a>(
    mut words: &'w [&'a str],
    through_shells: bool,
) -> &'w [&'a str] {
    loop {
        words = match words {
            [word, rest @ ..] if word.contains('=') && !word.starts_with('-') => rest,
            ["env" | "time" | "nice" | "npx" | "bunx" | "exec" | "sudo", rest @ ..] => rest,
            ["timeout", rest @ ..] => rest.get(1..).unwrap_or_default(),
            ["uv" | "poetry" | "pipenv" | "pdm", "run", rest @ ..]
            | ["python" | "python3", "-m", rest @ ..] => rest,
            [program, rest @ ..]
                if through_shells && matches!(name(program), "sh" | "bash" | "zsh" | "dash") =>
            {
                shell_command(rest)
            }
            _ => return words,
        };
    }
}

/// The name of the program at `path`.
fn name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Whether `words`, a simple command as [`simple_command`] leaves it,
/// runs tests, a build or a type check.
pub(super) fn runs_check(words: &[&str]) -> bool {
    let Some(&program) = words.first() else {
        return false;
    };
    let sub = words.get(1).copied().unwrap_or_default();
    let script = if sub == "run" {
        words.get(2).copied().unwrap_or_default()
    } else {
        sub
    };
    match name(program) {
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
fn shell_command<'w, 'a>(args: &'w [&'a str]) -> &'w [&'a str] {
    let is_flag = |word: &str| word.starts_with('-') && !word.starts_with("--");
    if let Some(at) = args
        .iter()
        .position(|word| is_flag(word) && word.contains('c'))
    {
        return &args[at + 1..];
    }
    let flags = args.iter().take_while(|word| word.starts_with('-')).count();
    args.get(flags + 1..).unwrap_or_default()
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
    fn test_and_build_commands_are_checks() {
        for command in [
            "cargo test -p aura-gate",
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
            "timeout",
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
        assert!(is_check_part(&["cargo", "test"]));
        assert!(!is_check_part(&["sh", "anything.sh", "cargo", "test"]));
        assert!(!is_check_part(&["bash", "-c", "cargo", "test"]));
    }
}
