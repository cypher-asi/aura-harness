//! The evidence read out of whole attempts, and the flags it raises.

use aura_core_types::PromptCategory;
use aura_model::{Message, ToolResultContent};
use serde_json::json;

use super::flags::tests_named;
use super::scan::KEPT_TEXT;
use super::types::same_file;
use super::*;
use crate::checks::{checks_seen, CheckSeen};
use crate::test_support::{call, reply, result, run};

/// The end of run a-4 of Compare #22: Claude Sonnet 5.5 on
/// `ansible-d33bedc4`, which Gate let stand and the task's own check
/// then failed.
fn a4() -> Vec<Message> {
    vec![
        Message::user("Your task is to implement the changes necessary"),
        call("1", "edit_file", json!({"path": "lib/ansible/module_utils/parsing/convert_bool.py"})),
        result("1", "Replaced 1 occurrence(s)", false),
        run(
            "2",
            json!({"program": "sh", "args": [".swe/run", "python -m pytest -q -p no:cacheprovider test/units/config test/units/module_utils/parsing 2>&1 | grep -E 'passed|failed|FAILED|Error' | head -20"]}),
        ),
        result(
            "2",
            r#"{"exit_code":0,"ok":true,"stderr":"","stdout":"TypeError: TestFindIniFile.test_no_cwd_cfg_no_warning_on_writable.<locals>._os_stat() got an unexpected keyword argument 'follow_symlinks'\nAssertionError: assert '/usr/local/lib/python3.11/site-packages/_pytest/main.py' == '/app/test/units/config/manager'\n======================== 9 passed, 7 warnings in 0.08s =========================\n"}"#,
            false,
        ),
        reply(
            "I rewrote `ensure_type()` in `lib/ansible/config/manager.py` to follow the PR, but it isn't verified. The only test run I got was 9 tests passing, and that run was cut short by an unrelated test error.",
        ),
    ]
}

#[test]
fn the_evidence_of_an_attempt_is_read_from_its_messages() {
    let evidence = Evidence::of(&a4());
    assert_eq!(
        (evidence.calls, evidence.commands, evidence.checks_run),
        (3, 1, 1)
    );
    assert_eq!(
        evidence.files_edited,
        ["lib/ansible/module_utils/parsing/convert_bool.py"]
    );
    let check = evidence.last_check().unwrap();
    assert!(
        !check.passed,
        "the pipe exited 0, but the check printed errors"
    );
    assert!(check.command.starts_with("sh .swe/run python -m pytest"));
    assert!(
        check.output.starts_with("TypeError: TestFindIniFile"),
        "{}",
        check.output
    );
    assert!(check.output.contains("9 passed"));
    assert!(evidence.reply.starts_with("I rewrote `ensure_type()`"));
    let back: Evidence = serde_json::from_str(&serde_json::to_string(&evidence).unwrap()).unwrap();
    assert_eq!(back, evidence);
}

#[test]
fn a_passing_check_that_proves_nothing_is_flagged() {
    let mut evidence = Evidence::of(&a4());
    assert_eq!(
        flags(&evidence, PromptCategory::MultiFileChange),
        [Flag::ReplyUnverified, Flag::NoTestChanged],
        "what the check printed failed it"
    );
    // Attempts already logged may hold such a check as passed.
    evidence.checks[0].passed = true;
    assert_eq!(
        flags(&evidence, PromptCategory::MultiFileChange),
        [
            Flag::MaskedCheck,
            Flag::CheckOutputFailures,
            Flag::MaskedCheckWithFailures,
            Flag::ReplyUnverified,
            Flag::NoTestChanged,
        ]
    );
}

