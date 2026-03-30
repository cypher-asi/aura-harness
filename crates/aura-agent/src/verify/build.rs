//! Build verification and auto-fix loop.
//!
//! Provides [`verify_and_fix_build`] which runs a build command, detects
//! failures, requests LLM-generated fixes, and iterates until the build
//! passes or retries are exhausted.

use std::collections::HashSet;
use std::path::Path;
use std::time::Instant;

use tracing::{info, warn};

use crate::file_ops::FileOp;

use super::common::apply_fix_and_record;
use super::error_types::BuildFixAttemptRecord;
use super::runner;
use super::signatures::{normalize_error_signature, parse_individual_error_signatures};
use super::test::run_and_handle_tests;
use super::utils::{
    all_errors_in_baseline, auto_correct_build_command, infer_default_build_command,
    rollback_to_snapshot, snapshot_modified_files,
};
use super::{emit, FixProvider, VerifyConfig, VerifyEvent};

/// Parameters for [`verify_and_fix_build`].
pub struct BuildVerifyParams<'a> {
    pub project_root: &'a Path,
    pub build_command: Option<&'a str>,
    pub test_command: Option<&'a str>,
    /// Label used in log messages (e.g. task title).
    pub task_label: &'a str,
    pub baseline_test_failures: &'a HashSet<String>,
    pub baseline_build_errors: &'a HashSet<String>,
    /// File ops from the initial execution, used to snapshot before fix loop.
    pub initial_file_ops: &'a [FileOp],
}

/// Result of [`verify_and_fix_build`].
pub struct BuildVerifyResult {
    pub fix_ops: Vec<FileOp>,
    pub build_passed: bool,
    pub attempts_used: u32,
    pub duplicate_bailouts: u32,
    pub fix_input_tokens: u64,
    pub fix_output_tokens: u64,
    pub last_stderr: String,
}

/// Run the build command once and return the set of pre-existing error
/// signatures for use as a baseline.
pub async fn capture_build_baseline(
    project_root: &Path,
    build_command: Option<&str>,
) -> HashSet<String> {
    let cmd = match build_command {
        Some(cmd) if !cmd.trim().is_empty() => cmd.to_string(),
        _ => match infer_default_build_command(project_root) {
            Some(cmd) => cmd,
            None => return HashSet::new(),
        },
    };
    match runner::run_build_command(project_root, &cmd, None).await {
        Ok(result) if !result.success => {
            let errors = parse_individual_error_signatures(&result.stderr);
            if !errors.is_empty() {
                info!(
                    count = errors.len(),
                    "captured {} pre-existing build error(s) as baseline",
                    errors.len()
                );
            }
            errors
        }
        _ => HashSet::new(),
    }
}

fn resolve_build_command(
    project_root: &Path,
    build_command: Option<&str>,
    event_tx: Option<&tokio::sync::mpsc::UnboundedSender<VerifyEvent>>,
) -> Option<String> {
    let cmd = match build_command {
        Some(cmd) if !cmd.trim().is_empty() => cmd.to_string(),
        _ => {
            if let Some(fallback) = infer_default_build_command(project_root) {
                info!(
                    command = %fallback,
                    "build_command missing; using inferred safe default for verification"
                );
                return Some(fallback);
            }
            emit(
                event_tx,
                VerifyEvent::BuildSkipped {
                    reason: "no build_command configured".into(),
                },
            );
            return None;
        }
    };
    let mut build_command = cmd;
    if let Some(corrected) = auto_correct_build_command(&build_command) {
        warn!(
            old = %build_command, new = %corrected,
            "eagerly rewriting server-starting build command"
        );
        build_command = corrected;
    }
    Some(build_command)
}

fn check_error_stagnation(
    task_label: &str,
    stderr: &str,
    prior_attempts: &[BuildFixAttemptRecord],
    attempt: u32,
) -> bool {
    let current_signature = normalize_error_signature(stderr);
    let consecutive_dupes = prior_attempts
        .iter()
        .rev()
        .take_while(|a| a.error_signature == current_signature)
        .count();
    if consecutive_dupes >= 2 {
        info!(
            task = %task_label, attempt,
            "same error pattern repeated {} times, aborting fix loop",
            consecutive_dupes + 1
        );
        return true;
    }
    false
}

