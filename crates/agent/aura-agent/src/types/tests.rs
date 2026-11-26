
use super::{AgentLoopResult, FileChange, FileChangeKind};

fn fc(path: &str, kind: FileChangeKind) -> FileChange {
    FileChange {
        path: path.into(),
        kind,
        lines_added: 0,
        lines_removed: 0,
    }
}

fn fc_lines(path: &str, kind: FileChangeKind, added: u32, removed: u32) -> FileChange {
    FileChange {
        path: path.into(),
        kind,
        lines_added: added,
        lines_removed: removed,
    }
}

#[test]
fn file_change_summary_keeps_net_create() {
    let mut result = AgentLoopResult::default();
    result.record_file_change(fc("src/new.rs", FileChangeKind::Create));
    result.record_file_change(fc("src/new.rs", FileChangeKind::Modify));
    assert_eq!(result.file_changes.len(), 1);
    assert!(matches!(
        result.file_changes[0].kind,
        FileChangeKind::Create
    ));
}

#[test]
fn file_change_summary_drops_create_then_delete() {
    let mut result = AgentLoopResult::default();
    result.record_file_change(fc("src/temp.rs", FileChangeKind::Create));
    result.record_file_change(fc("src/temp.rs", FileChangeKind::Delete));
    assert!(result.file_changes.is_empty());
}

#[test]
fn file_change_summary_turns_delete_then_create_into_modify() {
    let mut result = AgentLoopResult::default();
    result.record_file_change(fc("src/lib.rs", FileChangeKind::Delete));
    result.record_file_change(fc("src/lib.rs", FileChangeKind::Create));
    assert_eq!(result.file_changes.len(), 1);
    assert!(matches!(
        result.file_changes[0].kind,
        FileChangeKind::Modify
    ));
}

#[test]
fn file_change_summary_sums_line_counts_across_merges() {
    let mut result = AgentLoopResult::default();
    result.record_file_change(fc_lines("src/lib.rs", FileChangeKind::Modify, 10, 2));
    result.record_file_change(fc_lines("src/lib.rs", FileChangeKind::Modify, 5, 3));
    assert_eq!(result.file_changes.len(), 1);
    assert_eq!(result.file_changes[0].lines_added, 15);
    assert_eq!(result.file_changes[0].lines_removed, 5);
}