/// The end of run a-2 of Compare #32, its last check running `check`:
/// Claude Haiku 5.5 on `ansible-b748edea`, which ran only the 14 tests it
/// wrote and which Gate let stand. The task's check then failed.
fn a2(check: &str) -> Vec<Message> {
    let tests = "test/units/module_utils/urls/test_prepare_multipart.py";
    vec![
        Message::user("Your task is to implement the changes necessary"),
        call(
            "1",
            "edit_file",
            json!({"path": "lib/ansible/module_utils/urls.py"}),
        ),
        result("1", "Replaced 1 occurrence(s)", false),
        call("2", "write_file", json!({"path": tests})),
        result("2", &format!("Wrote 4655 bytes to {tests}"), false),
        run("3", json!({"program": "sh", "args": [".swe/run", check]})),
        result(
            "3",
            r#"{"exit_code":0,"ok":true,"stderr":"","stdout":"..............    [100%]\n14 passed in 0.31s\n"}"#,
            false,
        ),
        reply(format!(
            "`prepare_multipart` now has 14 tests in `{tests}`, and all 14 pass."
        )),
    ]
}

#[test]
fn a_check_of_only_tests_the_attempt_wrote_is_flagged() {
    let own = "python -m pytest -q test/units/module_utils/urls/test_prepare_multipart.py";
    let evidence = Evidence::of(&a2(own));
    assert_eq!(
        evidence.files_written,
        ["test/units/module_utils/urls/test_prepare_multipart.py"]
    );
    assert_eq!(
        flags(&evidence, PromptCategory::MultiFileChange),
        [Flag::OwnTestsOnly]
    );
    let one = format!("{own}::test_prepare_multipart");
    assert_eq!(
        flags(&Evidence::of(&a2(&one)), PromptCategory::MultiFileChange),
        [Flag::OwnTestsOnly]
    );
    for others_too in [
        "python -m pytest -q test/units/module_utils/urls/",
        "python -m pytest -q test/units/module_utils/urls/test_prepare_multipart.py \
         test/units/module_utils/urls/test_Request.py",
        "python -m pytest -q",
    ] {
        let evidence = Evidence::of(&a2(others_too));
        assert_eq!(
            flags(&evidence, PromptCategory::MultiFileChange),
            [],
            "{others_too}"
        );
    }

    let mut edited = a2(own);
    edited[3] = call(
        "2",
        "edit_file",
        json!({"path": "test/units/module_utils/urls/test_prepare_multipart.py"}),
    );
    let evidence = Evidence::of(&edited);
    assert!(evidence.files_written.is_empty(), "the file was there");
    assert_eq!(flags(&evidence, PromptCategory::MultiFileChange), []);
    let back: Evidence =
        serde_json::from_str(&serde_json::to_string(&Evidence::of(&a2(own))).unwrap()).unwrap();
    assert_eq!(back, Evidence::of(&a2(own)));
}

#[test]
fn the_tests_a_command_names_are_read_from_its_words() {
    assert_eq!(
        tests_named(
            "sh .swe/run 'python -m pytest -q -p no:cacheprovider test/units/config \
             test/units/x/test_y.py::TestZ 2>&1 | grep -E \"passed|failed\" | tail -60'"
        ),
        ["test/units/config", "test/units/x/test_y.py"]
    );
    assert_eq!(
        tests_named("npx vitest run src/parse.test.ts --reporter=dot"),
        ["src/parse.test.ts"]
    );
    for none in [
        "cargo test -p parser",
        "python -m pytest -q -k test_parse",
        "npm test",
    ] {
        assert!(tests_named(none).is_empty(), "{none}");
    }
}

#[test]
fn a_clean_attempt_raises_no_flag() {
    let messages = [
        call("1", "edit_file", json!({"path": "src/lib.rs"})),
        result("1", "ok", false),
        call("0", "edit_file", json!({"path": "tests/parser.rs"})),
        result("0", "ok", false),
        run("2", json!({"command": "cargo test -p parser"})),
        result(
            "2",
            "test result: ok. 12 passed; 0 failed; 0 ignored",
            false,
        ),
        reply("Fixed the parser; `cargo test -p parser` passes (12 tests)."),
    ];
    let mut evidence = Evidence::of(&messages);
    evidence.note_changed(&["src/lib.rs".to_string(), "tests/parser.rs".to_string()]);
    evidence.hit_limit = Some(false);
    assert_eq!(flags(&evidence, PromptCategory::SmallChange), []);
}

