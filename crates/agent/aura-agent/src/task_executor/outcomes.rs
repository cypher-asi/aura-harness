//! Rolling window of recent tool-call outcomes and policy-denial detection.

use super::RECENT_OUTCOMES_WINDOW;

/// One slot in the [`RecentToolOutcomes`] ring buffer.
#[derive(Debug, Clone, Copy)]
struct OutcomeEntry {
    is_error: bool,
    /// True when the error was a policy denial (e.g. `run_command` not
    /// in the allow-list) rather than a tool that actually executed
    /// and returned a non-zero exit. Policy denials are not counted as
    /// "real" failures against the pervasive-error guard because the
    /// agent has nothing to fix — it just needs to stop calling the
    /// blocked tool.
    policy_denied: bool,
}

/// Rolling window of recent tool-call outcomes used by the pervasive-
/// error guard on `task_done`.
///
/// Earlier revisions kept monotonic `total` / `errors` counters for the
/// lifetime of the `TaskToolExecutor`, which meant a noisy exploration
/// phase (e.g. a handful of `read_file` calls against directories,
/// plus policy-denied `run_command` attempts) could push the error
/// ratio over the 70% threshold and reject a `task_done` that
/// otherwise represented successful work.
///
/// The ring buffer keeps only the last [`RECENT_OUTCOMES_WINDOW`]
/// outcomes, and `reset()` is called when the executor transitions to
/// the implementing phase (after `submit_plan`) so prior exploration
/// noise never influences the completion check.
#[derive(Debug, Default)]
pub(crate) struct RecentToolOutcomes {
    entries: std::collections::VecDeque<OutcomeEntry>,
    /// True when the most recent `run_command` actually executed and
    /// returned a non-zero exit. Policy-denied commands *do not* set
    /// this flag because nothing ran.
    pub(crate) last_command_failed: bool,
    /// Set after `task_done` is rejected only because no writes landed and
    /// `no_changes_needed` was omitted. While set, exploratory detours are
    /// blocked so the next useful action is a write or corrected task_done.
    pub(crate) task_done_no_writes_rejected: bool,
}

impl RecentToolOutcomes {
    /// Record one tool-call outcome.
    pub fn record(&mut self, tool_name: &str, is_error: bool, content: &str) {
        let policy_denied = is_error && is_policy_denial(content);
        while self.entries.len() >= RECENT_OUTCOMES_WINDOW {
            self.entries.pop_front();
        }
        self.entries.push_back(OutcomeEntry {
            is_error,
            policy_denied,
        });
        if tool_name == "run_command" {
            // Policy denial means the command never ran — the agent
            // isn't staring at a broken build, it just hit an
            // allow-list. Treat it as a non-event for the "last
            // command failed" signal the completion guard reads.
            self.last_command_failed = is_error && !policy_denied;
        }
    }

    /// Total outcomes currently in the window.
    #[must_use]
    pub fn total(&self) -> usize {
        self.entries.len()
    }

    /// Errors in the window that were *not* policy denials.
    #[must_use]
    pub fn real_errors(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| e.is_error && !e.policy_denied)
            .count()
    }

    /// Clear the window. Called on plan acceptance so the noisy
    /// exploration phase never votes against a clean implementation
    /// phase.
    pub fn reset(&mut self) {
        self.entries.clear();
        self.last_command_failed = false;
        self.task_done_no_writes_rejected = false;
    }
}

pub(super) fn allowed_after_no_write_task_done_reject(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "task_done" | "write_file" | "edit_file" | "delete_file"
    )
}

/// Heuristic: does this tool-result content look like a policy denial
/// rather than an actual tool failure?
///
/// `aura-kernel`'s `PolicyVerdict::Deny` stamps one of a small set of
/// reasons into the tool result. We match on those prefixes so the
/// completion guard can distinguish "you tried to run a blocked tool"
/// from "the command you ran exited non-zero". The matching is
/// deliberately conservative — if a downstream tool happens to emit
/// one of these strings in its stderr, treating it as a policy denial
/// is safe (it just means we don't count it against the error ratio),
/// and the `last_command_failed` short-circuit is still protected by
/// the tool-name check on `run_command`.
pub(crate) fn is_policy_denial(content: &str) -> bool {
    let trimmed = content.trim_start();
    trimmed.starts_with("Tool '") && trimmed.contains("is not allowed")
        || trimmed.starts_with("Tool '") && trimmed.contains("requires approval")
        || trimmed.starts_with("Policy denied")
        || trimmed.starts_with("permissions: requires capability")
        || trimmed.starts_with("permissions: target out of scope")
}
