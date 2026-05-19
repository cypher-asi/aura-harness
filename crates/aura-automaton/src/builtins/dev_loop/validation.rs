//! Post-execution validation: turning a [`TaskExecutionResult`] into
//! either a forward-progress signal or a structured `NeedsDecomposition`
//! hint the orchestrator can consume.
//!
//! Kept separate from `aggregate.rs` because the two abstractions answer
//! different questions: `TaskAggregate` summarises file-change evidence
//! used by the commit gate, whereas `validate_execution` decides whether
//! the task should be retried, decomposed, or surfaced as a hard failure.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use aura_agent::agent_runner::TaskExecutionResult;
use aura_reasoner::{ContentBlock, Message, Role};
use serde::{Deserialize, Serialize};

use crate::error::AutomatonError;

/// Hard timeout for the in-process build preflight. Mirrors the
/// server-side gate in `aura-os-server::handlers::dev_loop::signals::
/// build_preflight` so the two gates stay in lockstep.
const BUILD_PREFLIGHT_TIMEOUT: Duration = Duration::from_secs(90);

/// Outcome of [`validate_build_preflight`]. Returned by the helper so
/// the caller can decide whether to keep the task `Done` or demote it
/// to `Failed` via the existing failure path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildPreflightOutcome {
    /// `true` when `cargo check` exited 0 within the timeout (or when
    /// the workspace isn't a Cargo project — the gate is Rust-only).
    pub ok: bool,
    /// First `Eddd` code surfaced by cargo, when extractable.
    pub first_error_code: Option<String>,
    /// Truncated tail of combined stdout+stderr (max 4 KiB).
    pub stderr_tail: String,
    /// `true` when the process was killed by the timeout.
    pub timed_out: bool,
}

/// True when the env var `AURA_BUILD_GATE` is set to a truthy value.
/// Orchestrators call this before invoking [`validate_build_preflight`]
/// so the gate is opt-in.
#[must_use]
pub fn build_preflight_gate_enabled() -> bool {
    std::env::var("AURA_BUILD_GATE").is_ok_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

/// Run `cargo check --message-format=short --quiet` against the given
/// workspace. The dev-loop orchestrator calls this AFTER
/// [`validate_execution`] returns `Ok(_)` and BEFORE persisting the
/// task as `Done`; when the verdict is `ok == false` the orchestrator
/// demotes the task via the existing `AutomatonError::AgentExecution`
/// path so retry budgets / failure reasons keep working unchanged.
///
/// Returns `BuildPreflightOutcome { ok: true, .. }` when the
/// workspace isn't a Cargo project — non-Rust workspaces aren't
/// gated.
#[must_use]
pub fn validate_build_preflight(workspace_path: &str) -> BuildPreflightOutcome {
    if workspace_path.trim().is_empty() {
        return BuildPreflightOutcome {
            ok: false,
            first_error_code: None,
            stderr_tail: "build preflight: workspace path is empty".into(),
            timed_out: false,
        };
    }
    let path = Path::new(workspace_path);
    if !path.exists() {
        return BuildPreflightOutcome {
            ok: false,
            first_error_code: None,
            stderr_tail: format!(
                "build preflight: workspace does not exist on disk: {workspace_path}"
            ),
            timed_out: false,
        };
    }
    if !path.join("Cargo.toml").exists() && !path.join("Cargo.lock").exists() {
        // Not a Cargo workspace — skipped as a true verdict.
        return BuildPreflightOutcome {
            ok: true,
            first_error_code: None,
            stderr_tail: "build preflight: not a Cargo workspace (skipped)".into(),
            timed_out: false,
        };
    }

    let start = Instant::now();
    let child = Command::new("cargo")
        .args(["check", "--message-format=short", "--quiet"])
        .env("CARGO_TERM_COLOR", "never")
        .env("CARGO_TERM_PROGRESS_WHEN", "never")
        .env("NO_COLOR", "1")
        .current_dir(path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let Ok(mut child) = child else {
        return BuildPreflightOutcome {
            ok: false,
            first_error_code: None,
            stderr_tail:
                "build preflight: failed to spawn `cargo check` (cargo not on PATH?). \
                 Disable AURA_BUILD_GATE to silence."
                    .into(),
            timed_out: false,
        };
    };

    use std::io::Read;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let stdout = drain(&mut child.stdout.take());
                let stderr = drain(&mut child.stderr.take());
                let combined = format!(
                    "{}\n{}",
                    String::from_utf8_lossy(&stdout),
                    String::from_utf8_lossy(&stderr)
                );
                return BuildPreflightOutcome {
                    ok: status.success(),
                    first_error_code: first_error_code(&combined),
                    stderr_tail: truncate_tail(&combined, 4_000),
                    timed_out: false,
                };
            }
            Ok(None) => {
                if start.elapsed() >= BUILD_PREFLIGHT_TIMEOUT {
                    let _ = child.kill();
                    return BuildPreflightOutcome {
                        ok: false,
                        first_error_code: None,
                        stderr_tail: format!(
                            "build preflight: `cargo check` exceeded {}s timeout and was killed",
                            BUILD_PREFLIGHT_TIMEOUT.as_secs()
                        ),
                        timed_out: true,
                    };
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(err) => {
                return BuildPreflightOutcome {
                    ok: false,
                    first_error_code: None,
                    stderr_tail: format!("build preflight: try_wait failed: {err}"),
                    timed_out: false,
                };
            }
        }
    }

    fn drain(handle: &mut Option<impl Read>) -> Vec<u8> {
        let mut buf = Vec::new();
        if let Some(h) = handle {
            let _ = h.read_to_end(&mut buf);
        }
        buf
    }
}

/// Convert a failing [`BuildPreflightOutcome`] into the matching
/// `AutomatonError::AgentExecution` so the orchestrator can plug the
/// verdict straight into the existing failure-handling path without
/// inventing a new variant. The returned message starts with the same
/// `build_preflight_failed:` discriminator the server-side gate uses
/// so dashboards / classifiers can recognise both sources uniformly.
#[must_use]
pub fn build_preflight_failure_to_error(outcome: &BuildPreflightOutcome) -> AutomatonError {
    let code = outcome
        .first_error_code
        .as_deref()
        .map_or_else(|| "unknown".to_string(), |c| format!("error[{c}]"));
    let message = if outcome.timed_out {
        "build_preflight_failed: `cargo check` exceeded the 90s timeout; \
         demoted task verdict to failure"
            .to_string()
    } else {
        format!(
            "build_preflight_failed: {code} surfaced by `cargo check`; \
             demoted task verdict to failure"
        )
    };
    AutomatonError::AgentExecution(message)
}

fn first_error_code(combined: &str) -> Option<String> {
    for line in combined.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("error[") {
            if let Some(end) = rest.find(']') {
                return Some(rest[..end].to_string());
            }
        }
    }
    None
}

