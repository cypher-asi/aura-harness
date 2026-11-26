//! Build command execution, output enrichment and error counting.

use std::path::Path;

use super::{handlers, TaskToolExecutor};
use crate::types::{AutoBuildResult, BuildBaseline};
use crate::verify::infer_default_build_command;

/// Merge build stdout/stderr into one diagnostic blob.
#[must_use]
pub fn combine_build_streams(stdout: &str, stderr: &str) -> String {
    let mut output = String::new();
    if !stdout.is_empty() {
        output.push_str(stdout);
    }
    if !stderr.is_empty() {
        if !output.is_empty() {
            output.push('\n');
        }
        output.push_str(stderr);
    }
    output
}

/// Count compiler error blocks in build output for auto-build messages.
#[must_use]
pub fn count_build_errors(output: &str) -> usize {
    let sigs = BuildBaseline::extract_signatures(output);
    if !sigs.is_empty() {
        return sigs.len();
    }
    if looks_like_compiler_errors(output) {
        1
    } else {
        0
    }
}

/// Run the project build command once. Returns `Ok(())` when there is no
/// configured command or the build succeeds; `Err` carries enriched stderr
/// for operator/agent consumption.
pub async fn run_project_build_check(
    project_folder: &str,
    build_command: Option<&str>,
) -> Result<(), String> {
    let project_root = Path::new(project_folder);
    let cmd = build_command
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .or_else(|| infer_default_build_command(project_root));
    let Some(cmd) = cmd else {
        return Ok(());
    };

    let result = match crate::verify::run_build_command(project_root, &cmd, None, None).await {
        Ok(r) => r,
        Err(e) => {
            return Err(format!("failed to run build command `{cmd}`: {e}"));
        }
    };
    if result.success {
        return Ok(());
    }

    let raw = combine_build_streams(&result.stdout, &result.stderr);
    let pf = project_folder.to_string();
    let raw_for_fallback = raw.clone();
    let enriched =
        tokio::task::spawn_blocking(move || handlers::enrich_compiler_output_sync(&pf, &raw))
            .await
            .unwrap_or_else(|e| {
                tracing::warn!("spawn_blocking panicked in enrich_compiler_output: {e}");
                raw_for_fallback
            });
    Err(format!(
        "Build check failed (`{cmd}`). Fix all errors before completing the task.\n\n{enriched}"
    ))
}

/// Check if build output looks like compiler errors (Rust or TypeScript).
pub(crate) fn looks_like_compiler_errors(output: &str) -> bool {
    let has_rust_errors = output.contains("error[E") && output.contains("-->");
    let has_generic_errors = output.contains("error:") && output.contains("-->");
    let has_ts_errors = output.contains("TS2") && output.contains("error TS");
    has_rust_errors || has_generic_errors || has_ts_errors
}

impl TaskToolExecutor {
    /// Body of [`crate::types::AgentToolExecutor::auto_build_check`].
    pub(super) async fn run_auto_build_check(&self) -> Option<AutoBuildResult> {
        let project_root = Path::new(&self.project_folder);
        let cmd = self
            .build_command
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .map(String::from)
            .or_else(|| infer_default_build_command(project_root))?;

        self.emit_text(
            aura_context_prompts::model_messages::auto_build::auto_build_status_line(&cmd),
        );

        match crate::verify::run_build_command(project_root, &cmd, None, None).await {
            Ok(result) => {
                let output = combine_build_streams(&result.stdout, &result.stderr);
                Some(self.auto_build_result(result.success, output).await)
            }
            Err(e) => {
                tracing::warn!(error = %e, "auto-build check failed to execute");
                None
            }
        }
    }

    /// Enrich failing build output and count its errors.
    async fn auto_build_result(&self, success: bool, output: String) -> AutoBuildResult {
        let output = if success {
            output
        } else {
            let pf = self.project_folder.clone();
            tokio::task::spawn_blocking(move || handlers::enrich_compiler_output_sync(&pf, &output))
                .await
                .unwrap_or_else(|e| {
                    tracing::warn!("spawn_blocking panicked in enrich_compiler_output: {e}");
                    String::new()
                })
        };
        let error_count = if success {
            0
        } else {
            count_build_errors(&output)
        };
        AutoBuildResult {
            success,
            output,
            error_count,
        }
    }
}
