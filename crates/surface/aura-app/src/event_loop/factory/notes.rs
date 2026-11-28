//! What the merge queue tells the agents: the chat line for a queued
//! push and for an entry that ended, the prompt a failed entry's agent
//! acts on, and the line for a queue that holds.

use aura_exec_merge_queue::{Enqueued, FailureKind, Hold, HoldKind, MergeEntry, MergeStatus};

/// Most characters of a failure's output the agent's prompt quotes.
const PROMPT_EXCERPT_CHARS: usize = 3_000;

/// The chat line for a push that was queued as `enqueued`; none for a
/// push seen again (a cloud turn's pushes are read twice).
pub(super) fn queued_note(branch: &str, target: &str, enqueued: &Enqueued) -> Option<String> {
    match enqueued {
        Enqueued::Added(id) => Some(format!(
            "Merge queue: {branch} is queued to merge into {target} ({id})."
        )),
        Enqueued::Updated(id) => Some(format!(
            "Merge queue: {branch} was queued already; it merges the new push ({id})."
        )),
        Enqueued::Duplicate(_) => None,
    }
}

/// The chat line for an entry that ended.
pub(super) fn outcome_line(entry: &MergeEntry) -> String {
    let detail = entry.detail.as_deref().unwrap_or_default();
    let pr = entry
        .pr
        .as_ref()
        .map_or(String::new(), |pr| format!(" {}", pr.url));
    match entry.status {
        MergeStatus::Merged => format!("Merge queue: {} merged{pr} ({detail}).", entry.branch),
        _ => format!("Merge queue: {} was not merged{pr}: {detail}", entry.branch),
    }
}

/// What the agent whose merge into `target` failed is told to do: why,
/// with the PR, the files or tests and their output, and to fix it on
/// the same branch.
pub(super) fn failure_prompt(entry: &MergeEntry, target: &str) -> String {
    let branch = &entry.branch;
    let pr = entry
        .pr
        .as_ref()
        .map_or(String::new(), |pr| format!(" through PR {}", pr.url));
    let mut prompt = format!(
        "The merge queue could not merge your push of `{branch}` ({}) into {target}{pr}: {}\n",
        entry.short_head(),
        entry.detail.as_deref().unwrap_or("no reason given")
    );
    let failure = entry.failure.as_ref();
    let kind = failure.map_or(FailureKind::Other, |failure| failure.kind);
    if let Some(failure) = failure {
        if !failure.files.is_empty() {
            prompt.push_str(&format!(
                "\nConflicting files: {}\n",
                failure.files.join(", ")
            ));
        }
        if !failure.tests.is_empty() {
            prompt.push_str(&format!("\nFailing tests: {}\n", failure.tests.join(", ")));
        }
        if let Some(excerpt) = &failure.excerpt {
            let start = excerpt.chars().count().saturating_sub(PROMPT_EXCERPT_CHARS);
            let excerpt: String = excerpt.chars().skip(start).collect();
            prompt.push_str(&format!("\nOutput:\n```text\n{excerpt}\n```\n"));
        }
    }
    let fix = match kind {
        FailureKind::Conflict => "resolve the conflicts in those files and build it",
        FailureKind::Check => "make the build and the failing tests pass",
        FailureKind::Refused | FailureKind::Other => "fix what the reason says",
    };
    prompt.push_str(&format!(
        "\nFix it on this same branch, `{branch}`: pull {target} into your branch, {fix}, then \
         call git_ship again. It goes back into the merge queue, which tests it again{}. Don't \
         start a new branch or PR for this fix.",
        if entry.pr.is_some() {
            " on the same PR"
        } else {
            ""
        }
    ));
    prompt
}

