//! Legacy `p/` project links and their one-way migration onto the
//! channel record's `project_id`.

use super::keys::{channel_key, link_key, link_key_channel, link_prefix, LINK_PREFIX};
use super::rows::Row;
use super::validate::{validate_id, validate_project_id};
use super::{lock, ChannelError, ChannelStore, ProjectLink};
use crate::cf;
use chrono::{DateTime, Utc};
use rocksdb::WriteBatch;
use std::collections::BTreeMap;

impl ChannelStore {
    /// Fold the legacy `p/` link rows into the channels' `project_id`,
    /// returning how many channels changed.
    ///
    /// For each channel with link rows, in one batch: when it has no
    /// project, it takes the link with the earliest `linked_at` (ties:
    /// the lowest project id) among those whose project `is_known`
    /// accepts (blank ids are never taken); when none is known it keeps
    /// no project. Then all of its link rows are deleted, including when
    /// it already had a project, which it keeps. `updated_at` is left
    /// alone: this is not a user change. Every such channel counts as
    /// changed, so a second call returns 0. Links of a channel that no
    /// longer exists are deleted too (logged, not counted); rows with a
    /// malformed key are skipped (logged) and left in place.
    ///
    /// A channel whose rows cannot be migrated (a link that fails to
    /// unseal or decode, a failed write) is logged with `warn!` and left
    /// as it was, rows included, and the others are still migrated; it
    /// is not counted, and a rerun tries it again.
    ///
    /// Runs under the write lock, so it does not interleave with
    /// [`Self::update`], [`Self::archive`] or [`Self::link_project`].
    ///
    /// # Errors
    /// Store, unsealing and decoding failures of the scan of the link
    /// rows; nothing is migrated then.
    pub fn migrate_links(&self, is_known: impl Fn(&str) -> bool) -> Result<usize, ChannelError> {
        let _guard = lock(&self.write_lock);
        let prefix = LINK_PREFIX.as_bytes();
        let rows = self.scan(cf::CHANNELS, prefix, prefix, "project links")?;
        let mut by_channel: BTreeMap<String, Vec<Row>> = BTreeMap::new();
        for (key, value) in rows {
            let Some(channel_id) = link_key_channel(&key) else {
                tracing::warn!(
                    key_len = key.len(),
                    "skipping project link row with a malformed key"
                );
                continue;
            };
            by_channel
                .entry(channel_id.to_string())
                .or_default()
                .push((key, value));
        }
        let mut changed = 0;
        for (channel_id, rows) in by_channel {
            match self.migrate_channel_links(&channel_id, &rows, &is_known) {
                Ok(true) => changed += 1,
                Ok(false) => {}
                Err(error) => tracing::warn!(
                    channel_id = %channel_id,
                    %error,
                    "could not migrate the project links of a channel; left for the next run"
                ),
            }
        }
        Ok(changed)
    }

    /// Migrate the link `rows` of channel `channel_id` in one batch;
    /// whether the channel exists (and so changed).
    fn migrate_channel_links(
        &self,
        channel_id: &str,
        rows: &[Row],
        is_known: &impl Fn(&str) -> bool,
    ) -> Result<bool, ChannelError> {
        let what = format!("project links of channel {channel_id}");
        let links = rows
            .iter()
            .map(|(_, value)| self.decode::<ProjectLink>(value, &what))
            .collect::<Result<Vec<_>, _>>()?;
        let channels = self.cf(cf::CHANNELS)?;
        let mut batch = WriteBatch::default();
        for (key, _) in rows {
            batch.delete_cf(&channels, key);
        }
        let Some(mut record) = self.get(channel_id)? else {
            self.commit(batch, &what)?;
            tracing::warn!(
                channel_id,
                dropped = links.len(),
                "deleted project links of a missing channel"
            );
            return Ok(false);
        };
        let mut dropped = links.len();
        if record.project_id.is_none() {
            let earliest = links
                .iter()
                .filter(|link| {
                    validate_project_id(&link.project_id).is_ok() && is_known(&link.project_id)
                })
                .min_by(|a, b| {
                    a.linked_at
                        .cmp(&b.linked_at)
                        .then_with(|| a.project_id.cmp(&b.project_id))
                });
            if let Some(link) = earliest {
                record.project_id = Some(link.project_id.clone());
                dropped -= 1;
                let value = self.encode(&record, &format!("channel {channel_id}"))?;
                batch.put_cf(&channels, channel_key(channel_id), value);
            }
        }
        self.commit(batch, &what)?;
        tracing::info!(
            channel_id,
            kept_project = record.project_id.as_deref().unwrap_or(""),
            dropped,
            "migrated legacy project links"
        );
        Ok(true)
    }

    /// Legacy: link `project_id` to channel `channel_id`; relinking
    /// refreshes `linked_at`. A channel's project now lives on its record
    /// (see [`Self::update`]); kept until its callers move, and its rows
    /// are folded in by [`Self::migrate_links`].
    ///
    /// # Errors
    /// [`ChannelError::InvalidId`], [`ChannelError::InvalidProjectId`],
    /// [`ChannelError::UnknownChannel`]; store, sealing and encoding
    /// failures.
    pub fn link_project(
        &self,
        channel_id: &str,
        project_id: &str,
        now: DateTime<Utc>,
    ) -> Result<ProjectLink, ChannelError> {
        validate_project_id(project_id)?;
        let _guard = lock(&self.write_lock);
        self.require_channel(channel_id)?;
        let link = ProjectLink {
            project_id: project_id.to_string(),
            linked_at: now,
        };
        let what = format!("project link {channel_id}/{project_id}");
        let value = self.encode(&link, &what)?;
        let mut batch = WriteBatch::default();
        batch.put_cf(
            &self.cf(cf::CHANNELS)?,
            link_key(channel_id, project_id),
            value,
        );
        self.commit(batch, &what)?;
        Ok(link)
    }

    /// Legacy: unlink `project_id` from channel `channel_id`. Unlinking a
    /// project that isn't linked is a no-op. Kept until its callers move
    /// to the record's `project_id`.
    ///
    /// # Errors
    /// [`ChannelError::InvalidId`], [`ChannelError::InvalidProjectId`];
    /// store failures.
    pub fn unlink_project(&self, channel_id: &str, project_id: &str) -> Result<(), ChannelError> {
        validate_id(channel_id)?;
        validate_project_id(project_id)?;
        let mut batch = WriteBatch::default();
        batch.delete_cf(&self.cf(cf::CHANNELS)?, link_key(channel_id, project_id));
        self.commit(batch, &format!("project link {channel_id}/{project_id}"))
    }

    /// Legacy: projects linked to channel `channel_id` by `p/` rows, in
    /// key order (by project id). Empty once [`Self::migrate_links`] ran;
    /// the channel's project is
    /// [`ChannelRecord::project_id`](super::ChannelRecord::project_id).
    /// Kept until its callers move.
    ///
    /// # Errors
    /// [`ChannelError::InvalidId`]; store, unsealing and decoding
    /// failures.
    pub fn projects(&self, channel_id: &str) -> Result<Vec<ProjectLink>, ChannelError> {
        validate_id(channel_id)?;
        let prefix = link_prefix(channel_id);
        let what = format!("project links of channel {channel_id}");
        self.scan(cf::CHANNELS, &prefix, &prefix, &what)?
            .into_iter()
            .map(|(_, value)| self.decode(&value, &what))
            .collect()
    }
}
