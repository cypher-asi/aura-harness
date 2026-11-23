//! Whether a check that ran to its end passed.

use super::command::{exit_status_masked, runs_after_check};
use crate::transcript::{shows_failures, Output};

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

/// Pass or fail from the result of a check that ran `command`, its
/// `output` and whether the call failed, and whether what the check
/// printed decided that; `None` when the command did not run to its end.
/// What the check printed ([`output_verdict`]) decides a passing exit
/// that is not the check's own ([`exit_status_masked`]), and a failing
/// exit that a command after the check may have set
/// ([`runs_after_check`]).
pub(super) fn verdict(output: &Output, is_error: bool, command: &str) -> Option<(bool, bool)> {
    let by_output = || output_verdict(output.printed()).map(|passed| (passed, true));
    if !is_error {
        return if exit_status_masked(command) {
            by_output()
        } else {
            Some((true, false))
        };
    }
    if !output.exited {
        return None;
    }
    if runs_after_check(command) {
        by_output()
    } else {
        Some((false, false))
    }
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
