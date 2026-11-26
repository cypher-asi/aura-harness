use crate::gateway::session::helpers::finalize::summarize_files_changed;
use aura_agent::{AgentLoopResult, FileChange, FileChangeKind};

#[test]
fn summarize_files_changed_groups_by_operation() {
    let loop_result = AgentLoopResult {
        file_changes: vec![
            FileChange {
                path: "src/new.rs".into(),
                kind: FileChangeKind::Create,
                lines_added: 0,
                lines_removed: 0,
            },
            FileChange {
                path: "src/lib.rs".into(),
                kind: FileChangeKind::Modify,
                lines_added: 0,
                lines_removed: 0,
            },
            FileChange {
                path: "src/old.rs".into(),
                kind: FileChangeKind::Delete,
                lines_added: 0,
                lines_removed: 0,
            },
        ],
        ..AgentLoopResult::default()
    };

    let summary = summarize_files_changed(&loop_result);
    assert_eq!(summary.created, vec!["src/new.rs"]);
    assert_eq!(summary.modified, vec!["src/lib.rs"]);
    assert_eq!(summary.deleted, vec!["src/old.rs"]);
    // No FileChange carried non-zero counts, so diffs stays empty
    // and the wire format doesn't carry a misleading 0/0 entry.
    assert!(summary.diffs.is_empty());
}

#[test]
fn summarize_files_changed_emits_diffs_for_nonzero_counts() {
    let loop_result = AgentLoopResult {
        file_changes: vec![
            FileChange {
                path: "src/touched.rs".into(),
                kind: FileChangeKind::Modify,
                lines_added: 7,
                lines_removed: 2,
            },
            // write_file path: counts stay at 0 (unknown), so this
            // entry must NOT show up in diffs.
            FileChange {
                path: "src/new.rs".into(),
                kind: FileChangeKind::Create,
                lines_added: 0,
                lines_removed: 0,
            },
        ],
        ..AgentLoopResult::default()
    };

    let summary = summarize_files_changed(&loop_result);
    assert_eq!(summary.diffs.len(), 1);
    assert_eq!(summary.diffs[0].path, "src/touched.rs");
    assert_eq!(summary.diffs[0].lines_added, 7);
    assert_eq!(summary.diffs[0].lines_removed, 2);
}
