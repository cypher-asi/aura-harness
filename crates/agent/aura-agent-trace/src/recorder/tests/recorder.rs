//! The recorder's bundle, summary and deferred finish.

use super::*;

#[test]
fn recorder_writes_bundle_and_summary() {
    let home = tempfile::tempdir().unwrap();
    let meta = RunMeta::new("m", crate::RunSource::Test);
    let rec = RunRecorder::start(home.path(), meta, TraceLevel::Full)
        .unwrap()
        .unwrap();
    assert!(rec.blob_store().is_some());
    let seen = Arc::new(AtomicU64::new(0));
    let seen2 = seen.clone();
    rec.subscribe(move |_| {
        seen2.fetch_add(1, Ordering::Relaxed);
    });
    rec.begin_turn("hello\nworld");
    rec.on_event(&exchange(0, &[]));
    rec.end_turn(&AgentLoopResult::default());
    let card = rec.finish().unwrap();
    assert_eq!(card.model_calls, 1);
    assert_eq!(seen.load(Ordering::Relaxed), 3);
    let meta = rec.bundle().read_meta().unwrap();
    assert_eq!(meta.title, "hello");
    assert!(meta.ended_at_ms.is_some());
    assert_eq!(rec.bundle().read_steps().unwrap().len(), 3);
}

fn one_turn() -> (tempfile::TempDir, Arc<RunRecorder>) {
    let home = tempfile::tempdir().unwrap();
    let meta = RunMeta::new("m", crate::RunSource::Test);
    let rec = RunRecorder::start(home.path(), meta, TraceLevel::Decisions)
        .unwrap()
        .unwrap();
    rec.begin_turn("hello");
    rec.on_event(&exchange(0, &[]));
    rec.end_turn(&AgentLoopResult::default());
    (home, rec)
}

#[test]
fn a_finish_left_for_later_is_written_once_with_the_end_it_noted() {
    let (_home, rec) = one_turn();
    let noted = now_ms();
    assert!(rec.finish_later(), "the first note asks for a write");
    assert!(!rec.finish_later(), "a second one shares it");
    assert!(
        rec.bundle().read_summary().is_none(),
        "nothing is written yet"
    );

    std::thread::sleep(std::time::Duration::from_millis(5));
    let written = now_ms();
    rec.finish_due();
    assert_eq!(rec.bundle().read_summary().unwrap().model_calls, 1);
    assert!(rec.dir().join(crate::run_log::LOG_FILE).exists());
    let ended = rec.bundle().read_meta().unwrap().ended_at_ms.unwrap();
    assert!((noted..written).contains(&ended), "stamped when noted");

    std::fs::remove_file(rec.bundle().path(bundle::SUMMARY_FILE)).unwrap();
    rec.finish_due();
    assert!(rec.bundle().read_summary().is_none(), "nothing was due");
    assert!(rec.finish_later(), "a note after the write asks again");
}

#[test]
fn finish_writes_what_was_left_for_later() {
    let (_home, rec) = one_turn();
    assert!(rec.finish_later());
    rec.finish();
    std::fs::remove_file(rec.bundle().path(bundle::SUMMARY_FILE)).unwrap();
    rec.finish_due();
    assert!(
        rec.bundle().read_summary().is_none(),
        "nothing was left due"
    );
}

#[test]
fn off_level_records_nothing() {
    let home = tempfile::tempdir().unwrap();
    let meta = RunMeta::new("m", crate::RunSource::Test);
    assert!(RunRecorder::start(home.path(), meta, TraceLevel::Off)
        .unwrap()
        .is_none());
    assert!(bundle::list_run_dirs(home.path()).is_empty());
}
