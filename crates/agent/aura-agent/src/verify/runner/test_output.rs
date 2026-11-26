//! Test runner output parsing (cargo, Jest, pytest, Go, RSpec).

use super::truncate_output;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct IndividualTestResult {
    pub name: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Parse test runner output into individual test results and a summary line.
///
/// Recognises cargo test, Jest/Vitest/Mocha-style JS runners, pytest, Go
/// `go test`, and RSpec. Each parser returns an empty vec when its format
/// signature isn't present, so the chain falls through to the next one. If
/// nothing matches, a single aggregate result is synthesised from the exit
/// code so unparseable output still flows through the DoD test gate.
///
/// The DoD hard gate combines this parser with the runner's exit code: a
/// suite is considered passing only when `success == true` AND no parsed
/// failure is present. That means a runner whose output we don't recognise
/// still gates correctly via its exit code, while runners we *do* recognise
/// also surface failing test names back to the agent so it can fix the
/// right thing.
pub fn parse_test_output(
    stdout: &str,
    stderr: &str,
    success: bool,
) -> (Vec<IndividualTestResult>, String) {
    let combined = format!("{stdout}\n{stderr}");

    for parser in [
        parse_cargo_test as fn(&str) -> Vec<IndividualTestResult>,
        parse_jest_output,
        parse_pytest_output,
        parse_go_test_output,
        parse_rspec_output,
    ] {
        let results = parser(&combined);
        if !results.is_empty() {
            return (results.clone(), tally_summary(&results));
        }
    }

    let status = if success { "passed" } else { "failed" };
    let summary = if success {
        "all tests passed".to_string()
    } else {
        "tests failed".to_string()
    };
    let result = IndividualTestResult {
        name: "(aggregate)".to_string(),
        status: status.to_string(),
        message: if success {
            None
        } else {
            Some(truncate_output(&combined, 2000))
        },
    };
    (vec![result], summary)
}

fn tally_summary(results: &[IndividualTestResult]) -> String {
    let passed = results.iter().filter(|r| r.status == "passed").count();
    let failed = results.iter().filter(|r| r.status == "failed").count();
    let skipped = results.iter().filter(|r| r.status == "skipped").count();
    if skipped > 0 {
        format!("{passed} passed, {failed} failed, {skipped} skipped")
    } else {
        format!("{passed} passed, {failed} failed")
    }
}

fn parse_cargo_test(output: &str) -> Vec<IndividualTestResult> {
    let mut results = Vec::new();
    for line in output.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with("test ") {
            continue;
        }
        let rest = &trimmed[5..];
        if let Some(idx) = rest.find(" ... ") {
            let name = rest[..idx].trim().to_string();
            let outcome = rest[idx + 5..].trim();
            let status = match outcome {
                "ok" => "passed",
                "FAILED" => "failed",
                s if s.starts_with("ignored") => "skipped",
                _ => continue,
            };
            let message = if status == "failed" {
                Some(outcome.to_string())
            } else {
                None
            };
            results.push(IndividualTestResult {
                name,
                status: status.to_string(),
                message,
            });
        }
    }
    results
}

fn parse_jest_output(output: &str) -> Vec<IndividualTestResult> {
    let mut results = Vec::new();
    for line in output.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("PASS ") {
            results.push(IndividualTestResult {
                name: rest.trim().to_string(),
                status: "passed".to_string(),
                message: None,
            });
        } else if let Some(rest) = trimmed.strip_prefix("FAIL ") {
            results.push(IndividualTestResult {
                name: rest.trim().to_string(),
                status: "failed".to_string(),
                message: None,
            });
        } else if trimmed.starts_with("\u{2713} ") || trimmed.starts_with("✓ ") {
            results.push(IndividualTestResult {
                name: trimmed[2..].trim().to_string(),
                status: "passed".to_string(),
                message: None,
            });
        } else if trimmed.starts_with("\u{2717} ")
            || trimmed.starts_with("✕ ")
            || trimmed.starts_with("✗ ")
        {
            results.push(IndividualTestResult {
                name: trimmed[3..].trim().to_string(),
                status: "failed".to_string(),
                message: None,
            });
        }
    }
    results
}

