use super::*;

#[test]
fn parse_cargo_test_output() {
    let stdout = "\
running 3 tests
test utils::tests::test_parse ... ok
test utils::tests::test_format ... FAILED
test utils::tests::test_skip ... ignored
";
    let (results, summary) = parse_test_output(stdout, "", true);
    assert_eq!(results.len(), 3);
    assert_eq!(results[0].status, "passed");
    assert_eq!(results[1].status, "failed");
    assert_eq!(results[2].status, "skipped");
    assert!(summary.contains("1 passed"));
    assert!(summary.contains("1 failed"));
    assert!(summary.contains("1 skipped"));
}

#[test]
fn parse_jest_pass_fail() {
    let stdout = "\
PASS src/utils.test.ts
FAIL src/api.test.ts
PASS src/hooks.test.ts
";
    let (results, summary) = parse_test_output(stdout, "", true);
    assert_eq!(results.len(), 3);
    assert_eq!(results.iter().filter(|r| r.status == "passed").count(), 2);
    assert_eq!(results.iter().filter(|r| r.status == "failed").count(), 1);
    assert!(summary.contains("2 passed"));
}

#[test]
fn parse_fallback_success() {
    let (results, summary) = parse_test_output("all ok", "", true);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].status, "passed");
    assert!(summary.contains("all tests passed"));
}

#[test]
fn parse_fallback_failure() {
    let (results, summary) = parse_test_output("boom", "something went wrong", false);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].status, "failed");
    assert!(results[0].message.is_some());
    assert!(summary.contains("tests failed"));
}

#[test]
fn truncate_short_output_unchanged() {
    assert_eq!(truncate_output("hello", 100), "hello");
}

#[test]
fn truncate_long_output() {
    let long = "a".repeat(200);
    let result = truncate_output(&long, 50);
    assert!(result.len() < 200);
    assert!(result.contains("truncated"));
}

#[test]
fn truncate_output_handles_multibyte_boundaries() {
    let long = "é".repeat(100);
    let result = truncate_output(&long, 51);
    assert!(result.contains("truncated"));
}

#[test]
fn needs_shell_with_pipe() {
    assert!(needs_shell("cargo test | head"));
}

#[test]
fn needs_shell_with_and() {
    assert!(needs_shell("cd foo && npm build"));
}

#[test]
fn needs_shell_simple_command() {
    assert!(!needs_shell("cargo build --release"));
}

#[test]
fn parse_pytest_output_short_form() {
    let stdout = "\
============================= test session starts ==============================
collected 4 items

tests/test_a.py::test_one PASSED
tests/test_a.py::test_two FAILED
tests/test_b.py::TestX::test_three PASSED
tests/test_c.py::test_four SKIPPED

=========================== short test summary info ============================
FAILED tests/test_a.py::test_two - AssertionError: expected 1 got 2
==================== 2 passed, 1 failed, 1 skipped in 0.12s ====================
";
    let (results, summary) = parse_test_output(stdout, "", false);
    let failed: Vec<_> = results.iter().filter(|r| r.status == "failed").collect();
    let passed: Vec<_> = results.iter().filter(|r| r.status == "passed").collect();
    let skipped: Vec<_> = results.iter().filter(|r| r.status == "skipped").collect();
    assert_eq!(passed.len(), 2);
    assert_eq!(failed.len(), 1, "summary block must not double-count");
    assert_eq!(skipped.len(), 1);
    assert_eq!(failed[0].name, "tests/test_a.py::test_two");
    assert!(summary.contains("1 failed"));
}