fn truncate_tail(s: &str, limit: usize) -> String {
    if s.len() <= limit {
        return s.to_string();
    }
    let mut start = s.len().saturating_sub(limit);
    while start < s.len() && !s.is_char_boundary(start) {
        start += 1;
    }
    format!("... (truncated to last {limit} bytes)\n{}", &s[start..])
}

/// Structured hint attached to a `NeedsDecomposition` outcome so the
/// orchestrator (Phase 3, in aura-os) can auto-split a task that reached
/// implementation phase but produced no file operations. Empty/None fields
/// are expected when the validator cannot reliably recover the context.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct DecompositionHint {
    /// Unique paths the agent attempted to `write_file` / `edit_file`
    /// without ever producing a non-error `tool_result`.
    pub failed_paths: Vec<String>,
    /// Name of the most recent assistant-side tool_use block, if any.
    pub last_pending_tool_name: Option<String>,
    /// Short JSON summary of that tool_use's input (via
    /// `aura_compaction::summarize_write_input` when applicable).
    pub last_pending_tool_input_summary: Option<String>,
}

/// Validate an agent-task execution result. Returns:
/// - `Ok(exec)` when the task produced file ops or explicitly declared
///   no-changes-needed.
/// - `Err(AutomatonError::NeedsDecomposition { hint })` when the task
///   reached the implementing phase but produced no file ops — the caller
///   (or the Phase 3 orchestrator in aura-os) can consume the hint to
///   auto-split and retry.
/// - `Err(AutomatonError::AgentExecution(..))` for the classic
///   "completed-without-changes" case that never reached implementing.
pub(crate) fn validate_execution(
    exec: TaskExecutionResult,
) -> Result<TaskExecutionResult, AutomatonError> {
    if !exec.file_ops.is_empty() || exec.no_changes_needed {
        return Ok(exec);
    }

    if exec.reached_implementing {
        let hint = build_decomposition_hint(&exec.messages);
        return Err(AutomatonError::NeedsDecomposition { hint });
    }

    Err(AutomatonError::AgentExecution(
        "task completed without any file operations — completion not verified".into(),
    ))
}

