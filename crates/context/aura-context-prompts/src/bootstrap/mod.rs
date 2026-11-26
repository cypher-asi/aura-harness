//! Bootstrap user-message construction for an agentic task run.
//!
//! Phase 2 lifted this out of `aura-agent/src/prompts/context.rs`. The
//! function is render-only: it threads the project / spec / task /
//! session descriptors (plus an optional pre-resolved enrichment
//! block) into a single user-message string. The shared header logic
//! lives in [`header`] so the fix prompt can reuse it.

use crate::descriptors::{ProjectInfo, SessionInfo, SpecInfo, TaskInfo};

pub mod header;

fn bootstrap_spec_byte_budget() -> usize {
    aura_config::agent().prompts.bootstrap_spec_bytes
}

fn bootstrap_should_strip_code_fences() -> bool {
    aura_config::agent().prompts.bootstrap_strip_code_fences
}

/// Strip fenced code blocks (```...```) from a markdown string,
/// replacing each one with a `[code example: <N> bytes elided to fit
/// body cap]` marker. Preserves all surrounding prose so the agent
/// retains the human-readable explanation. Indentation-based code
/// blocks (4-space indent) are left untouched on purpose: they are
/// rare in our specs / task descriptions and harder to detect
/// reliably.
fn strip_fenced_code_blocks(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_fence = false;
    let mut fence_buf = String::new();
    for line in input.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\r', '\n']);
        let is_fence = trimmed.trim_start().starts_with("```");
        if !in_fence && is_fence {
            in_fence = true;
            fence_buf.clear();
            continue;
        }
        if in_fence && is_fence {
            in_fence = false;
            let elided_bytes = fence_buf.len();
            out.push_str(&format!(
                "[code example: {elided_bytes} bytes elided to fit body cap; \
                 read the source files directly with `read_file` if needed]\n"
            ));
            fence_buf.clear();
            continue;
        }
        if in_fence {
            fence_buf.push_str(line);
        } else {
            out.push_str(line);
        }
    }
    if in_fence {
        out.push_str("```\n");
        out.push_str(&fence_buf);
    }
    out
}

fn append_spec_section(ctx: &mut String, spec: &SpecInfo<'_>, budget: usize) {
    if budget == 0 || spec.markdown_contents.is_empty() {
        ctx.push_str(&format!("# Spec: {}\n\n", spec.title));
        return;
    }
    let sanitized;
    let body: &str = if bootstrap_should_strip_code_fences() {
        sanitized = strip_fenced_code_blocks(spec.markdown_contents);
        &sanitized
    } else {
        spec.markdown_contents
    };
    if body.len() <= budget {
        ctx.push_str(&format!("# Spec: {}\n{body}\n\n", spec.title));
        return;
    }
    let cut = body
        .char_indices()
        .take_while(|(i, _)| *i < budget)
        .last()
        .map_or(0, |(i, c)| i + c.len_utf8());
    let kept = &body[..cut];
    let omitted = body.len() - cut;
    ctx.push_str(&format!(
        "# Spec: {}\n{kept}\n\n[... spec truncated to fit body cap; omitted {omitted} bytes. \
         The task description below carries the actionable content for this turn.]\n\n",
        spec.title
    ));
}

fn task_description_for_bootstrap(task: &TaskInfo<'_>) -> String {
    if bootstrap_should_strip_code_fences() {
        strip_fenced_code_blocks(task.description)
    } else {
        task.description.to_string()
    }
}

/// What [`build_agentic_task_context`] renders into a task run's first
/// user message.
#[derive(Debug, Clone, Copy)]
pub struct BootstrapCtx<'a> {
    pub project: &'a ProjectInfo<'a>,
    pub spec: &'a SpecInfo<'a>,
    pub task: &'a TaskInfo<'a>,
    pub session: &'a SessionInfo<'a>,
    /// Finished predecessor tasks, summarized under
    /// `# Completed Predecessor Tasks` when non-empty.
    pub completed_deps: &'a [TaskInfo<'a>],
    /// The tasks this session completed so far, rendered under
    /// `# Session Progress` when non-empty.
    pub work_log_summary: &'a str,
    /// 0-indexed retry counter — 0 for the first run, 1+ for retries.
    pub attempt: u32,
    /// Pre-resolved paths/symbols/file-heads markdown produced by the
    /// [`crate::enrichment`] module via the agent-side
    /// `prompt_resolve::resolve_hints` orchestrator. It is only spliced
    /// when `attempt == 0` and the block is non-empty: on retries, Phase
    /// 5's decomposition will provide a different, narrower context and
    /// the resolve cost is wasted on re-runs of the same task.
    pub enrichment_block: Option<&'a str>,
}

/// Build the initial user-message context for an agentic task run.
#[must_use]
pub fn build_agentic_task_context(input: BootstrapCtx<'_>) -> String {
    let mut ctx = String::new();
    ctx.push_str(&format!(
        "# Project: {}\n{}\n\n",
        input.project.name, input.project.description
    ));
    append_spec_section(&mut ctx, input.spec, bootstrap_spec_byte_budget());
    let task_desc = task_description_for_bootstrap(input.task);
    ctx.push_str(&format!("# Task: {}\n{task_desc}\n\n", input.task.title));

    if input.attempt == 0 {
        if let Some(block) = input.enrichment_block {
            let trimmed = block.trim();
            if !trimmed.is_empty() {
                ctx.push_str(trimmed);
                ctx.push_str("\n\n");
            }
        }
    }

    if !input.session.summary_of_previous_context.is_empty() {
        ctx.push_str(&format!(
            "# Previous Context Summary\n{}\n\n",
            input.session.summary_of_previous_context
        ));
    }
    if !input.task.execution_notes.is_empty() {
        ctx.push_str(&format!(
            "# Notes from Prior Attempts\n{}\n\n",
            input.task.execution_notes
        ));
    }

    if !input.completed_deps.is_empty() {
        ctx.push_str("# Completed Predecessor Tasks\n");
        ctx.push_str(&format_completed_deps(input.completed_deps));
        ctx.push('\n');
    }

    let work_log_summary = input.work_log_summary;
    if !work_log_summary.is_empty() {
        ctx.push_str(&format!(
            "# Session Progress (tasks completed so far)\n{work_log_summary}\n\n\
             If this task's work was already done by a prior task, call task_done with \
             `no_changes_needed: true` instead of re-implementing.\n\n"
        ));
    }

    ctx.push_str("Make the changes this task requires, then call task_done.\n");

    ctx
}

fn format_completed_deps(completed_deps: &[TaskInfo<'_>]) -> String {
    let mut output = String::new();
    let mut dep_budget = 5_000usize;
    for dep in completed_deps {
        let files_list = dep
            .files_changed
            .iter()
            .map(|fc| format!("{} ({})", fc.path, fc.op))
            .collect::<Vec<_>>()
            .join(", ");
        let section = format!(
            "## {}\n{}\nFiles: {}\n\n",
            dep.title, dep.execution_notes, files_list,
        );
        if section.len() > dep_budget {
            break;
        }
        dep_budget -= section.len();
        output.push_str(&section);
    }
    output
}

#[cfg(test)]
mod tests;
