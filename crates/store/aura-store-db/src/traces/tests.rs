use super::*;
use rocksdb::{ColumnFamilyDescriptor, Options};

fn test_db(dir: &std::path::Path) -> Arc<DBWithThreadMode<MultiThreaded>> {
    let mut opts = Options::default();
    opts.create_if_missing(true);
    opts.create_missing_column_families(true);
    let cfs = [
        crate::cf::TRACE_RUNS,
        crate::cf::TRACE_STEPS,
        crate::cf::TRACE_INDEX,
    ]
    .into_iter()
    .map(|name| ColumnFamilyDescriptor::new(name, Options::default()));
    Arc::new(DBWithThreadMode::<MultiThreaded>::open_cf_descriptors(&opts, dir, cfs).unwrap())
}

#[test]
fn run_roundtrip_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let store = TraceStore::new(test_db(dir.path()));
    store.put_meta("r1", Some("agent"), b"{\"m\":1}").unwrap();
    store
        .put_steps("r1", [(10, &b"ten"[..]), (2, &b"two"[..])])
        .unwrap();
    store.put_steps("r1", [(2, &b"two!"[..])]).unwrap();
    store
        .append_rewards("r1", &[b"w1".to_vec(), b"w2".to_vec()])
        .unwrap();
    store.append_records("r1", &[b"rec".to_vec()]).unwrap();
    store.put_summary("r1", b"card").unwrap();

    assert_eq!(store.meta("r1").unwrap().unwrap(), b"{\"m\":1}");
    assert_eq!(
        store.steps("r1").unwrap(),
        vec![b"two!".to_vec(), b"ten".to_vec()]
    );
    assert_eq!(
        store.rewards("r1").unwrap(),
        vec![b"w1".to_vec(), b"w2".to_vec()]
    );
    assert_eq!(store.records("r1").unwrap(), vec![b"rec".to_vec()]);
    assert_eq!(store.summary("r1").unwrap().unwrap(), b"card");
    assert!(store.meta("missing").unwrap().is_none());
}

#[test]
fn owners_list_their_runs_newest_last() {
    let dir = tempfile::tempdir().unwrap();
    let store = TraceStore::new(test_db(dir.path()));
    store.put_meta("0001-a", Some("x"), b"{}").unwrap();
    store.put_meta("0002-b", Some("x"), b"{}").unwrap();
    store.put_meta("0003-c", Some("y"), b"{}").unwrap();
    store.put_meta("0004-d", None, b"{}").unwrap();
    assert_eq!(store.runs_of("x").unwrap(), vec!["0001-a", "0002-b"]);
    assert_eq!(store.latest_run_of("y").unwrap().as_deref(), Some("0003-c"));
    assert!(store.latest_run_of("z").unwrap().is_none());
    assert_eq!(store.run_ids().unwrap().len(), 4);
}

#[test]
fn delete_removes_everything_of_the_run() {
    let dir = tempfile::tempdir().unwrap();
    let store = TraceStore::new(test_db(dir.path()));
    store.put_meta("r1", Some("x"), b"{}").unwrap();
    store.put_meta("r10", Some("x"), b"{}").unwrap();
    store.put_steps("r1", [(1, &b"s"[..])]).unwrap();
    store.put_steps("r10", [(1, &b"s"[..])]).unwrap();
    store.delete_run("r1").unwrap();
    assert!(store.meta("r1").unwrap().is_none());
    assert!(store.steps("r1").unwrap().is_empty());
    assert_eq!(store.runs_of("x").unwrap(), vec!["r10"]);
    assert_eq!(store.steps("r10").unwrap().len(), 1);
}

#[test]
fn keys_with_slashes_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let store = TraceStore::new(test_db(dir.path()));
    assert!(matches!(
        store.put_meta("a/b", None, b"{}"),
        Err(TraceStoreError::InvalidKey(_))
    ));
    assert!(matches!(
        store.put_meta("ok", Some(""), b"{}"),
        Err(TraceStoreError::InvalidKey(_))
    ));
}

#[test]
fn sealed_values_are_not_plaintext_on_disk() {
    let dir = tempfile::tempdir().unwrap();
    let db = test_db(dir.path());
    let cipher = Arc::new(SealCipher::new(&[9u8; 32]));
    let store = TraceStore::with_cipher(Arc::clone(&db), Some(cipher));
    let secret = b"model said the api key is hunter2";
    store.put_steps("r1", [(1, &secret[..])]).unwrap();
    assert_eq!(store.steps("r1").unwrap(), vec![secret.to_vec()]);

    let cf = db.cf_handle(crate::cf::TRACE_STEPS).unwrap();
    let raw = db
        .iterator_cf(&cf, IteratorMode::Start)
        .next()
        .unwrap()
        .unwrap()
        .1;
    assert!(SealCipher::is_sealed(&raw));
    assert!(!raw.windows(secret.len()).any(|w| w == secret.as_slice()));
}