#[test]
fn how_the_change_was_made_and_checked_is_flagged() {
    let messages = [
        call("1", "edit_file", json!({"path": "/work/copy/src/lib.rs"})),
        result("1", "ok", false),
        run("2", json!({"command": "cargo test -p parser"})),
        result("2", "test result: ok. 12 passed; 0 failed", false),
        run("3", json!({"command": "sed -i 's/a/b/' src/parse.rs"})),
        result("3", "", false),
        reply("Done. No tests exist for the new path, so the suite doesn't cover it."),
    ];
    let mut evidence = Evidence::of(&messages);
    assert!(evidence.changed_after_check, "sed ran after the tests");
    assert_eq!(evidence.changed_by_commands(), None, "nothing compared");
    assert_eq!(
        flags(&evidence, PromptCategory::SmallChange),
        [Flag::StaleCheck, Flag::ReplyNotCovered, Flag::NoTestChanged]
    );
    evidence.note_changed(&["src/lib.rs".to_string(), "src/parse.rs".to_string()]);
    assert_eq!(evidence.files_changed, Some(2));
    assert_eq!(evidence.changed_by_commands(), Some(vec!["src/parse.rs"]));
    assert_eq!(
        flags(&evidence, PromptCategory::SmallChange),
        [
            Flag::ShellEdit,
            Flag::StaleCheck,
            Flag::ReplyNotCovered,
            Flag::NoTestChanged
        ]
    );
    let back: Evidence = serde_json::from_str(&serde_json::to_string(&evidence).unwrap()).unwrap();
    assert_eq!(back, evidence);
}

#[test]
fn a_change_the_last_check_ran_after_leaves_it_standing() {
    let checked_last = [
        run("1", json!({"command": "sed -i 's/a/b/' src/parse.rs"})),
        result("1", "", false),
        run("2", json!({"command": "cargo test"})),
        result("2", "ok", false),
    ];
    assert!(!Evidence::of(&checked_last).changed_after_check);
    assert!(same_file("/work/copy/src/lib.rs", "src/lib.rs"));
    assert!(same_file("./src/lib.rs", "src/lib.rs"));
    assert!(!same_file("src/mylib.rs", "lib.rs"));
    assert!(Flag::ALL.iter().all(|flag| !flag.describe().is_empty()));
}

#[test]
fn the_other_signs_are_flagged() {
    let mut evidence = Evidence::of(&[reply(
        "I was unable to find the cause. Two tests still fail.",
    )]);
    evidence.files_changed = Some(0);
    evidence.hit_limit = Some(true);
    assert_eq!(
        flags(&evidence, PromptCategory::Debugging),
        [
            Flag::NoCheck,
            Flag::ReplyFailures,
            Flag::NoChanges,
            Flag::GaveUp,
            Flag::HitLimit
        ]
    );
    assert_eq!(
        flags(&evidence, PromptCategory::Question),
        [
            Flag::NoCheck,
            Flag::ReplyFailures,
            Flag::GaveUp,
            Flag::HitLimit
        ],
        "a question changes no file"
    );
}

#[test]
fn long_output_keeps_its_end_and_a_long_reply_its_start() {
    let long = format!("{}END", "x".repeat(3 * KEPT_TEXT));
    let messages = [
        run("1", json!({"command": "pytest"})),
        result("1", &long, false),
        reply(format!("START{}", "y".repeat(3 * KEPT_TEXT))),
    ];
    let evidence = Evidence::of(&messages);
    let check = evidence.last_check().unwrap();
    assert_eq!(check.output.chars().count(), KEPT_TEXT);
    assert!(check.output.ends_with("END"));
    assert_eq!(evidence.reply.chars().count(), KEPT_TEXT);
    assert!(evidence.reply.starts_with("START"));
}

