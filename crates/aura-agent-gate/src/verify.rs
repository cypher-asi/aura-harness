//! Running a block's checks.
//!
//! Checks run through the `run_command` tool against the calling
//! agent's tool context, so they get the same sandbox, environment
//! scrubbing and command policy as the coordinator's own commands.

use aura_tools::{CmdRunTool, Tool, ToolContext};

use crate::block::{BlockSpec, CheckOutcome, VerifyLevel};

/// Output kept from each check.
const OUTPUT_TAIL: usize = 2_000;

/// What a block's checks showed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verification {
    /// Highest level whose checks all passed.
    pub level: VerifyLevel,
    /// Every command run, in order; stops at the first failure.
    pub checks: Vec<CheckOutcome>,
    /// First failure.
    pub failure: Option<String>,
}

/// Run `spec`'s build, checks and acceptance commands in that order,
/// stopping at the first failure.
pub async fn run_checks(ctx: &ToolContext, spec: &BlockSpec, timeout_ms: u64) -> Verification {
    let stages = [
        (
            VerifyLevel::Built,
            "build",
            spec.build.iter().cloned().collect::<Vec<_>>(),
        ),
        (VerifyLevel::Tested, "check", spec.checks.clone()),
        (VerifyLevel::Accepted, "acceptance", spec.acceptance.clone()),
    ];
    let mut level = VerifyLevel::Ran;
    let mut checks = Vec::new();
    for (stage_level, stage, commands) in stages {
        if commands.is_empty() {
            continue;
        }
        for command in commands {
            let outcome = run_check(ctx, stage, &command, timeout_ms).await;
            let passed = outcome.passed;
            checks.push(outcome);
            if !passed {
                // Acceptance commands stay hidden from later workers.
                let failure = if stage == "acceptance" {
                    "an acceptance check failed".to_string()
                } else {
                    format!("{stage} failed: {command}")
                };
                return Verification {
                    level,
                    checks,
                    failure: Some(failure),
                };
            }
        }
        level = stage_level;
    }
    Verification {
        level,
        checks,
        failure: None,
    }
}

/// Run one shell command through `run_command`.
pub async fn run_check(
    ctx: &ToolContext,
    stage: &str,
    command: &str,
    timeout_ms: u64,
) -> CheckOutcome {
    let args = serde_json::json!({
        "shell_script": command,
        "allow_shell": true,
        "timeout_ms": timeout_ms,
    });
    match CmdRunTool.execute(ctx, args).await {
        Ok(result) => {
            let mut output = String::from_utf8_lossy(&result.stdout).into_owned();
            let stderr = String::from_utf8_lossy(&result.stderr);
            if !stderr.trim().is_empty() {
                if !output.is_empty() {
                    output.push('\n');
                }
                output.push_str(&stderr);
            }
            CheckOutcome {
                stage: stage.into(),
                command: command.into(),
                passed: result.ok && result.exit_code.unwrap_or(0) == 0,
                exit_code: result.exit_code,
                output: tail(&output, OUTPUT_TAIL),
            }
        }
        Err(e) => CheckOutcome {
            stage: stage.into(),
            command: command.into(),
            passed: false,
            exit_code: None,
            output: e.to_string(),
        },
    }
}

/// Last `max` bytes of `text`, on a character boundary.
#[must_use]
pub fn tail(text: &str, max: usize) -> String {
    let text = text.trim_end();
    if text.len() <= max {
        return text.to_string();
    }
    let mut start = text.len() - max;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    format!("…{}", &text[start..])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::tests::edit;
    use aura_tools::{Sandbox, ToolConfig};

    fn ctx(dir: &std::path::Path) -> ToolContext {
        let mut config = ToolConfig::default();
        config.command.enabled = true;
        config.command.allow_shell = true;
        config.command.bypass_allowlists = true;
        ToolContext::new(Sandbox::new(dir).unwrap(), config)
    }

    #[tokio::test]
    async fn stages_run_in_order_and_stop_at_the_first_failure() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("ok"), "1").unwrap();
        let ctx = ctx(dir.path());
        let mut spec = edit("a");
        spec.build = Some("test -f ok".into());
        spec.checks = vec!["true".into(), "echo boom; exit 3".into()];
        spec.acceptance = vec!["true".into()];
        let v = run_checks(&ctx, &spec, 10_000).await;
        assert_eq!(v.level, VerifyLevel::Built);
        assert_eq!(v.checks.len(), 3);
        assert_eq!(v.checks[2].exit_code, Some(3));
        assert!(v.checks[2].output.contains("boom"));
        assert_eq!(
            v.failure.as_deref(),
            Some("check failed: echo boom; exit 3")
        );

        spec.checks.pop();
        let v = run_checks(&ctx, &spec, 10_000).await;
        assert_eq!(v.level, VerifyLevel::Accepted);
        assert!(v.failure.is_none());
    }

    #[test]
    fn tail_keeps_the_end() {
        assert_eq!(tail("abc", 10), "abc");
        assert_eq!(tail("abcdef", 3), "…def");
        assert_eq!(tail("ééé", 3), "…é");
    }
}