/// Run the build/test verify-and-fix loop.
///
/// Iterates up to `config.max_build_fix_retries`, running the build command,
/// requesting LLM fixes on failure, and optionally running tests on success.
#[allow(clippy::too_many_lines)]
pub async fn verify_and_fix_build(
    params: &BuildVerifyParams<'_>,
    config: &VerifyConfig,
    fix_provider: &dyn FixProvider,
    event_tx: Option<&tokio::sync::mpsc::UnboundedSender<VerifyEvent>>,
) -> anyhow::Result<BuildVerifyResult> {
    let mut build_cmd =
        match resolve_build_command(params.project_root, params.build_command, event_tx) {
            Some(cmd) => cmd,
            None => {
                return Ok(BuildVerifyResult {
                    fix_ops: vec![],
                    build_passed: true,
                    attempts_used: 0,
                    duplicate_bailouts: 0,
                    fix_input_tokens: 0,
                    fix_output_tokens: 0,
                    last_stderr: String::new(),
                });
            }
        };

    let base_path = params.project_root;
    let mut fix_ops: Vec<FileOp> = Vec::new();
    let mut prior: Vec<BuildFixAttemptRecord> = Vec::new();
    let mut test_prior: Vec<BuildFixAttemptRecord> = Vec::new();
    let (mut dup_bail, mut inp_t, mut out_t) = (0u32, 0u64, 0u64);
    let mut last_stderr = String::new();
    let pre_fix_snapshots = snapshot_modified_files(base_path, params.initial_file_ops);

    for attempt in 1..=config.max_build_fix_retries {
        // --- run build ---
        let build_start = Instant::now();

        let (line_tx, mut line_rx) = tokio::sync::mpsc::unbounded_channel();
        if let Some(tx) = event_tx {
            let fwd = tx.clone();
            tokio::spawn(async move {
                while let Some(line) = line_rx.recv().await {
                    let _ = fwd.send(VerifyEvent::OutputDelta(line));
                }
            });
        } else {
            tokio::spawn(async move { while line_rx.recv().await.is_some() {} });
        }

        emit(
            event_tx,
            VerifyEvent::BuildStarted {
                command: build_cmd.clone(),
            },
        );

        let br = runner::run_build_command(base_path, &build_cmd, Some(line_tx)).await?;
        let dur = build_start.elapsed().as_millis() as u64;

        // --- timeout auto-correct ---
        if br.timed_out {
            if let Some(c) = auto_correct_build_command(&build_cmd) {
                warn!(old = %build_cmd, new = %c, "build command timed out, auto-correcting");
                build_cmd = c;
                continue;
            }
        }

        // --- success ---
        if br.success {
            emit(
                event_tx,
                VerifyEvent::BuildPassed {
                    command: build_cmd.clone(),
                    stdout: br.stdout.clone(),
                    duration_ms: dur,
                },
            );
            match params.test_command {
                Some(test_cmd) if !test_cmd.trim().is_empty() => {
                    let (tp, i, o) = run_and_handle_tests(
                        base_path,
                        test_cmd,
                        attempt,
                        params.baseline_test_failures,
                        fix_provider,
                        event_tx,
                        &mut test_prior,
                        &mut fix_ops,
                    )
                    .await?;
                    inp_t += i;
                    out_t += o;
                    if tp {
                        return Ok(BuildVerifyResult {
                            fix_ops,
                            build_passed: true,
                            attempts_used: attempt,
                            duplicate_bailouts: dup_bail,
                            fix_input_tokens: inp_t,
                            fix_output_tokens: out_t,
                            last_stderr: String::new(),
                        });
                    }
                    continue;
                }
                _ => {
                    return Ok(BuildVerifyResult {
                        fix_ops,
                        build_passed: true,
                        attempts_used: attempt,
                        duplicate_bailouts: dup_bail,
                        fix_input_tokens: inp_t,
                        fix_output_tokens: out_t,
                        last_stderr: String::new(),
                    });
                }
            }
        }

        // --- failure ---
        last_stderr = br.stderr.clone();
        emit(
            event_tx,
            VerifyEvent::BuildFailed {
                command: build_cmd.clone(),
                stdout: br.stdout.clone(),
                stderr: br.stderr.clone(),
                attempt,
                duration_ms: dur,
            },
        );

        if all_errors_in_baseline(params.baseline_build_errors, &br.stderr) {
            return Ok(BuildVerifyResult {
                fix_ops,
                build_passed: true,
                attempts_used: attempt,
                duplicate_bailouts: dup_bail,
                fix_input_tokens: inp_t,
                fix_output_tokens: out_t,
                last_stderr: String::new(),
            });
        }

        if attempt == config.max_build_fix_retries {
            info!(task = %params.task_label, "build still failing after max retries");
            return Ok(BuildVerifyResult {
                fix_ops,
                build_passed: false,
                attempts_used: attempt,
                duplicate_bailouts: dup_bail,
                fix_input_tokens: inp_t,
                fix_output_tokens: out_t,
                last_stderr,
            });
        }

        if check_error_stagnation(params.task_label, &br.stderr, &prior, attempt) {
            dup_bail += 1;
            rollback_to_snapshot(base_path, &pre_fix_snapshots).await;
            info!(task = %params.task_label, "rolled back files after stagnated fix loop");
            return Ok(BuildVerifyResult {
                fix_ops,
                build_passed: false,
                attempts_used: attempt,
                duplicate_bailouts: dup_bail,
                fix_input_tokens: inp_t,
                fix_output_tokens: out_t,
                last_stderr,
            });
        }

        // --- request and apply fix ---
        emit(event_tx, VerifyEvent::BuildFixAttempt { attempt });

        let (response, i, o) = fix_provider
            .request_fix(&build_cmd, &br.stderr, &br.stdout, &prior)
            .await?;
        inp_t += i;
        out_t += o;

        apply_fix_and_record(
            base_path,
            &response,
            attempt,
            &br.stderr,
            &mut prior,
            &mut fix_ops,
            "build-fix",
            fix_provider,
        )
        .await?;
    }

    Ok(BuildVerifyResult {
        fix_ops,
        build_passed: false,
        attempts_used: config.max_build_fix_retries,
        duplicate_bailouts: dup_bail,
        fix_input_tokens: inp_t,
        fix_output_tokens: out_t,
        last_stderr,
    })
}