/// A turn with every kind of call and message the evidence reads.
fn every_kind() -> Vec<Message> {
    let piped = Message::tool_results(vec![(
        "5".to_string(),
        ToolResultContent::Json(json!({
            "exit_code": 0,
            "ok": true,
            "stderr": "",
            "stdout": "test result: ok. 3 passed; 0 failed"
        })),
        false,
    )]);
    vec![
        Message::user("Make the parser strict."),
        reply("I'll read the parser first."),
        call("1", "read_file", json!({"path": "src/parse.rs"})),
        result("1", "fn parse() {}", false),
        call("2", "edit_file", json!({"path": "src/parse.rs"})),
        result("2", "Replaced 1 occurrence(s)", false),
        call("3", "write_file", json!({"path": "tests/strict.rs"})),
        result("3", "Wrote 120 bytes to tests/strict.rs", false),
        call("4", "edit_file", json!({"path": "src/parse.rs"})),
        result("4", "Replaced 1 occurrence(s)", false),
        run("5", json!({"command": "cargo test 2>&1 | tail -3"})),
        piped,
        Message::user(
            "[harness] You changed files but did not run any test or build to check the change.",
        ),
        run("6", json!({"command": "sed -i 's/a/b/' src/lex.rs"})),
        result("6", "", false),
        run("7", json!({"program": "cargo", "args": ["build"]})),
        result("7", "exit_code: 101\nerror[E0308]: mismatched types", true),
        run("8", json!({"command": "cargo test", "background": true})),
        result("8", "started in the background", false),
        call("9", "delete_file", json!({"path": "src/old.rs"})),
        result("9", "Deleted src/old.rs", false),
        reply("  The parser is strict now, but the build still fails.\n"),
    ]
}

/// The evidence is written to Gate's attempt log as it is serialized
/// here, so its bytes are pinned.
#[test]
fn the_evidence_written_to_the_log_is_pinned() {
    let mut evidence = Evidence::of(&every_kind());
    evidence.note_changed(&[
        "src/parse.rs".to_string(),
        "tests/strict.rs".to_string(),
        "src/old.rs".to_string(),
        "src/lex.rs".to_string(),
    ]);
    evidence.hit_limit = Some(false);
    assert_eq!(
        serde_json::to_string(&evidence).unwrap(),
        r#"{"calls":11,"commands":4,"checks_run":2,"nudges":1,"checks":[{"command":"cargo test 2>&1 | tail -3","passed":true,"output":"test result: ok. 3 passed; 0 failed"},{"command":"cargo build","passed":false,"output":"exit_code: 101\nerror[E0308]: mismatched types"}],"files_edited":["src/parse.rs","tests/strict.rs","src/old.rs"],"files_written":["tests/strict.rs"],"files_changed":4,"changed_paths":["src/parse.rs","tests/strict.rs","src/old.rs","src/lex.rs"],"changed_after_check":true,"hit_limit":false,"reply":"The parser is strict now, but the build still fails."}"#
    );
    assert_eq!(
        flags(&evidence, PromptCategory::MultiFileChange),
        [Flag::ReplyFailures, Flag::ShellEdit, Flag::StaleCheck]
    );
    assert_eq!(
        serde_json::to_string(&Flag::ALL).unwrap(),
        r#"["masked_check","check_output_failures","masked_check_with_failures","reply_unverified","reply_failures","no_check","no_changes","gave_up","hit_limit","shell_edit","stale_check","reply_not_covered","no_test_changed","own_tests_only"]"#
    );
}

#[test]
fn the_checks_seen_in_a_turn_are_pinned() {
    assert_eq!(
        checks_seen(&every_kind()),
        [
            CheckSeen {
                command: "cargo test 2>&1 | tail -3".to_string(),
                passed: true,
                by_output: true,
                output: r#"{"exit_code":0,"ok":true,"stderr":"","stdout":"test result: ok. 3 passed; 0 failed"}"#.to_string(),
            },
            CheckSeen {
                command: "cargo build".to_string(),
                passed: false,
                by_output: false,
                output: "exit_code: 101\nerror[E0308]: mismatched types".to_string(),
            },
        ]
    );
}
