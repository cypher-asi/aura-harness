use std::fmt::Write;
use std::path::Path;

use super::{ProjectInfo, SessionInfo, SpecInfo, TaskInfo};
use crate::build::{classify_build_errors, error_category_guidance, ErrorCategory};
use crate::file_ops::{self, ErrorReferences, StubReport};
use crate::verify::error_types::{parse_error_references, BuildFixAttemptRecord};

pub struct BuildFixPromptParams<'a> {
    pub project: &'a ProjectInfo<'a>,
    pub spec: &'a SpecInfo<'a>,
    pub task: &'a TaskInfo<'a>,
    pub session: &'a SessionInfo<'a>,
    pub codebase_snapshot: &'a str,
    pub build_command: &'a str,
    pub stderr: &'a str,
    pub stdout: &'a str,
    pub prior_notes: &'a str,
    pub prior_attempts: &'a [BuildFixAttemptRecord],
}

#[must_use]
pub fn build_fix_prompt_with_history(params: &BuildFixPromptParams<'_>) -> String {
    let mut prompt = String::new();

    prompt.push_str(&format_fix_header(
        params.project,
        params.spec,
        params.task,
        params.session,
        params.prior_notes,
        params.prior_attempts,
    ));

    let mut categories = classify_build_errors(params.stderr);
    let error_refs = parse_error_references(params.stderr);
    let resolved_context =
        file_ops::resolve_error_context(Path::new(params.project.folder_path), &error_refs);

    detect_api_hallucination(&error_refs, &mut categories);

    let guidance = error_category_guidance(&categories);

    prompt.push_str(&format_fix_body(
        params.build_command,
        params.stderr,
        params.stdout,
        &guidance,
        &resolved_context,
        &error_refs,
        params.project.folder_path,
        params.codebase_snapshot,
    ));

    prompt
}

fn format_fix_header(
    project: &ProjectInfo<'_>,
    spec: &SpecInfo<'_>,
    task: &TaskInfo<'_>,
    session: &SessionInfo<'_>,
    prior_notes: &str,
    prior_attempts: &[BuildFixAttemptRecord],
) -> String {
    let mut header = String::new();

    let _ = write!(
        header,
        "# Project: {}\n{}\n\n",
        project.name, project.description
    );
    let _ = write!(
        header,
        "# Spec: {}\n{}\n\n",
        spec.title, spec.markdown_contents
    );
    let _ = write!(header, "# Task: {}\n{}\n\n", task.title, task.description);

    if !session.summary_of_previous_context.is_empty() {
        let _ = write!(
            header,
            "# Previous Context Summary\n{}\n\n",
            session.summary_of_previous_context
        );
    }

    if !prior_notes.is_empty() {
        let _ = write!(
            header,
            "# Notes from Initial Implementation\n{prior_notes}\n\n",
        );
    }

    if !prior_attempts.is_empty() {
        header.push_str("# Previous Fix Attempts (all failed)\nThe following fixes were already attempted and did NOT solve the problem. You MUST try a fundamentally different approach.\n\n");
        for (i, attempt) in prior_attempts.iter().enumerate() {
            let _ = writeln!(header, "## Attempt {}", i + 1);
            if !attempt.changes_summary.is_empty() {
                let _ = write!(header, "Changes made:\n{}\n", attempt.changes_summary);
            } else if !attempt.files_changed.is_empty() {
                header.push_str("Files changed:\n");
                for f in &attempt.files_changed {
                    let _ = writeln!(header, "- {f}");
                }
            }
            let _ = write!(header, "Error:\n```\n{}\n```\n\n", attempt.stderr);
        }
    }

    header
}

fn detect_api_hallucination(error_refs: &ErrorReferences, categories: &mut Vec<ErrorCategory>) {
    let mut type_counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for (t, _) in &error_refs.methods_not_found {
        *type_counts.entry(t.as_str()).or_insert(0) += 1;
    }
    if type_counts.values().any(|&c| c >= 3) || error_refs.wrong_arg_counts.len() >= 3 {
        categories.push(ErrorCategory::RustApiHallucination);
    }
}

#[allow(clippy::too_many_arguments)]
fn format_fix_body(
    build_command: &str,
    stderr: &str,
    stdout: &str,
    guidance: &str,
    resolved_context: &str,
    error_refs: &ErrorReferences,
    folder_path: &str,
    codebase_snapshot: &str,
) -> String {
    let mut body = String::new();

    let _ = write!(
        body,
        "# Build/Test Verification FAILED\n\
         The command `{build_command}` failed after the previous file operations were applied.\n\
         You MUST fix ALL errors below.\n\n",
    );

    if !guidance.is_empty() {
        let _ = write!(
            body,
            "## Error Analysis & Required Fix Strategy\n{guidance}\n",
        );
    }

    let truncated_stderr = truncate_prompt_output(stderr, 8000);
    let _ = write!(body, "## stderr\n```\n{truncated_stderr}\n```\n\n");

    if !stdout.is_empty() {
        let truncated_stdout = truncate_prompt_output(stdout, 4000);
        let _ = write!(body, "## stdout\n```\n{truncated_stdout}\n```\n\n");
    }

    if error_refs.methods_not_found.len() > 3 {
        body.push_str(
            "WARNING: You are calling 3+ methods that do not exist. You MUST use ONLY \
             the methods listed in the \"Actual API Reference\" section below. Do NOT \
             invent or guess method names.\n\n",
        );
    }

    if !resolved_context.is_empty() {
        body.push_str(resolved_context);
        body.push('\n');
    }

    let error_source_files = file_ops::resolve_error_source_files(
        Path::new(folder_path),
        error_refs,
        file_ops::ERROR_SOURCE_BUDGET,
    );
    if !error_source_files.is_empty() {
        body.push_str(&error_source_files);
        body.push('\n');
    }

    if !codebase_snapshot.is_empty() {
        let _ = write!(
            body,
            "# Current Codebase Files (after previous changes)\n{codebase_snapshot}\n",
        );
    }

    body
}

fn truncate_prompt_output(s: &str, max_chars: usize) -> String {
    if s.len() <= max_chars {
        return s.to_string();
    }
    let half = max_chars / 2;
    let start = &s[..half];
    let end = &s[s.len() - half..];
    format!(
        "{start}\n\n... (truncated {0} bytes) ...\n\n{end}",
        s.len() - max_chars
    )
}

/// Build a prompt that tells the agent to replace stub/placeholder code with
/// real implementations.
#[must_use]
pub fn build_stub_fix_prompt(stub_reports: &[StubReport]) -> String {
    let mut prompt = String::from(
        "STOP: Your implementation compiles but contains stub/placeholder code that must be \
         filled in. The following locations have incomplete implementations:\n\n",
    );

    for report in stub_reports {
        let _ = write!(
            prompt,
            "- {}:{} -- {}\n  ```\n  {}\n  ```\n\n",
            report.path, report.line, report.pattern, report.context,
        );
    }

    prompt.push_str(
        "Replace ALL stubs with real, working implementations. Read the spec and codebase \
         to understand what each function should do, then implement it fully.\n\
         Do NOT use todo!(), unimplemented!(), Default::default() as a placeholder, or \
         ignore function parameters with _ prefixes.\n\
         After fixing, verify the build still passes, then call task_done.\n",
    );

    prompt
}

#[cfg(test)]
mod tests;