/// Parse pytest output. Pytest emits two relevant formats:
///
/// * Verbose live output (`pytest -v`):
///   `tests/foo.py::test_x PASSED                          [ 25%]`
///   — the test ID is *first*, the status word is in the middle.
/// * Short-form summary block (`pytest -q` and the trailing summary
///   of any verbose run):
///   `FAILED tests/foo.py::test_x - AssertionError: ...`
///   — the status word is *first*, the test ID follows.
///
/// We try both shapes per line and dedupe by test ID so a verbose run
/// that emits the same failure in both the live log and the summary
/// block still counts as one failure. Lines that don't contain a
/// `pytest::node` style ID are ignored to avoid grabbing unrelated
/// `FAILED to load shared lib` chatter from build noise.
fn parse_pytest_output(output: &str) -> Vec<IndividualTestResult> {
    let mut results = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for line in output.lines() {
        let trimmed = line.trim();
        let Some((status, name)) = parse_pytest_line(trimmed) else {
            continue;
        };
        if !looks_like_pytest_node(&name) {
            continue;
        }
        if !seen.insert(name.clone()) {
            continue;
        }
        results.push(IndividualTestResult {
            name,
            status: status.to_string(),
            message: None,
        });
    }

    results
}

fn parse_pytest_line(line: &str) -> Option<(&'static str, String)> {
    // Shape 1: status keyword at the start. Used in pytest's tail
    // summary block and by `pytest -rf` style flag rendering.
    if let Some((status, rest)) = strip_pytest_status_prefix(line) {
        let name = rest.split(" - ").next().unwrap_or(rest).trim().to_string();
        if !name.is_empty() {
            return Some((status, name));
        }
    }

    // Shape 2: status keyword at the end of the line (possibly
    // followed by `[ NN% ]`). Used by `pytest -v`.
    if let Some((status, name)) = strip_pytest_status_suffix(line) {
        return Some((status, name));
    }

    None
}

fn strip_pytest_status_prefix(line: &str) -> Option<(&'static str, &str)> {
    for (kw, status) in PYTEST_KEYWORDS {
        if let Some(rest) = line.strip_prefix(kw).and_then(|r| r.strip_prefix(' ')) {
            return Some((status, rest));
        }
    }
    None
}

fn strip_pytest_status_suffix(line: &str) -> Option<(&'static str, String)> {
    // Drop the trailing percent indicator if present: `... PASSED [ 25%]`.
    let body = if let Some(idx) = line.rfind('[') {
        let trailing = line[idx..].trim();
        if trailing.ends_with("%]") {
            line[..idx].trim_end()
        } else {
            line
        }
    } else {
        line
    };

    for (kw, status) in PYTEST_KEYWORDS {
        if let Some(prefix) = body.strip_suffix(kw) {
            if prefix.ends_with(char::is_whitespace) {
                let name = prefix.trim().to_string();
                if !name.is_empty() {
                    return Some((status, name));
                }
            }
        }
    }
    None
}

const PYTEST_KEYWORDS: &[(&str, &str)] = &[
    ("PASSED", "passed"),
    ("FAILED", "failed"),
    ("SKIPPED", "skipped"),
    ("ERROR", "failed"),
    ("XFAIL", "skipped"),
    ("XPASS", "passed"),
];

/// Pytest test IDs always contain `::` (e.g. `tests/foo.py::test_x` or
/// `tests/foo.py::Class::test_x`). Filter on this so we don't snag
/// unrelated `FAILED ...` lines from non-pytest log output.
fn looks_like_pytest_node(name: &str) -> bool {
    name.contains("::")
}

/// Parse `go test` output. Recognises the `--- PASS:`, `--- FAIL:`,
/// `--- SKIP:` summary lines emitted by the standard testing package, plus
/// the per-package `FAIL\tpkg/...` lines that surface when a package fails
/// to build before any test runs.
fn parse_go_test_output(output: &str) -> Vec<IndividualTestResult> {
    const GO_PREFIXES: &[(&str, &str)] = &[
        ("--- PASS: ", "passed"),
        ("--- FAIL: ", "failed"),
        ("--- SKIP: ", "skipped"),
    ];

    let mut results = Vec::new();
    for line in output.lines() {
        let trimmed = line.trim();

        let parsed = GO_PREFIXES
            .iter()
            .find_map(|(prefix, status)| trimmed.strip_prefix(prefix).map(|rest| (*status, rest)));

        if let Some((status, rest)) = parsed {
            // `--- FAIL: TestName (0.12s)` → keep just `TestName`.
            let name = rest.split_whitespace().next().unwrap_or("").to_string();
            if !name.is_empty() {
                results.push(IndividualTestResult {
                    name,
                    status: status.to_string(),
                    message: None,
                });
            }
            continue;
        }

        // Build-time / package-level failures look like:
        //   `FAIL\tgithub.com/example/pkg [build failed]`
        // No individual test names available, but we still want the run
        // to register as failing.
        if let Some(rest) = trimmed.strip_prefix("FAIL\t") {
            let name = rest.split_whitespace().next().unwrap_or(rest);
            if !name.is_empty() {
                results.push(IndividualTestResult {
                    name: format!("{name} (package)"),
                    status: "failed".to_string(),
                    message: None,
                });
            }
        }
    }
    results
}

