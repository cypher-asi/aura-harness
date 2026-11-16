//! Storage error types.

pub use aura_store_record::StoreError;

/// Wraps a `RocksDB` failure as [`StoreError::Backend`].
pub(crate) fn backend(err: rocksdb::Error) -> StoreError {
    StoreError::Backend(Box::new(err))
}
