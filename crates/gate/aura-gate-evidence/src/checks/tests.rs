//! The checks read out of whole turns: which ran to their end, which ran
//! without the change, and whether each passed.

use aura_model::Message;
use serde_json::json;

use super::*;
use crate::test_support::{ran, result, run};

#[test]
fn every_check_that_ran_is_seen_in_order() {
    let messages = [
        ran("a", "cargo test", "exit_code: 1\nboom", true),
        ran("b", "ls", "src", false),
        ran(
            "c",
            "cargo test | tail -3",
            "test result: ok. 3 passed; 0 failed",
            false,
        ),
        [
            run("d", json!({"command": "cargo test", "background": true})),
            result("d", "started", false),
        ],
    ]
    .concat();
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
fn a_check_run_through_the_task_container_decides() {
    let messages = [
        run(
            "1",
            json!({"shell_script": "sh .swe/run 'python -m pytest -q test/units'"}),
        ),
        result("1", r#"{"exit_code":1,"stdout":"1 failed"}"#, true),
    ];
    assert_eq!(last_check(&messages), Some(false));
}

#[test]
fn a_failing_exit_a_command_after_the_tests_may_have_set_goes_by_the_output() {
    let tests_then_grep = [
        run(
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
        run(
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

    let tests_then_linter = ran(
        "1",
        "pytest -q && pyflakes src",
        "exit_code: 127\nstdout:\n12 passed in 0.3s\nstderr:\nsh: pyflakes: not found",
        true,
    );
    assert_eq!(last_check(&tests_then_linter), Some(true));
}

#[test]
fn what_a_failing_check_printed_decides_only_when_a_command_ran_after_it() {
    let failures_piped_on = ran(
        "1",
        "pytest -q 2>&1 | tail -3",
        r#"{"exit_code":1,"stdout":"===== 19 failed, 251 passed in 1.55s ====="}"#,
        true,
    );
    let seen = last_check_seen(&failures_piped_on).unwrap();
    assert!(!seen.passed && seen.by_output, "{seen:?}");

    let nothing_printed = [
        ran(
            "1",
            "cargo test",
            "test result: ok. 3 passed; 0 failed",
            false,
        ),
        ran(
            "2",
            "cargo test 2>&1 | grep -c panicked",
            "exit_code: 1\nstdout:\n0",
            true,
        ),
    ]
    .concat();
    let seen = last_check_seen(&nothing_printed).unwrap();
    assert_eq!(seen.command, "cargo test", "the earlier check stands");

    let check_last = ran(
        "1",
        "cd app && pytest -q",
        "exit_code: 1\nstdout:\n12 passed in 0.3s",
        true,
    );
    let seen = last_check_seen(&check_last).unwrap();
    assert!(!seen.passed && !seen.by_output, "{seen:?}");
}

/// The last check of a turn that ran `command`, which exited 0 having
/// printed `stdout`.
fn passed_printing(command: &str, stdout: &str) -> Option<CheckSeen> {
    let output = json!({"exit_code": 0, "ok": true, "stderr": "", "stdout": stdout});
    last_check_seen(&[
        run("1", json!({"command": command})),
        result("1", &output.to_string(), false),
    ])
}

/// Two checks of run a-2 of Compare #32 (Claude Haiku 5.5 on
/// `ansible-b748edea`) with what they printed, as its attempt entry kept
/// them. Each exited 0 through `tail`, and both counted as passed.
#[test]
fn a_passing_exit_another_command_set_fails_on_what_the_check_printed() {
    let broken_import = passed_printing(
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

    let missing_path = passed_printing(
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
}

/// The third check of run a-2 of Compare #32, which passed, and others
/// whose output decides as much.
#[test]
fn a_passing_exit_another_command_set_passes_on_what_the_check_printed() {
    let passing = passed_printing(
        "sh .swe/run python -m pytest -q test/units/module_utils/urls 2>&1 | tail -3; cd lib \
         && python -c \"from ansible.module_utils.urls import prepare_multipart as p; ct, b \
         = p({'foo': 'bar'}); print(ct)\"",
        "\n-- Docs: https://docs.pytest.org/en/latest/warnings.html\n========== 64 passed, \
         1 warning in 0.34s ==========\nmultipart/form-data; \
         boundary=\"===============891502969508264522==\"\n",
    )
    .unwrap();
    assert!(passing.passed && passing.by_output, "{passing:?}");

    let built = passed_printing(
        "cargo build 2>&1 | tail -1",
        "    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.10s\n",
    )
    .unwrap();
    assert!(built.passed && built.by_output, "{built:?}");
    assert_eq!(
        passed_printing(
            "cargo test 2>&1 | tail -1",
            "warning: unused variable: `x`\n"
        ),
        None,
        "neither failures nor a pass"
    );
}

/// A wide suite of run a-4 of Compare #33 (Claude Sonnet 5.5 on
/// `ansible-d33bedc4`), the change in place or stashed away.
const WIDE: &str = "python -m pytest -q -p no:cacheprovider test/units/plugins \
                    test/units/cli test/units/utils 2>&1";

/// A `run_command` result that exited 0 having printed `stdout`.
fn exited_ok(stdout: &str) -> String {
    json!({"exit_code": 0, "ok": true, "stderr": "", "stdout": stdout}).to_string()
}

/// The tests of a-4's change, which pass.
fn own_tests_passing() -> [Message; 2] {
    [
        run(
            "1",
            json!({"program": "sh", "args": [".swe/run", "python tmp_check.py; python -m pytest \
                -q -p no:cacheprovider test/units/config/test_manager.py 2>&1 | tail -5"]}),
        ),
        result("1", &exited_ok("===== 116 passed in 1.02s =====\n"), false),
    ]
}

/// [`WIDE`] with the change in place, failing.
fn wide_failing() -> [Message; 2] {
    [
        run(
            "2",
            json!({"program": "sh", "args": [".swe/run", format!("{WIDE} | tail -5")]}),
        ),
        result(
            "2",
            &exited_ok(
                "ERROR test/units/cli/test_galaxy.py::TestGalaxyInitSkeleton::test_role_dirs\n\
                 = 96 failed, 1539 passed, 2 skipped, 54 errors in 27.60s =\n",
            ),
            false,
        ),
    ]
}

/// The result of call `id`, [`WIDE`] failing as it did with the change.
fn failed_before(id: &str) -> Message {
    result(
        id,
        &exited_ok("= 96 failed, 1539 passed, 2 skipped, 54 errors in 27.45s =\n M lib/x.py\n"),
        false,
    )
}

/// The last three test runs of a-4: the tests of the change pass, a wider
/// suite fails, and the same suite fails with the change stashed. Gate
/// counted the stashed run as the change's failing check.
#[test]
fn a_run_without_the_change_is_not_its_check_and_excuses_the_same_run_with_it() {
    let stashed = run(
        "3",
        json!({"program": "sh", "args": [".swe/run", format!(
            "git stash -q; {WIDE} | tail -1; git stash pop -q; git status --short"
        )]}),
    );
    let mut messages = own_tests_passing().to_vec();
    messages.extend(wide_failing());
    messages.extend([stashed, failed_before("3")]);
    let seen = checks_seen(&messages);
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert!(seen[0].passed && seen[0].command.contains("test_manager.py"));
    assert_eq!(
        last_check(&wide_failing()),
        Some(false),
        "nothing shows the failures were there before"
    );
}

#[test]
fn a_suite_that_passes_without_the_change_fails_with_it() {
    let mut passed_before = wide_failing().to_vec();
    passed_before.extend([
        run(
            "3",
            json!({"command": format!("git stash; sh .swe/run {WIDE} | tail -1; git stash pop")}),
        ),
        result(
            "3",
            &exited_ok("===== 1689 passed in 27.45s =====\n"),
            false,
        ),
    ]);
    assert_eq!(last_check(&passed_before), Some(false));
}

#[test]
fn a_change_stashed_in_one_call_and_brought_back_in_another_is_followed() {
    let mut over_calls = wide_failing().to_vec();
    over_calls.extend([
        run("3", json!({"command": "git stash -u"})),
        result("3", "Saved working directory and index state", false),
        run(
            "4",
            json!({"command": format!("sh .swe/run {WIDE} | tail -1")}),
        ),
        failed_before("4"),
        run("5", json!({"command": "git stash pop"})),
        result("5", "Dropped refs/stash@{0}", false),
    ]);
    over_calls.extend(own_tests_passing());
    let seen = checks_seen(&over_calls);
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert!(seen[0].passed);
}

/// A stash that failed stashed nothing: the run after it is the change's.
#[test]
fn a_stash_that_failed_stashed_nothing() {
    let mut not_stashed = vec![
        run("1", json!({"command": "git stash -u"})),
        result(
            "1",
            "exit_code: 128\nstderr:\nfatal: not a git repository",
            true,
        ),
    ];
    not_stashed.extend(wide_failing());
    assert_eq!(last_check(&not_stashed), Some(false));
}

#[test]
fn a_stray_zero_width_character_does_not_hide_a_check() {
    let messages = [
        run(
            "1",
            json!({"program": "sh", "args": [".swe/run", "python\u{200b} -m pytest -q \
                test/units/module_utils/urls/test_prepare_multipart.py 2>&1 | tail -60"]}),
        ),
        result(
            "1",
            r#"{"exit_code":0,"ok":true,"stderr":"","stdout":"..............    [100%]\n14 passed in 0.31s\n"}"#,
            false,
        ),
    ];
    let seen = last_check_seen(&messages).unwrap();
    assert!(seen.passed && seen.by_output, "{seen:?}");
}

#[test]
fn the_last_check_that_ran_decides() {
    let failed_then_passed = [
        run("1", json!({"shell_script": "cargo test"})),
        result("1", r#"{"exit_code":101,"ok":false,"stderr":"..."}"#, true),
        run("2", json!({"program": "cargo", "args": ["test"]})),
        result("2", "test result: ok", false),
    ];
    assert_eq!(last_check(&failed_then_passed), Some(true));

    let passed_then_failed = [
        ran("1", "npm test", "ok", false),
        ran(
            "2",
            "npm run build",
            "exit_code: 1\nstderr:\nerror TS2304",
            true,
        ),
    ]
    .concat();
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
        ran("1", "cargo test", "test result: ok", false),
        ran(
            "2",
            "cargo test",
            "denied: run_command needs approval",
            true,
        ),
        [
            run("3", json!({"command": "cargo test", "background": true})),
            result("3", "started in the background", false),
        ],
        ran("4", "ls", "exit_code: 2", true),
        ran(
            "5",
            "cargo test",
            r#"{"ok":false,"stderr":"command not allowed"}"#,
            true,
        ),
    ]
    .concat();
    assert_eq!(last_check(&messages), Some(true));
    assert_eq!(last_check(&messages[2..]), None);
    assert_eq!(last_check(&[]), None);
}
