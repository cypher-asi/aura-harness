use super::*;
use crate::entry::{MergeStatus, Origin};
use chrono::{TimeZone, Utc};

/// A finished entry `q{seq}`, finished `seq` seconds into 2026.
fn finished(seq: u64) -> MergeEntry {
    let at = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap()
        + chrono::Duration::seconds(i64::try_from(seq).unwrap());
    MergeEntry {
        id: EntryId::from_seq(seq),
        branch: format!("aura/b{seq}"),
        head: format!("{seq:040}"),
        remote: None,
        origin: Origin::default(),
        status: if seq % 2 == 0 {
            MergeStatus::Merged
        } else {
            MergeStatus::Failed
        },
        enqueued_at: at,
        pushed_at: None,
        started_at: None,
        finished_at: Some(at),
        attempts: 1,
        detail: Some("a reason long enough to spread lines over chunks ".repeat(8)),
        merged: None,
    }
}

fn seqs(page: &HistoryPage) -> Vec<u64> {
    page.entries
        .iter()
        .map(|entry| entry.id.as_str()[1..].parse().unwrap())
        .collect()
}

/// Every entry by paging from the newest, `limit` at a time.
fn all_pages(dir: &Path, limit: usize) -> (Vec<u64>, usize) {
    let (mut all, mut pages, mut before) = (Vec::new(), 0, None);
    loop {
        let page = read_page(dir, before, limit).unwrap();
        pages += 1;
        all.extend(seqs(&page));
        match page.older {
            Some(older) => before = Some(older),
            None => return (all, pages),
        }
    }
}

#[test]
fn no_history_is_an_empty_page() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(
        read_page(dir.path(), None, PAGE).unwrap(),
        HistoryPage::default()
    );
}

#[test]
fn every_merge_is_kept_and_paged_newest_first() {
    let dir = tempfile::tempdir().unwrap();
    let entries: Vec<MergeEntry> = (1..=2_500).map(finished).collect();
    for batch in entries.chunks(7) {
        append(dir.path(), batch).unwrap();
    }
    let first = read_page(dir.path(), None, PAGE).unwrap();
    assert_eq!(seqs(&first), (2_401..=2_500).rev().collect::<Vec<_>>());
    assert!(first.older.is_some());

    let (all, pages) = all_pages(dir.path(), PAGE);
    assert_eq!(
        all,
        (1..=2_500).rev().collect::<Vec<_>>(),
        "none lost or repeated"
    );
    assert_eq!(pages, 25);
}

#[test]
fn a_page_cursor_stays_good_while_newer_merges_are_appended() {
    let dir = tempfile::tempdir().unwrap();
    append(dir.path(), &(1..=10).map(finished).collect::<Vec<_>>()).unwrap();
    let first = read_page(dir.path(), None, 4).unwrap();
    append(dir.path(), &[finished(11)]).unwrap();
    let second = read_page(dir.path(), first.older, 4).unwrap();
    assert_eq!(seqs(&second), [6, 5, 4, 3]);
}

#[test]
fn a_merge_written_twice_shows_once_and_a_torn_line_is_skipped() {
    let dir = tempfile::tempdir().unwrap();
    append(dir.path(), &[finished(1), finished(2)]).unwrap();
    // A crash mid-append leaves half a line.
    let mut file = OpenOptions::new()
        .append(true)
        .open(path_in(dir.path()))
        .unwrap();
    file.write_all(b"{\"id\":\"q3\",\"bra").unwrap();
    drop(file);
    // Finished again after recovery, then more.
    append(dir.path(), &[finished(2), finished(4)]).unwrap();
    let (all, _) = all_pages(dir.path(), PAGE);
    assert_eq!(all, [4, 2, 1]);
}

#[test]
fn seeding_starts_the_history_from_an_old_queue_file_once() {
    let dir = tempfile::tempdir().unwrap();
    seed(dir.path(), &[finished(3), finished(1), finished(2)]).unwrap();
    assert_eq!(seqs(&read_page(dir.path(), None, PAGE).unwrap()), [3, 2, 1]);
    seed(dir.path(), &[finished(9)]).unwrap();
    assert_eq!(seqs(&read_page(dir.path(), None, PAGE).unwrap()), [3, 2, 1]);
}
