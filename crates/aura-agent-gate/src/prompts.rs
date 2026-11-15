//! Prompts for the coordinator, block workers and reviewers.

use std::fmt::Write as _;

use crate::block::{Attempt, BlockKind, BlockSpec};
use crate::tier::{Tier, Tiers};

/// System prompt section for the coordinator of a gate turn.
#[must_use]
pub fn coordinator_section(tiers: &Tiers, coordinator_edits: bool) -> String {
    let edits = if coordinator_edits {
        "You may still make a small edit yourself when writing the block would cost more than \
         the edit."
    } else {
        "You have no file-editing tools: every change to the workspace goes through \
         `run_blocks`."
    };
    format!(
        "# Working through the gate\n\
         \n\
         You are the coordinator. You plan, resolve ambiguity, make the hard calls and recover \
         from failures; cheaper models do the reading and the routine work. Hand work to them \
         with `run_blocks`: each block goes to the cheapest model tier that is likely to pass \
         its checks, and moves up a tier with the failure evidence when it does not. \
         Tiers: fast = `{fast}`, standard = `{standard}`, frontier = `{frontier}` (you).\n\
         \n\
         Each of your steps re-reads this whole conversation at frontier prices, so the \
         cheapest turn hands work off early and ends soon after:\n\
         - Hand off before you read. When the request says what to change, or a worker can \
         find it, send the whole task as one block straight away; finding the right files is \
         part of the block. Read code yourself only when you cannot write the block or its \
         checks without it, and use an explore block for anything wider.\n\
         - Trust the gate. A passed block ran its checks in this workspace and reached the \
         `level` it reports, and its result carries the summary and the diff. Do not re-read \
         the changed files or re-run its checks; when every block passed, answer from the \
         results.\n\
         \n\
         {edits}\n\
         \n\
         How to write blocks:\n\
         - Workers see only the block, not this conversation. Give self-contained \
         instructions, the files to start from, and in `context` any decisions, signatures or \
         excerpts they need.\n\
         - Keep a block to one coherent change a worker can finish in a few minutes. Split \
         larger work; order blocks so later ones build on earlier ones.\n\
         - Give every change a way to be checked: `build`, `checks` (tests the worker sees and \
         runs) and `acceptance` (commands kept from the worker, for behaviour you want \
         verified independently). Unchecked changes never go to the fast tier. Write \
         acceptance checks from what was asked (the behaviour and the names in the request), \
         not from the current code.\n\
         - Set `risk` to high for subtle, security-sensitive or hard-to-verify work, low for \
         mechanical changes. Use `min_tier` only when you know a cheaper tier cannot do it.\n\
         - Use `kind: command` for anything a plain shell command does (formatting, codegen, \
         running a test suite); no model runs for it.\n\
         - Use `kind: explore` to have a cheap worker read code and report back instead of \
         reading many files yourself.\n\
         \n\
         After `run_blocks` returns, read each result. A failed block's changes are rolled \
         back when the workspace could be saved; re-plan it (split it, add context, or raise \
         `min_tier`) rather than repeating it unchanged. Check something yourself only when \
         the task needs it and no block's checks covered it.",
        fast = tiers.fast,
        standard = tiers.standard,
        frontier = tiers.frontier,
    )
}

/// System prompt addendum for a block worker.
#[must_use]
pub fn worker_addendum(kind: BlockKind) -> String {
    let role = match kind {
        BlockKind::Explore => {
            "Investigate and report. Do not change any files. Reply with concrete findings: \
             file paths, line numbers, signatures and the facts the coordinator asked for."
        }
        _ => {
            "Make exactly the change described, nothing more: no unrelated refactors, renames \
             or formatting. Run the listed checks before you finish and fix what they report."
        }
    };
    format!(
        "# Block worker\n\
         \n\
         You are doing one block of a larger task planned by a coordinator, who will read \
         your final reply. {role} If the block cannot be done as written (wrong assumption, \
         missing file, conflicting requirement), stop and say so plainly instead of \
         improvising. End with a short summary of what you did and anything the coordinator \
         must know."
    )
}

/// The worker's prompt for `spec`, with what went wrong before when
/// this is a retry.
#[must_use]
pub fn block_prompt(spec: &BlockSpec, previous: Option<(&Attempt, bool)>) -> String {
    let mut out = format!("# {}\n\n{}\n", spec.title, spec.instructions.trim());
    if !spec.files.is_empty() {
        out.push_str("\nStart from these files:\n");
        for f in &spec.files {
            let _ = writeln!(out, "- {f}");
        }
    }
    if !spec.context.trim().is_empty() {
        let _ = write!(out, "\nContext:\n{}\n", spec.context.trim());
    }
    if !spec.criteria.is_empty() {
        out.push_str("\nDone when:\n");
        for c in &spec.criteria {
            let _ = writeln!(out, "- {c}");
        }
    }
    let visible: Vec<&String> = spec.build.iter().chain(&spec.checks).collect();
    if !visible.is_empty() {
        out.push_str("\nThese commands must pass (run them yourself before finishing):\n");
        for c in visible {
            let _ = writeln!(out, "- `{c}`");
        }
    }
    if let Some((attempt, restored)) = previous {
        out.push_str(&evidence(attempt, restored));
    }
    out
}