/// The chat line for an agent whose entry waits because the queue holds.
pub(super) fn held_line(entry: &MergeEntry, hold: &Hold) -> String {
    let why = match hold.kind {
        HoldKind::ForgeUnavailable => "GitHub can't be reached",
        HoldKind::TargetBroken => "the target branch itself fails its checks",
    };
    format!(
        "Merge queue: {} keeps its place and waits: {why}. Nothing is wrong with it; the queue \
         tries again on its own. ({})",
        entry.branch, hold.reason
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use aura_exec_merge_queue::{
        EntryId, Failure, MergeRequest, Origin, PrRef, QueueState, QueuedPush, RunOutcome,
    };

    fn failed_entry(failure: Failure) -> MergeEntry {
        let mut state = QueueState::default();
        let id = state
            .enqueue(
                MergeRequest {
                    push: QueuedPush {
                        branch: "aura/x-rus1".to_string(),
                        head: "0123456789".to_string(),
                        remote: Some("origin".to_string()),
                    },
                    origin: Origin::default(),
                },
                chrono::Utc::now(),
            )
            .id()
            .clone();
        state.claim_next(chrono::Utc::now());
        state
            .finish(&id, RunOutcome::Failed(failure), chrono::Utc::now())
            .unwrap_or_else(|| panic!("{id} finished"))
    }

    fn pr() -> PrRef {
        PrRef {
            number: 12,
            url: "https://github.com/o/r/pull/12".to_string(),
        }
    }

    #[test]
    fn a_failure_tells_the_agent_why_and_how_to_ship_again() {
        let entry = failed_entry("test `cargo test` ended with exit 101".into());
        let prompt = failure_prompt(&entry, "trunk");
        assert!(prompt.contains("aura/x-rus1"), "{prompt}");
        assert!(prompt.contains("exit 101"), "{prompt}");
        assert!(prompt.contains("git_ship"), "{prompt}");
        assert!(prompt.contains("pull trunk into your branch"), "{prompt}");
        assert!(!prompt.contains("main"), "{prompt}");
        assert!(outcome_line(&entry).contains("was not merged"));
    }

    #[test]
    fn a_conflict_names_the_pr_and_the_files_and_keeps_the_branch() {
        let entry = failed_entry(Failure {
            files: vec!["src/a.rs".to_string(), "src/b.rs".to_string()],
            pr: Some(pr()),
            ..Failure::new(FailureKind::Conflict, "conflicts with main at 1234")
        });
        let prompt = failure_prompt(&entry, "main");
        assert!(
            prompt.contains("https://github.com/o/r/pull/12"),
            "{prompt}"
        );
        assert!(
            prompt.contains("Conflicting files: src/a.rs, src/b.rs"),
            "{prompt}"
        );
        assert!(prompt.contains("same branch, `aura/x-rus1`"), "{prompt}");
        assert!(prompt.contains("same PR"), "{prompt}");
        assert!(outcome_line(&entry).contains("pull/12"));
    }

    #[test]
    fn a_failing_test_shows_its_name_and_output() {
        let entry = failed_entry(Failure {
            tests: vec!["cli_golden_top_level_help".to_string()],
            excerpt: Some("-  changelog\n+".to_string()),
            ..Failure::new(FailureKind::Check, "test `cargo test` ended with exit 101")
        });
        let prompt = failure_prompt(&entry, "main");
        assert!(
            prompt.contains("Failing tests: cli_golden_top_level_help"),
            "{prompt}"
        );
        assert!(prompt.contains("-  changelog"), "{prompt}");
        assert!(prompt.contains("failing tests pass"), "{prompt}");
    }

    #[test]
    fn the_queued_note_says_where_the_push_is() {
        let id = EntryId::from_seq(4);
        let note = |enqueued| queued_note("aura/x", "main", &enqueued).unwrap_or_default();
        assert!(note(Enqueued::Added(id.clone())).contains("q4"));
        assert!(note(Enqueued::Updated(id.clone())).contains("new push"));
        assert_eq!(
            queued_note("aura/x", "main", &Enqueued::Duplicate(id)),
            None
        );
    }
}