#[test]
fn parse_go_test_output_pass_fail_skip() {
    let stdout = "\
=== RUN   TestAlpha
--- PASS: TestAlpha (0.01s)
=== RUN   TestBeta
--- FAIL: TestBeta (0.02s)
    foo_test.go:42: expected 5, got 4
=== RUN   TestGamma
--- SKIP: TestGamma (0.00s)
FAIL\tgithub.com/example/broken [build failed]
ok  \tgithub.com/example/ok\t0.123s
";
    let (results, summary) = parse_test_output(stdout, "", false);
    let names_failed: Vec<_> = results
        .iter()
        .filter(|r| r.status == "failed")
        .map(|r| r.name.clone())
        .collect();
    assert!(names_failed.iter().any(|n| n == "TestBeta"));
    assert!(
        names_failed
            .iter()
            .any(|n| n.contains("github.com/example/broken")),
        "package-level FAIL line should be captured: got {names_failed:?}"
    );
    assert!(results
        .iter()
        .any(|r| r.name == "TestAlpha" && r.status == "passed"));
    assert!(results
        .iter()
        .any(|r| r.name == "TestGamma" && r.status == "skipped"));
    assert!(summary.contains("failed"));
}

#[test]
fn parse_rspec_output_with_failures() {
    let stdout = "\
Failures:

  1) UserModel#full_name returns first and last name
     Failure/Error: expect(user.full_name).to eq('Jane Doe')

  2) UserModel#email validates format
     Failure/Error: expect(user.errors).to be_empty

Finished in 0.234 seconds
12 examples, 2 failures, 1 pending
";
    let (results, summary) = parse_test_output(stdout, "", false);
    let failed: Vec<_> = results.iter().filter(|r| r.status == "failed").collect();
    let passed: Vec<_> = results.iter().filter(|r| r.status == "passed").collect();
    let skipped: Vec<_> = results.iter().filter(|r| r.status == "skipped").collect();
    assert_eq!(failed.len(), 2);
    assert_eq!(passed.len(), 12 - 2 - 1, "passes derived from summary line");
    assert_eq!(skipped.len(), 1);
    assert!(summary.contains("2 failed"));
}

#[test]
fn parse_rspec_output_all_green() {
    let stdout = "\
Finished in 0.05 seconds
4 examples, 0 failures
";
    let (results, summary) = parse_test_output(stdout, "", true);
    assert_eq!(results.len(), 4);
    assert!(results.iter().all(|r| r.status == "passed"));
    assert!(summary.contains("4 passed"));
}

#[test]
fn parse_unknown_format_falls_back_to_exit_code() {
    let (results, _summary) = parse_test_output(
        "running tests with custom-runner v0.3 ...\nall green\n",
        "",
        true,
    );
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].name, "(aggregate)");
    assert_eq!(results[0].status, "passed");

    let (results, _summary) = parse_test_output("custom-runner: failure!", "trace...", false);
    assert_eq!(results[0].status, "failed");
}