/// Parse RSpec's progress output. Looks for the failure summary block
/// (`Failures:` followed by `1) <full description>`) and counts examples
/// from the trailing `N examples, M failures, K pending` line.
fn parse_rspec_output(output: &str) -> Vec<IndividualTestResult> {
    let mut results = rspec_named_failures(output);

    // Synthesise pass entries from the summary line so a green RSpec run
    // also produces a non-empty parse result. Without this, an all-green
    // RSpec invocation would fall through to the aggregate-only branch.
    if let Some(stats) = rspec_summary_counts(output) {
        push_rspec_summary_entries(&mut results, &stats);
    }

    results
}

/// Collect the named failures from RSpec's `Failures:` block.
fn rspec_named_failures(output: &str) -> Vec<IndividualTestResult> {
    let mut results = Vec::new();
    let mut in_failures_block = false;
    for line in output.lines() {
        let trimmed = line.trim();

        if trimmed == "Failures:" {
            in_failures_block = true;
            continue;
        }
        if in_failures_block {
            if let Some(rest) = trimmed
                .strip_prefix(|c: char| c.is_ascii_digit())
                .and_then(|r| r.strip_prefix(')'))
            {
                let name = rest.trim().to_string();
                if !name.is_empty() {
                    results.push(IndividualTestResult {
                        name,
                        status: "failed".to_string(),
                        message: None,
                    });
                }
            }
            // The failures block ends when we hit a line starting with a
            // summary marker (`Finished in`, `N examples`, etc.).
            if trimmed.starts_with("Finished in") || trimmed.contains(" examples,") {
                in_failures_block = false;
            }
        }
    }
    results
}

/// Append pass/pending entries and any unnamed failures implied by the
/// RSpec summary line.
fn push_rspec_summary_entries(results: &mut Vec<IndividualTestResult>, stats: &RspecStats) {
    let already_failed = results.len();
    let passes = stats
        .examples
        .saturating_sub(stats.failures + stats.pending);
    for i in 0..passes {
        results.push(IndividualTestResult {
            name: format!("(rspec example #{idx})", idx = i + 1),
            status: "passed".to_string(),
            message: None,
        });
    }
    for i in 0..stats.pending {
        results.push(IndividualTestResult {
            name: format!("(rspec pending #{idx})", idx = i + 1),
            status: "skipped".to_string(),
            message: None,
        });
    }
    // Reconcile: if the summary reports more failures than the named
    // ones we captured, top up with anonymous entries so the gate
    // surfaces the right *count* even when failure descriptions
    // weren't parseable.
    let extra_fail = stats.failures.saturating_sub(already_failed);
    for i in 0..extra_fail {
        results.push(IndividualTestResult {
            name: format!("(rspec failure #{idx})", idx = already_failed + i + 1),
            status: "failed".to_string(),
            message: None,
        });
    }
}

#[derive(Default)]
struct RspecStats {
    examples: usize,
    failures: usize,
    pending: usize,
}

fn rspec_summary_counts(output: &str) -> Option<RspecStats> {
    for line in output.lines() {
        let trimmed = line.trim();
        // Looking for: `12 examples, 3 failures, 1 pending` (pending is
        // optional). Be lenient on whitespace.
        if !trimmed.contains(" examples,") {
            continue;
        }
        let mut stats = RspecStats::default();
        for chunk in trimmed.split(',') {
            let chunk = chunk.trim();
            let mut parts = chunk.split_whitespace();
            let Some(num) = parts.next().and_then(|s| s.parse::<usize>().ok()) else {
                continue;
            };
            match parts.next() {
                Some("examples") | Some("example") => stats.examples = num,
                Some("failures") | Some("failure") => stats.failures = num,
                Some("pending") => stats.pending = num,
                _ => {}
            }
        }
        if stats.examples > 0 || stats.failures > 0 {
            return Some(stats);
        }
    }
    None
}
