//! Reading a failed test check's output: which tests failed, and the
//! part of the output that shows why (each test's `---- <name> stdout
//! ----` block: its assertion, or its snapshot diff).
//!
//! Pure functions of the text, for libtest's output (`cargo test`);
//! other output gives no tests and the caller falls back to its tail.

/// Most tests a failure lists.
const MAX_TESTS: usize = 20;
/// Most characters of excerpt kept.
const MAX_EXCERPT_CHARS: usize = 8_000;

/// The tests `output` says failed (`test <name> ... FAILED`), in order,
/// each once.
#[must_use]
pub fn failed_tests(output: &str) -> Vec<String> {
    let mut tests: Vec<String> = Vec::new();
    for line in output.lines() {
        let Some(rest) = line.trim().strip_prefix("test ") else {
            continue;
        };
        let Some(name) = rest.strip_suffix(" ... FAILED") else {
            continue;
        };
        let name = name.trim();
        if !name.is_empty() && !name.contains(' ') && !tests.iter().any(|t| t == name) {
            tests.push(name.to_string());
            if tests.len() == MAX_TESTS {
                break;
            }
        }
    }
    tests
}

/// The `---- <test> stdout ----` blocks of `output` for `tests`, joined:
/// what each failing test printed, up to the next block or the list of
/// failures. `None` when there are none.
#[must_use]
pub fn test_excerpt(output: &str, tests: &[String]) -> Option<String> {
    let mut excerpt = String::new();
    let mut inside = false;
    for line in output.lines() {
        let trimmed = line.trim();
        if let Some(name) = trimmed
            .strip_prefix("---- ")
            .and_then(|rest| rest.strip_suffix(" stdout ----"))
        {
            inside = tests.iter().any(|test| test == name);
            if inside {
                excerpt.push_str(line);
                excerpt.push('\n');
            }
            continue;
        }
        if trimmed == "failures:" || trimmed.starts_with("test result:") {
            inside = false;
        }
        if inside {
            excerpt.push_str(line);
            excerpt.push('\n');
        }
    }
    if excerpt.trim().is_empty() {
        return None;
    }
    let chars = excerpt.chars().count();
    Some(if chars > MAX_EXCERPT_CHARS {
        excerpt.chars().take(MAX_EXCERPT_CHARS).collect()
    } else {
        excerpt
    })
}

/// The command that runs only `tests` again, for a `cargo test` command;
/// `None` for any other command, which is run again whole.
#[must_use]
pub fn rerun_command(command: &str, tests: &[String]) -> Option<String> {
    let words: Vec<&str> = command.split_whitespace().collect();
    let is_cargo = |word: &str| word.rsplit('/').next() == Some("cargo");
    let is_cargo_test = words
        .windows(2)
        .any(|pair| is_cargo(pair[0]) && pair[1] == "test");
    let plain = !command.contains(['&', '|', ';', '>', '<', '`', '$']);
    if !is_cargo_test || !plain || tests.is_empty() {
        return None;
    }
    let safe = tests.iter().all(|test| {
        test.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_:-.".contains(&b))
    });
    if !safe {
        return None;
    }
    let names = tests.join(" ");
    Some(if words.contains(&"--") {
        format!("{command} --exact {names}")
    } else {
        format!("{command} -- --exact {names}")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What `cargo test --workspace` printed when the CLI's help snapshot
    /// went stale on main (`8a1be374`), cut down.
    const STALE_SNAPSHOT: &str = "\
     Running tests/cli_golden.rs (target/debug/deps/cli_golden-1)

running 3 tests
test cli_golden_login ... ok
test cli_golden_top_level_help ... FAILED
test cli_golden_logout ... ok

failures:

---- cli_golden_top_level_help stdout ----
-old snapshot
+new results
────────────┬───────────────────────────────
    5     5 │   run        Run a task
          6 │+  changelog  Assemble the changelog fragments
────────────┴───────────────────────────────

thread 'cli_golden_top_level_help' panicked at tests/cli_golden.rs:40:5:
snapshot assertion for 'top_level_help' failed in line 40

failures:
    cli_golden_top_level_help

test result: FAILED. 2 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out
";

    #[test]
    fn the_failing_tests_and_their_diff_are_read_from_cargo_output() {
        let tests = failed_tests(STALE_SNAPSHOT);
        assert_eq!(tests, vec!["cli_golden_top_level_help".to_string()]);
        let excerpt = test_excerpt(STALE_SNAPSHOT, &tests).unwrap_or_default();
        assert!(
            excerpt.starts_with("---- cli_golden_top_level_help stdout ----"),
            "{excerpt}"
        );
        assert!(excerpt.contains("+  changelog"), "{excerpt}");
        assert!(excerpt.contains("snapshot assertion"), "{excerpt}");
        assert!(!excerpt.contains("test result"), "{excerpt}");
    }

    #[test]
    fn output_without_libtest_lines_gives_no_tests() {
        assert!(failed_tests("npm ERR! Test failed.").is_empty());
        assert_eq!(test_excerpt("nothing here", &[]), None);
    }

    #[test]
    fn only_cargo_test_commands_rerun_just_the_failing_tests() {
        let tests = vec!["a::b".to_string(), "c".to_string()];
        assert_eq!(
            rerun_command("cargo test --workspace", &tests).as_deref(),
            Some("cargo test --workspace -- --exact a::b c")
        );
        assert_eq!(
            rerun_command("cargo test -p x -- --test-threads 2", &tests).as_deref(),
            Some("cargo test -p x -- --test-threads 2 --exact a::b c")
        );
        assert_eq!(
            rerun_command("./ci/cargo test", &tests).as_deref(),
            Some("./ci/cargo test -- --exact a::b c")
        );
        assert_eq!(rerun_command("npm test", &tests), None);
        assert_eq!(rerun_command("cargo test && rm -rf /", &tests), None);
        assert_eq!(rerun_command("cargo test", &["x; rm".to_string()]), None);
    }
}
