//! What a failed command said, for error messages: a build's compiler
//! errors first, the tail of its output after, and whether it looks like
//! the machine failed rather than the code.

use crate::error::CloudError;
use crate::provider::ExecOutput;

/// Bytes of the end of the output kept after the errors.
const TAIL_BYTES: usize = 300;

/// What in a failed command's report says the machine, the network or a
/// race failed it, not the code: worth one more try on a fresh machine.
const TRANSIENT_SIGNS: &[&str] = &[
    "sigkill",
    "signal: 9",
    "killed",
    "out of memory",
    "cannot allocate memory",
    "no space left on device",
    "failed to download",
    "spurious network error",
    "could not resolve host",
    "connection reset",
    "connection refused",
    "timed out",
    "failed to get `",
];

impl ExecOutput {
    /// What failed, from stderr, for error messages: its error blocks
    /// (lines from one starting `error` up to the next `warning` or cargo
    /// status line, `Caused by:` included), the first ones first, up to
    /// `max` bytes. A build's warnings, `unused dependency` among them,
    /// are printed after its errors and would otherwise fill the
    /// message. Without any error line, the last `max` bytes
    /// ([`Self::stderr_tail`]).
    #[must_use]
    pub fn stderr_errors(&self, max: usize) -> String {
        let text = String::from_utf8_lossy(&self.stderr);
        let mut errors = String::new();
        let mut in_error = false;
        for line in text.lines() {
            let head = line.trim_start();
            if head.starts_with("error") {
                in_error = true;
            } else if head.starts_with("warning") || is_status_line(head) {
                in_error = false;
            }
            if !in_error || head.is_empty() {
                continue;
            }
            if errors.len() + line.len() + 1 > max {
                if errors.is_empty() {
                    errors.extend(line.chars().take(max));
                }
                break;
            }
            errors.push_str(line);
            errors.push('\n');
        }
        let errors = errors.trim().to_string();
        if errors.is_empty() {
            self.stderr_tail(max)
        } else {
            errors
        }
    }

    /// [`Self::stderr_errors`] up to `max` bytes, then the last lines of
    /// the output, so the message shows both what failed and how the
    /// command ended.
    #[must_use]
    pub fn stderr_summary(&self, max: usize) -> String {
        let errors = self.stderr_errors(max);
        let tail = self.stderr_tail(TAIL_BYTES);
        let tail = tail.trim();
        if tail.is_empty() || errors.contains(tail) {
            errors
        } else {
            format!("{errors}\n… last lines:\n{tail}")
        }
    }
}

/// Whether `line`, trimmed, is one of cargo's progress lines.
fn is_status_line(line: &str) -> bool {
    [
        "Compiling ",
        "Checking ",
        "Downloaded ",
        "Downloading ",
        "Finished ",
        "Fresh ",
        "Executable ",
    ]
    .iter()
    .any(|status| line.starts_with(status))
}

/// Whether `err`, a step's failure, looks like the machine or the network
/// failed rather than the code: a killed or out-of-memory process, a full
/// disk, a download that failed. A compiler error never is.
#[must_use]
pub fn is_transient(err: &CloudError) -> bool {
    let CloudError::ExecFailed {
        exit_code, stderr, ..
    } = err
    else {
        return matches!(
            err,
            CloudError::Timeout { .. } | CloudError::Transport { .. }
        );
    };
    let said = stderr.to_lowercase();
    if said.contains("error[e") {
        return false;
    }
    *exit_code == 137 || TRANSIENT_SIGNS.iter().any(|sign| said.contains(sign))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failed(stderr: &str) -> ExecOutput {
        ExecOutput {
            exit_code: 101,
            stdout: Vec::new(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    /// What a cargo build killed for memory prints: the error, why, then
    /// the unused-dependency warnings cargo gives at the very end.
    const KILLED: &str = "   Compiling aura-app v0.1.0\n\
        error: could not compile `aura-app` (lib test)\n\
        \n\
        Caused by:\n  process didn't exit successfully: `rustc --crate-name aura_app` (signal: 9, SIGKILL: kill)\n\
        warning: unused dependency `serde_json`\n  --> crates/core/aura-core-permissions/Cargo.toml:13:1\n\
        warning: unused dependency `static_assertions`\n  --> crates/store/aura-store-db/Cargo.toml:17:1\n";

    #[test]
    fn a_failed_builds_errors_come_before_its_warnings() {
        let out = failed(
            "   Compiling a v0.1.0\n\
             warning: unused dependency `serde_json`\n  --> Cargo.toml:13:1\n\n\
             error[E0425]: cannot find value `x` in this scope\n  --> src/lib.rs:3:5\n\n\
             warning: unused import\n\n\
             error: could not compile `a` (lib test) due to 1 previous error\n",
        );
        assert_eq!(
            out.stderr_errors(800),
            "error[E0425]: cannot find value `x` in this scope\n  --> src/lib.rs:3:5\n\
             error: could not compile `a` (lib test) due to 1 previous error"
        );
        let first = out.stderr_errors(60);
        assert!(
            first.starts_with("error[E0425]: cannot find value"),
            "{first}"
        );
        assert!(first.len() <= 60, "{first}");
        let warned = failed("warning: a\nall done badly");
        assert_eq!(
            warned.stderr_errors(8),
            "ne badly",
            "no error line: the tail"
        );
    }

    #[test]
    fn a_killed_build_says_why_before_the_warnings_and_ends_with_the_tail() {
        let summary = failed(KILLED).stderr_summary(800);
        assert!(
            summary.starts_with(
                "error: could not compile `aura-app` (lib test)\nCaused by:\n  process didn't \
                 exit successfully: `rustc --crate-name aura_app` (signal: 9, SIGKILL: kill)\n\
                 … last lines:\n"
            ),
            "{summary}"
        );
        assert!(
            summary.ends_with("aura-store-db/Cargo.toml:17:1"),
            "{summary}"
        );
    }

    fn exec_failed(stderr: &str, exit_code: i32) -> CloudError {
        CloudError::ExecFailed {
            machine: "aura-proj-575d-build-c96a24".into(),
            command: "cargo test --workspace --no-run".into(),
            exit_code,
            stderr: stderr.into(),
        }
    }

    #[test]
    fn a_killed_or_starved_build_is_worth_a_retry_and_a_compile_error_is_not() {
        let killed = failed(KILLED).stderr_summary(800);
        assert!(is_transient(&exec_failed(&killed, 101)));
        assert!(is_transient(&exec_failed(
            "ld: final link failed: No space left on device",
            101
        )));
        assert!(is_transient(&exec_failed("", 137)), "exit 137 is a kill");
        assert!(is_transient(&exec_failed(
            "error: failed to download `serde v1.0`",
            101
        )));
        let compile = "error[E0063]: missing field `run_started` in initializer of `TurnExtras`";
        assert!(!is_transient(&exec_failed(compile, 101)));
        assert!(!is_transient(&exec_failed(
            "error: could not compile `a`",
            101
        )));
    }
}
