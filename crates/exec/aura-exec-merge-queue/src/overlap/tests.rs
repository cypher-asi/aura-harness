use super::*;
use crate::entry::{MergeRequest, Origin, QueuedPush};
use crate::state::{Enqueued, RunOutcome};

fn at(seconds: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(1_700_000_000 + seconds, 0).unwrap()
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| (*item).to_string()).collect()
}

/// `state` with a push of `branch` at `head` by `agent`, changing `files`.
fn push(state: &mut QueueState, branch: &str, head: &str, agent: &str, files: &[&str]) -> EntryId {
    let request = MergeRequest {
        push: QueuedPush {
            branch: branch.to_string(),
            head: head.to_string(),
            remote: None,
        },
        origin: Origin {
            agent: Some(agent.to_string()),
            agent_name: Some(agent.to_uppercase()),
            channel: None,
        },
    };
    let id = state.enqueue(request, at(0)).id().clone();
    assert!(state.record_files(&id, head, strings(files)));
    id
}

fn work(branch: &str, agent: &str, files: &[&str]) -> Work {
    Work {
        entry: None,
        branch: branch.to_string(),
        agent: Some(agent.to_string()),
        agent_name: None,
        files: strings(files),
    }
}

#[test]
fn files_are_recorded_for_the_head_they_were_read_from() {
    let mut state = QueueState::default();
    let id = push(&mut state, "aura/a", "h1", "a", &["src/a.rs"]);
    assert_eq!(state.entries[0].files, strings(&["src/a.rs"]));
    let again = state.enqueue(
        MergeRequest {
            push: QueuedPush {
                branch: "aura/a".to_string(),
                head: "h2".to_string(),
                remote: None,
            },
            origin: Origin::default(),
        },
        at(1),
    );
    assert_eq!(again, Enqueued::Updated(id.clone()));
    assert!(
        !state.record_files(&id, "h1", strings(&["old.rs"])),
        "a stale read is dropped"
    );
    assert!(state.record_files(&id, "h2", strings(&["new.rs"])));
    assert_eq!(state.entries[0].files, strings(&["new.rs"]));
    assert!(!state.record_files(&EntryId::from_seq(99), "h2", Vec::new()));
}

#[test]
fn open_work_is_the_latest_of_each_branch_not_yet_merged() {
    let mut state = QueueState::default();
    push(&mut state, "aura/merged", "m1", "m", &["m.rs"]);
    push(&mut state, "aura/failed", "f1", "f", &["f.rs"]);
    push(&mut state, "aura/stale", "s1", "s", &["s.rs"]);
    push(&mut state, "aura/queued", "q1", "q", &["q.rs"]);
    push(&mut state, "aura/none", "n1", "n", &[]);
    let finish = |state: &mut QueueState, outcome, when| {
        let claimed = state.claim_next(at(when)).unwrap();
        state.finish(&claimed.id, outcome, at(when));
    };
    let merged = RunOutcome::Merged {
        merged: "x".to_string(),
        note: String::new(),
        pr: None,
    };
    finish(&mut state, merged, 10);
    let now = at(Duration::hours(FIXING_HOURS).num_seconds());
    finish(&mut state, RunOutcome::Failed("conflict".into()), 20);
    finish(&mut state, RunOutcome::Failed("old".into()), -100);
    let branches: Vec<String> = open_work(&state.entries, now)
        .into_iter()
        .map(|work| work.branch)
        .collect();
    assert_eq!(branches, ["aura/failed", "aura/queued"]);
}

#[test]
fn overlaps_share_files_someone_writes_with_other_owners_only() {
    let subject = work(
        "aura/a",
        "a",
        &["src/x.rs", "src/y.rs", "CHANGELOG.md", "Cargo.lock"],
    );
    let others = [
        work("aura/b", "b", &["src/y.rs", "src/x.rs", "src/z.rs"]),
        work("aura/c", "c", &["CHANGELOG.md", "Cargo.lock"]),
        work("aura/d", "d", &["src/z.rs"]),
        work("aura/a", "other", &["src/x.rs"]),
        work("aura/e", "a", &["src/x.rs"]),
    ];
    let found = find(&subject, &others);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].with.branch, "aura/b");
    assert_eq!(found[0].files, strings(&["src/x.rs", "src/y.rs"]));
}

#[test]
fn work_says_where_it_is_and_whose_it_is() {
    let mut entry = work("aura/a", "hex", &[]);
    assert_eq!(entry.place(), "worktree aura/a");
    assert_eq!(entry.owner(), "aura/a");
    entry.entry = Some(EntryId::from_seq(4));
    entry.agent_name = Some("RUS1".to_string());
    assert_eq!(entry.place(), "q4 aura/a");
    assert_eq!(entry.owner(), "RUS1");
}
