//! Temporary RocksDB store for one benchmark run.

use std::path::Path;
use std::sync::Arc;

use rocksdb::{ColumnFamilyDescriptor, DBWithThreadMode, MultiThreaded, Options};

pub(crate) fn test_db(dir: &Path) -> Arc<DBWithThreadMode<MultiThreaded>> {
    let mut options = Options::default();
    options.create_if_missing(true);
    options.create_missing_column_families(true);
    let cfs = [
        "memory_facts",
        "memory_events",
        "memory_procedures",
        "memory_event_index",
        "memory_config",
    ]
    .into_iter()
    .map(|name| ColumnFamilyDescriptor::new(name, Options::default()));
    Arc::new(
        DBWithThreadMode::<MultiThreaded>::open_cf_descriptors(&options, dir, cfs)
            .expect("open benchmark store"),
    )
}
