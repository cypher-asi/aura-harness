//! Task context budget management.
//!
//! Caps the task context string to a configurable character budget and
//! summarizes the work log for inclusion in it.

use aura_config::MAX_WORK_LOG_TASK_CONTEXT;

pub fn cap_bootstrap_task_context(task_context: &mut String) {
    let budget = aura_config::agent().prompts.bootstrap_context_chars;
    if budget == 0 || task_context.len() <= budget {
        return;
    }

    cap_task_context(task_context, budget);
    if task_context.len() <= budget {
        return;
    }

    let marker = "\n\n[... task context truncated before LLM routing; use read_file/search_code/get_task_context for additional details ...]\n";
    let marker_len = marker.len().min(budget);
    let keep = budget.saturating_sub(marker_len);
    let cut = task_context
        .char_indices()
        .take_while(|(idx, ch)| idx + ch.len_utf8() <= keep)
        .last()
        .map_or(0, |(idx, ch)| idx + ch.len_utf8());
    task_context.truncate(cut);
    task_context.push_str(marker);
}

/// Build a summary of the work log for inclusion in task context.
pub fn build_work_log_summary(work_log: &[String]) -> String {
    if work_log.is_empty() {
        return String::new();
    }
    let mut summary = work_log.join("\n---\n");
    if summary.len() > MAX_WORK_LOG_TASK_CONTEXT {
        summary.truncate(MAX_WORK_LOG_TASK_CONTEXT);
        summary.push_str("\n... (truncated) ...");
    }
    summary
}

// ---------------------------------------------------------------------------
// Context budget management
// ---------------------------------------------------------------------------

/// Trim `task_context` to at most `budget` characters by progressively
/// removing lower-priority sections (codebase snapshot first, then dep API,
/// then workspace map), preserving the core task description and spec.
const CAP_SECTIONS: &[&str] = &[
    "\n# Current Codebase Files\n",
    "\n# Dependency API Surface\n",
    "\n# Workspace Structure\n",
    "\n# Type Definitions Referenced in Task\n",
];

pub fn cap_task_context(task_context: &mut String, budget: usize) {
    if task_context.len() <= budget {
        return;
    }

    for section_header in CAP_SECTIONS {
        if task_context.len() <= budget {
            return;
        }
        if let Some(start) = task_context.find(section_header) {
            let next_section = task_context[start + section_header.len()..]
                .find("\n# ")
                .map(|pos| start + section_header.len() + pos);
            let end = next_section.unwrap_or(task_context.len());

            let section_len = end - start;
            let overshoot = task_context.len().saturating_sub(budget);

            if overshoot >= section_len {
                task_context.replace_range(start..end, "");
            } else {
                let keep = section_len - overshoot;
                let trim_start = start + keep;
                task_context.replace_range(
                    trim_start..end,
                    "\n... (truncated to fit context budget) ...\n",
                );
            }
        }
    }

    if task_context.len() > budget {
        task_context.truncate(budget);
        task_context.push_str("\n... (context truncated) ...\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_work_log_summary_empty() {
        assert_eq!(build_work_log_summary(&[]), "");
    }

    #[test]
    fn build_work_log_summary_joins_entries() {
        let log = vec!["Task 1 done".into(), "Task 2 done".into()];
        let summary = build_work_log_summary(&log);
        assert!(summary.contains("Task 1 done"));
        assert!(summary.contains("---"));
        assert!(summary.contains("Task 2 done"));
    }

    #[test]
    fn build_work_log_summary_truncates_long_input() {
        let log: Vec<String> = (0..500)
            .map(|i| format!("Entry {i}: some work done here"))
            .collect();
        let summary = build_work_log_summary(&log);
        assert!(summary.len() <= MAX_WORK_LOG_TASK_CONTEXT + 30);
        assert!(summary.contains("(truncated)"));
    }

    #[test]
    fn cap_task_context_within_budget_unchanged() {
        let mut ctx = "Short context".to_string();
        let original = ctx.clone();
        cap_task_context(&mut ctx, 1000);
        assert_eq!(ctx, original);
    }

    #[test]
    fn cap_task_context_trims_codebase_section_first() {
        let mut ctx = String::new();
        ctx.push_str("# Task\nDo something\n");
        ctx.push_str("\n# Current Codebase Files\n");
        ctx.push_str(&"x".repeat(5000));
        ctx.push_str("\n# Dependency API Surface\n");
        ctx.push_str("dep info here");

        let original_len = ctx.len();
        let budget = 200;
        cap_task_context(&mut ctx, budget);
        assert!(ctx.len() < original_len);
        assert!(!ctx.contains(&"x".repeat(4000)));
        assert!(ctx.contains("truncated"));
    }

    #[test]
    fn cap_task_context_hard_truncate_last_resort() {
        let mut ctx = "x".repeat(10_000);
        cap_task_context(&mut ctx, 500);
        assert!(ctx.len() <= 550);
        assert!(ctx.contains("(context truncated)"));
    }

    #[test]
    fn cap_bootstrap_task_context_applies_small_routing_budget() {
        let mut ctx = String::new();
        ctx.push_str("# Task\nFix the issue\n");
        ctx.push_str("\n# Current Codebase Files\n");
        ctx.push_str(&"x".repeat(20_000));

        cap_bootstrap_task_context(&mut ctx);

        assert!(
            ctx.len() <= aura_config::DEFAULT_BOOTSTRAP_TASK_CONTEXT_CHARS,
            "expected bootstrap context <= {}, got {}",
            aura_config::DEFAULT_BOOTSTRAP_TASK_CONTEXT_CHARS,
            ctx.len()
        );
        assert!(ctx.contains("# Task"));
    }
}
