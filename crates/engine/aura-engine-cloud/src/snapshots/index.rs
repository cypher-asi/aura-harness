//! The index of the snapshots built, kept in
//! `AURA_HOME/cloud/snapshots.json`.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::SnapshotBuilder;
use crate::error::{CloudError, CloudResult};

/// One snapshot as the index remembers it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotRecord {
    /// Snapshot name.
    pub name: String,
    /// Fingerprint it was built for.
    pub fingerprint: String,
    /// Project commit it holds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    /// boxd version.
    pub version: u64,
    /// When it was built, Unix milliseconds.
    pub built_ms: u64,
}

/// The snapshots built, by key
/// ([`BASE_SNAPSHOT`](crate::names::BASE_SNAPSHOT) or a project id),
/// kept in `AURA_HOME/cloud/snapshots.json`.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotIndex {
    /// Records by key.
    pub records: BTreeMap<String, SnapshotRecord>,
}

impl SnapshotIndex {
    /// Read the index; a missing or unreadable file is an empty index.
    pub async fn load(path: &Path) -> Self {
        match tokio::fs::read(path).await {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|err| {
                tracing::warn!(path = %path.display(), error = %err, "snapshot index unreadable; starting empty");
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    /// Write the index atomically (temp file, then rename).
    ///
    /// # Errors
    ///
    /// When the file can't be written.
    pub async fn save(&self, path: &Path) -> CloudResult<()> {
        if let Some(dir) = path.parent() {
            tokio::fs::create_dir_all(dir)
                .await
                .map_err(|err| CloudError::io(format!("create {}", dir.display()), err))?;
        }
        let bytes = serde_json::to_vec_pretty(self).map_err(|err| CloudError::InvalidResponse {
            op: "snapshot index",
            message: err.to_string(),
        })?;
        let tmp = path.with_extension("json.tmp");
        tokio::fs::write(&tmp, bytes)
            .await
            .map_err(|err| CloudError::io(format!("write {}", tmp.display()), err))?;
        tokio::fs::rename(&tmp, path)
            .await
            .map_err(|err| CloudError::io(format!("replace {}", path.display()), err))
    }
}

impl SnapshotBuilder {
    pub(super) async fn record(&self, key: &str) -> Option<SnapshotRecord> {
        let mut guard = self.index.lock().await;
        if guard.is_none() {
            *guard = Some(SnapshotIndex::load(&self.index_path).await);
        }
        guard
            .as_ref()
            .and_then(|index| index.records.get(key).cloned())
    }

    pub(super) async fn store(&self, key: &str, record: SnapshotRecord) -> CloudResult<()> {
        let mut guard = self.index.lock().await;
        let index = guard.get_or_insert_with(SnapshotIndex::default);
        index.records.insert(key.to_string(), record);
        index.save(&self.index_path).await
    }
}