/// Extract a best-effort `DecompositionHint` from the final message history
/// of a task that reached implementation phase without any file ops.
///
/// `failed_paths` = unique paths from write_file/edit_file tool_use blocks
/// whose tool_use id never produced a non-error tool_result.
/// `last_pending_tool_name` = name of the last ToolUse in the most recent
/// assistant message.
/// `last_pending_tool_input_summary` = short summary via
/// `aura_compaction::summarize_write_input` (when it applies) or the
/// raw JSON truncated to a reasonable length.
pub(crate) fn build_decomposition_hint(messages: &[Message]) -> DecompositionHint {
    if messages.is_empty() {
        return DecompositionHint::default();
    }

    let mut tool_uses: HashMap<String, (String, serde_json::Value)> = HashMap::new();
    let mut successful_ids: HashSet<String> = HashSet::new();

    for msg in messages {
        match msg.role {
            Role::Assistant => {
                for block in &msg.content {
                    if let ContentBlock::ToolUse { id, name, input } = block {
                        tool_uses.insert(id.clone(), (name.clone(), input.clone()));
                    }
                }
            }
            Role::User => {
                for block in &msg.content {
                    if let ContentBlock::ToolResult {
                        tool_use_id,
                        is_error,
                        ..
                    } = block
                    {
                        if !*is_error {
                            successful_ids.insert(tool_use_id.clone());
                        }
                    }
                }
            }
        }
    }

    let mut failed_paths: Vec<String> = Vec::new();
    let mut seen_paths: HashSet<String> = HashSet::new();
    for (id, (name, input)) in &tool_uses {
        if successful_ids.contains(id) {
            continue;
        }
        if !matches!(name.as_str(), "write_file" | "edit_file") {
            continue;
        }
        if let Some(path) = input.get("path").and_then(|v| v.as_str()) {
            if seen_paths.insert(path.to_string()) {
                failed_paths.push(path.to_string());
            }
        }
    }

    let (last_pending_tool_name, last_pending_tool_input_summary) = last_pending_tool_use(messages);

    DecompositionHint {
        failed_paths,
        last_pending_tool_name,
        last_pending_tool_input_summary,
    }
}

fn last_pending_tool_use(messages: &[Message]) -> (Option<String>, Option<String>) {
    let last_assistant = messages.iter().rev().find(|m| m.role == Role::Assistant);
    let Some(msg) = last_assistant else {
        return (None, None);
    };
    let last_tool_use = msg.content.iter().rev().find_map(|b| match b {
        ContentBlock::ToolUse { name, input, .. } => Some((name.clone(), input.clone())),
        _ => None,
    });
    let Some((name, input)) = last_tool_use else {
        return (None, None);
    };

    let summary = aura_compaction::summarize_write_input(&name, &input)
        .and_then(|v| serde_json::to_string(&v).ok())
        .or_else(|| serde_json::to_string(&input).ok())
        .map(|s| truncate_summary(&s, 240));

    (Some(name), summary)
}

fn truncate_summary(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let mut cut = max;
        while !s.is_char_boundary(cut) && cut > 0 {
            cut -= 1;
        }
        format!("{}…", &s[..cut])
    }
}

#[cfg(test)]
mod build_preflight_tests {
    use super::*;

    #[test]
    fn build_preflight_gate_enabled_honours_env_var() {
        let key = "AURA_BUILD_GATE";
        let original = std::env::var(key).ok();
        std::env::set_var(key, "true");
        assert!(build_preflight_gate_enabled());
        std::env::set_var(key, "0");
        assert!(!build_preflight_gate_enabled());
        std::env::remove_var(key);
        assert!(!build_preflight_gate_enabled());
        if let Some(value) = original {
            std::env::set_var(key, value);
        }
    }

    #[test]
    fn validate_build_preflight_skips_non_cargo_workspace() {
        let tmp = tempfile::tempdir().unwrap();
        let outcome = validate_build_preflight(tmp.path().to_str().unwrap());
        assert!(outcome.ok, "non-cargo workspace must short-circuit ok");
        assert!(outcome.stderr_tail.contains("not a Cargo workspace"));
    }

    #[test]
    fn validate_build_preflight_rejects_missing_workspace() {
        let outcome = validate_build_preflight("");
        assert!(!outcome.ok);
        assert!(outcome.stderr_tail.contains("workspace path is empty"));
    }

    #[test]
    fn build_preflight_failure_to_error_carries_discriminator() {
        let outcome = BuildPreflightOutcome {
            ok: false,
            first_error_code: Some("E0432".to_string()),
            stderr_tail: String::new(),
            timed_out: false,
        };
        let AutomatonError::AgentExecution(msg) = build_preflight_failure_to_error(&outcome)
        else {
            panic!("expected AgentExecution");
        };
        assert!(msg.starts_with("build_preflight_failed:"));
        assert!(msg.contains("error[E0432]"));
    }

    #[test]
    fn build_preflight_failure_to_error_handles_timeout() {
        let outcome = BuildPreflightOutcome {
            ok: false,
            first_error_code: None,
            stderr_tail: String::new(),
            timed_out: true,
        };
        let AutomatonError::AgentExecution(msg) = build_preflight_failure_to_error(&outcome)
        else {
            panic!("expected AgentExecution");
        };
        assert!(msg.contains("exceeded the 90s timeout"));
    }

    #[test]
    fn first_error_code_extracts_e_designator() {
        let combined = "warning: x\nerror[E0277]: trait bound\n";
        assert_eq!(first_error_code(combined).as_deref(), Some("E0277"));
        assert_eq!(first_error_code("clean"), None);
    }
}
