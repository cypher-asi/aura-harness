//! Opening a database with column families this build does not know.

use crate::rocks_store::*;
use rocksdb::{ColumnFamilyDescriptor, DBWithThreadMode, MultiThreaded, Options};
use tempfile::TempDir;

#[test]
fn opens_database_with_unknown_future_column_family() {
    let dir = TempDir::new().unwrap();
    let mut options = Options::default();
    options.create_if_missing(true);
    options.create_missing_column_families(true);
    let future_family = "future_runtime_state";
    let initial = DBWithThreadMode::<MultiThreaded>::open_cf_descriptors(
        &options,
        dir.path(),
        vec![ColumnFamilyDescriptor::new(
            future_family,
            Options::default(),
        )],
    )
    .unwrap();
    initial
        .put_cf(
            &initial.cf_handle(future_family).unwrap(),
            b"future-key",
            b"future-value",
        )
        .unwrap();
    drop(initial);

    let store = RocksStore::open(dir.path(), false)
        .expect("older-compatible store should retain unknown column families");
    let future = store.db_handle().cf_handle(future_family).unwrap();
    assert_eq!(
        store.db_handle().get_cf(&future, b"future-key").unwrap(),
        Some(b"future-value".to_vec())
    );
}