#[tokio::test]
async fn run_build_command_rejects_empty_command() {
    let err = run_build_command(std::path::Path::new("."), "  ", None, None)
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("build_command is empty"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn run_build_command_reports_spawn_failure_with_context() {
    let dir = tempfile::tempdir().unwrap();
    let command = "__aura_missing_command_for_runner_test__";

    let err = run_build_command(dir.path(), command, None, None)
        .await
        .unwrap_err();
    let msg = err.to_string();

    assert!(msg.contains(command), "missing command context: {msg}");
    assert!(
        msg.contains(&dir.path().display().to_string()),
        "missing directory context: {msg}"
    );
}

/// The variables `command` sets, by name.
fn envs_of(command: &Command) -> std::collections::HashMap<String, Option<std::ffi::OsString>> {
    command
        .as_std()
        .get_envs()
        .map(|(key, value)| {
            (
                key.to_string_lossy().into_owned(),
                value.map(std::ffi::OsStr::to_os_string),
            )
        })
        .collect()
}

#[test]
fn build_commands_with_a_scratch_get_its_variables_whether_shelled_or_not() {
    let dir = tempfile::tempdir().unwrap();
    let scratch = RunScratch::new(dir.path().join("a-1.tmp"));
    for build_command in ["cargo check --workspace", "cargo fmt --check && cargo test"] {
        let command = build_child_command(dir.path(), build_command, Some(&scratch));
        let envs = envs_of(&command);
        for (key, value) in scratch.vars() {
            assert_eq!(
                envs.get(key),
                Some(&Some(value.into_os_string())),
                "{build_command}: {key}"
            );
        }
        assert_eq!(command.as_std().get_current_dir(), Some(dir.path()));
    }
}

#[test]
fn build_commands_without_a_scratch_keep_the_harness_environment() {
    let dir = tempfile::tempdir().unwrap();
    for build_command in ["cargo check", "make && make test"] {
        let command = build_child_command(dir.path(), build_command, None);
        assert!(envs_of(&command).is_empty(), "{build_command}");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn a_build_run_with_a_scratch_sees_its_folders() {
    let dir = tempfile::tempdir().unwrap();
    let scratch = RunScratch::new(dir.path().join("a-1.tmp"));
    let result = run_build_command(
        dir.path(),
        "echo \"$TMPDIR|$CARGO_TARGET_DIR\"",
        None,
        Some(&scratch),
    )
    .await
    .unwrap();
    assert!(result.success, "{result:?}");
    let expected = format!(
        "{}|{}",
        scratch.dir().join("tmp").display(),
        scratch.dir().join("target").display()
    );
    assert_eq!(result.stdout.trim(), expected);
}

#[test]
fn parse_pytest_does_not_eat_unrelated_failed_lines() {
    let stdout = "\
some build noise about FAILED to load shared lib
no test markers here
";
    let (results, _summary) = parse_test_output(stdout, "", true);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].name, "(aggregate)");
}

/// `windows_resolve_program` must keep its hands off any input that
/// already looks like a path or carries an explicit executable
/// extension. Returning `None` here is the contract that lets
/// `resolve_program` fall back to `Command::new` with
/// the operator's original string, which is critical so we don't
/// silently rewrite (for example) a project-local `./build.cmd`
/// into a `.cmd` that happens to share a name on PATH.
#[cfg(target_os = "windows")]
#[test]
fn windows_resolve_program_skips_paths_and_explicit_extensions() {
    use std::path::Path;

    let cwd = Path::new(".");
    // Anything containing a path separator → caller intent.
    assert!(windows_resolve_program(r"tools\foo.exe", cwd).is_none());
    assert!(windows_resolve_program("./build.sh", cwd).is_none());
    assert!(windows_resolve_program(r"C:\Windows\System32\where.exe", cwd).is_none());

    // Explicit extension → let `Command::new` do its own PATH search,
    // which already works for `.exe` and tolerates `.cmd`/`.bat`
    // when the operator typed them out.
    assert!(windows_resolve_program("npm.cmd", cwd).is_none());
    assert!(windows_resolve_program("BUILD.BAT", cwd).is_none());
    assert!(windows_resolve_program("python.exe", cwd).is_none());
}

/// Smoke test that `which::which_in` actually finds a real Windows
/// shim when given a bare name. We probe `cmd` (which is always on
/// PATH as `cmd.exe`) so the test is robust on stripped-down CI
/// images that don't ship npm/node. The point is to assert *that*
/// resolution happens, not which extension wins; on a host without
/// `cmd` on PATH we skip rather than fail so this stays portable.
#[cfg(target_os = "windows")]
#[test]
fn windows_resolve_program_finds_bare_name_via_pathext() {
    use std::path::Path;

    let cwd = Path::new(".");
    let Some(resolved) = windows_resolve_program("cmd", cwd) else {
        // `cmd` somehow not on PATH on this host. Nothing to assert
        // about behaviour we don't control; treat as a non-failure
        // skip so the test suite stays green in restrictive CI
        // sandboxes (e.g. WSL crates run on Linux containers).
        return;
    };
    assert!(
        resolved.is_absolute(),
        "expected an absolute resolved path, got {}",
        resolved.display()
    );
    let lower = resolved
        .file_name()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();
    assert!(
        lower.starts_with("cmd."),
        "expected resolved file to start with 'cmd.', got {lower}"
    );
}