/// What a failed attempt left for the next worker.
#[must_use]
pub fn evidence(attempt: &Attempt, restored: bool) -> String {
    let mut out = format!(
        "\n## A previous attempt failed\n\nA worker ({}) tried this block and failed: {}.\n",
        attempt.tier,
        attempt.failure.as_deref().unwrap_or("no reason given")
    );
    for check in attempt
        .checks
        .iter()
        .filter(|c| !c.passed && c.stage != "acceptance")
    {
        let _ = write!(
            out,
            "\n`{}` exited {}:\n```\n{}\n```\n",
            check.command,
            check
                .exit_code
                .map_or_else(|| "without a status".to_string(), |c| c.to_string()),
            check.output
        );
    }
    if attempt
        .checks
        .iter()
        .any(|c| !c.passed && c.stage == "acceptance")
    {
        out.push_str(
            "\nThe change passed the visible checks but failed an independent acceptance \
             check; re-read the instructions and criteria for a requirement it missed.\n",
        );
    }
    if !attempt.summary.trim().is_empty() {
        let _ = write!(out, "\nIts closing note:\n{}\n", attempt.summary.trim());
    }
    if restored {
        out.push_str("\nIts changes were rolled back; you start from the original files.\n");
    } else if !attempt.files_changed.is_empty() {
        let _ = write!(
            out,
            "\nIts changes are still in place in: {}. Fix or replace them.\n",
            attempt.files_changed.join(", ")
        );
    }
    out
}

/// System prompt addendum for a reviewer.
#[must_use]
pub fn reviewer_addendum() -> String {
    "# Block reviewer\n\
     \n\
     You review one change made by another worker. Do not change any files. Read the changed \
     files and judge only whether the change does what the block asked, correctly and without \
     breaking anything nearby. Your reply must start with a line that is exactly `APPROVE` or \
     `REJECT: <the most important problem>`, followed by any detail."
        .to_string()
}

/// The reviewer's prompt for `attempt` of `spec`.
#[must_use]
pub fn review_prompt(spec: &BlockSpec, attempt: &Attempt) -> String {
    let mut out = format!(
        "Review this change.\n\n## The block\n\n# {}\n\n{}\n",
        spec.title,
        spec.instructions.trim()
    );
    if !spec.criteria.is_empty() {
        out.push_str("\nDone when:\n");
        for c in &spec.criteria {
            let _ = writeln!(out, "- {c}");
        }
    }
    let _ = write!(
        out,
        "\n## What the worker did\n\nChanged files: {}\n\nIts summary:\n{}\n",
        if attempt.files_changed.is_empty() {
            "(none reported)".to_string()
        } else {
            attempt.files_changed.join(", ")
        },
        attempt.summary.trim()
    );
    out
}

/// Whether a reviewer's reply approves, and the reason when it doesn't.
///
/// # Errors
/// The rejection reason.
pub fn parse_review(reply: &str) -> Result<(), String> {
    let first = reply
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .trim_matches('*');
    if first.eq_ignore_ascii_case("approve") || first.to_ascii_uppercase().starts_with("APPROVE") {
        return Ok(());
    }
    let reason = first
        .split_once(':')
        .map_or(first, |(_, r)| r)
        .trim()
        .to_string();
    Err(if reason.is_empty() {
        "the reviewer did not approve".into()
    } else {
        format!("review rejected: {reason}")
    })
}

/// Reasoning effort for a tier doing `kind` work.
#[must_use]
pub const fn effort(tier: Tier, kind: BlockKind) -> &'static str {
    match (tier, kind) {
        (_, BlockKind::Explore | BlockKind::Docs) => "low",
        (Tier::Frontier, _) => "high",
        _ => "medium",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::tests::edit;
    use crate::block::{CheckOutcome, VerifyLevel};

    #[test]
    fn worker_prompt_shows_visible_checks_only() {
        let mut spec = edit("Add flag");
        spec.checks = vec!["cargo test".into()];
        spec.acceptance = vec!["sh secret.sh".into()];
        let prompt = block_prompt(&spec, None);
        assert!(prompt.contains("`cargo test`"));
        assert!(!prompt.contains("secret.sh"));
        assert!(prompt.contains("src/lib.rs"));
    }

    #[test]
    fn evidence_hides_acceptance_commands() {
        let attempt = Attempt {
            n: 1,
            tier: Tier::Fast,
            model: "m".into(),
            effort: "medium".into(),
            reason: "rule".into(),
            explored: false,
            propensity: 1.0,
            passed: false,
            level: VerifyLevel::Tested,
            failure: Some("an acceptance check failed".into()),
            checks: vec![CheckOutcome {
                stage: "acceptance".into(),
                command: "sh secret.sh".into(),
                passed: false,
                exit_code: Some(1),
                output: "expected 3".into(),
            }],
            cost_usd: 0.0,
            input_tokens: 0,
            output_tokens: 0,
            files_changed: vec!["src/lib.rs".into()],
            summary: "done".into(),
            agent: None,
            wall_ms: 0,
        };
        let text = evidence(&attempt, true);
        assert!(!text.contains("expected 3"));
        assert!(!text.contains("secret.sh"));
        assert!(text.contains("independent acceptance check"));
        assert!(text.contains("rolled back"));
    }

    #[test]
    fn review_replies_parse() {
        assert!(parse_review("APPROVE\nlooks fine").is_ok());
        assert!(parse_review("**APPROVE**").is_ok());
        assert_eq!(
            parse_review("REJECT: misses the empty case").unwrap_err(),
            "review rejected: misses the empty case"
        );
        assert!(parse_review("").is_err());
    }
}
