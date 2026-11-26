//! Raw row access shared by every `impl ChannelStore` file: column
//! family handles, the sealing JSON codec, reads, prefix scans and
//! batch commits.

use super::{ChannelError, ChannelStore};
use rocksdb::{BoundColumnFamily, Direction, IteratorMode, WriteBatch, WriteOptions};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::borrow::Cow;
use std::sync::Arc;

/// One raw `(key, value)` row read by a prefix scan.
pub(super) type Row = (Box<[u8]>, Box<[u8]>);

impl ChannelStore {
    pub(super) fn cf(
        &self,
        name: &'static str,
    ) -> Result<Arc<BoundColumnFamily<'_>>, ChannelError> {
        self.db
            .cf_handle(name)
            .ok_or(ChannelError::MissingColumnFamily(name))
    }

    pub(super) fn write_opts(&self) -> WriteOptions {
        let mut opts = WriteOptions::default();
        opts.set_sync(self.sync_writes);
        opts
    }

    /// JSON-encode `value` and seal it in sealed mode.
    pub(super) fn encode<T: Serialize>(
        &self,
        value: &T,
        what: &str,
    ) -> Result<Vec<u8>, ChannelError> {
        let plain = serde_json::to_vec(value).map_err(|source| ChannelError::Serde {
            context: format!("encoding {what}"),
            source,
        })?;
        match &self.cipher {
            Some(cipher) => cipher.seal(&plain).map_err(|source| ChannelError::Seal {
                context: format!("sealing {what}"),
                source,
            }),
            None => Ok(plain),
        }
    }

    /// Unseal `bytes` in sealed mode and JSON-decode them.
    pub(super) fn decode<T: DeserializeOwned>(
        &self,
        bytes: &[u8],
        what: &str,
    ) -> Result<T, ChannelError> {
        let plain = match &self.cipher {
            Some(cipher) => {
                Cow::Owned(cipher.open(bytes).map_err(|source| ChannelError::Seal {
                    context: format!("opening {what}"),
                    source,
                })?)
            }
            None => Cow::Borrowed(bytes),
        };
        serde_json::from_slice(&plain).map_err(|source| ChannelError::Serde {
            context: format!("decoding {what}"),
            source,
        })
    }

    pub(super) fn read<T: DeserializeOwned>(
        &self,
        family: &'static str,
        key: &[u8],
        what: &str,
    ) -> Result<Option<T>, ChannelError> {
        let cf = self.cf(family)?;
        let bytes = self
            .db
            .get_cf(&cf, key)
            .map_err(|source| ChannelError::Backend {
                context: format!("reading {what}"),
                source,
            })?;
        bytes.map(|bytes| self.decode(&bytes, what)).transpose()
    }

    /// Every `(key, value)` row of `family` whose key starts with
    /// `prefix`, from `start` (which must itself start with `prefix`).
    pub(super) fn scan(
        &self,
        family: &'static str,
        prefix: &[u8],
        start: &[u8],
        what: &str,
    ) -> Result<Vec<Row>, ChannelError> {
        let cf = self.cf(family)?;
        let mut rows = Vec::new();
        for item in self
            .db
            .iterator_cf(&cf, IteratorMode::From(start, Direction::Forward))
        {
            let (key, value) = item.map_err(|source| ChannelError::Backend {
                context: format!("reading {what}"),
                source,
            })?;
            if !key.starts_with(prefix) {
                break;
            }
            rows.push((key, value));
        }
        Ok(rows)
    }

    pub(super) fn commit(&self, batch: WriteBatch, what: &str) -> Result<(), ChannelError> {
        self.db
            .write_opt(batch, &self.write_opts())
            .map_err(|source| ChannelError::Backend {
                context: format!("writing {what}"),
                source,
            })
    }
}
