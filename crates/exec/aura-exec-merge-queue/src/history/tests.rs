use super::*;
use crate::entry::{EntryId, MergeStatus, Origin};
use chrono::DateTime;

fn entry(seq: u64) -> MergeEntry {
    MergeEntry {
        id: EntryId::from_seq(seq),
        branch: format!("aura/b{seq}"),
        head: format!("{seq:040}"),
        remote: Some("origin".to_string()),
        origin: Origin::default(),
        status: MergeStatus::Merged,
        enqueued_at: DateTime::from_timestamp(1_700_000_000, 0).unwrap_or_default(),
        pushed_at: None,
        started_at: None,
        finished_at: None,
        updated_at: None,
        attempts: 1,
        // Long enough that a page spans blocks.
        detail: Some("x".repeat(700)),
        merged: None,
    }
}

fn ids(entries: &[MergeEntry]) -> Vec<String> {
    entries.iter().map(|e| e.id.to_string()).collect()
}

#[test]
fn appended_entries_come_back_newest_first_in_pages() {
    let dir = tempfile::tempdir().unwrap();
    let history = History::new(dir.path());
    let mut mark = HistoryMark::default();
    for chunk in (1..=250).collect::<Vec<u64>>().chunks(30) {
        let entries: Vec<MergeEntry> = chunk.iter().map(|&seq| entry(seq)).collect();
        mark = history.append(mark, &entries).unwrap();
    }
    assert_eq!(mark.count, 250);
    let first = history.newest(mark, 0, 100).unwrap();
    assert_eq!(first.len(), 100);
    assert_eq!(first[0].id.to_string(), "q250");
    assert_eq!(first[99].id.to_string(), "q151");
    let mut all = Vec::new();
    let mut skip = 0;
    loop {
        let page = history.newest(mark, skip, 70).unwrap();
        if page.is_empty() {
            break;
        }
        skip += page.len() as u64;
        all.extend(page);
    }
    let expected: Vec<String> = (1..=250).rev().map(|seq| format!("q{seq}")).collect();
    assert_eq!(ids(&all), expected);
    assert_eq!(all[0], entry(250), "entries read back whole");
}

#[test]
fn an_append_whose_queue_file_was_never_saved_is_not_history() {
    let dir = tempfile::tempdir().unwrap();
    let history = History::new(dir.path());
    let mark = history.append(HistoryMark::default(), &[entry(1)]).unwrap();
    // A crash after this append, before the queue file saved its mark.
    let lost = history.append(mark, &[entry(2)]).unwrap();
    assert_eq!(lost.count, 2);
    assert_eq!(ids(&history.newest(mark, 0, 10).unwrap()), ["q1"]);
    // The entry is still in the queue file; it is archived again.
    let again = history.append(mark, &[entry(2), entry(3)]).unwrap();
    assert_eq!(again.count, 3);
    assert_eq!(
        ids(&history.newest(again, 0, 10).unwrap()),
        ["q3", "q2", "q1"]
    );
}

#[test]
fn a_file_shorter_than_its_mark_is_counted_again() {
    let dir = tempfile::tempdir().unwrap();
    let history = History::new(dir.path());
    let mark = history
        .append(HistoryMark::default(), &[entry(1), entry(2)])
        .unwrap();
    let path = dir.path().join(HISTORY_FILE);
    let text = std::fs::read(&path).unwrap();
    let first_line = text.iter().position(|&b| b == b'\n').unwrap() + 1;
    // Lost its last line and half of nothing else.
    std::fs::write(&path, &text[..first_line + 10]).unwrap();
    let after = history.append(mark, &[entry(3)]).unwrap();
    assert_eq!(after.count, 2, "one line survived, one appended");
    let read = history.newest(after, 0, 10).unwrap();
    assert_eq!(ids(&read), ["q3", "q1"], "the torn line is gone");
}

#[test]
fn no_file_is_no_history() {
    let dir = tempfile::tempdir().unwrap();
    let history = History::new(dir.path());
    let mark = HistoryMark {
        count: 5,
        bytes: 500,
    };
    assert!(history.newest(mark, 0, 10).unwrap().is_empty());
}
