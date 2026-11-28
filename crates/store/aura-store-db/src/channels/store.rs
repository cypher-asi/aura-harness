//! Channel records: create, read, list, update and archive, the
//! coordinator's last-read marker, and the fault-injected create.

use super::keys::{channel_key, seen_key, slug_key, CHANNEL_PREFIX};
use super::validate::{valid_slug, validate_id, validate_project_id, validate_update};
use super::{
    lock, ChannelError, ChannelRecord, ChannelStore, ChannelUpdate, MemberRecord, MemberRole,
    NewChannel,
};
use crate::agents::{AgentRecord, AgentStore, RegistryDiff};
use crate::cf;
use aura_core_types::AgentId;
use chrono::{DateTime, Utc};
use rocksdb::WriteBatch;
use std::sync::{Arc, Mutex};

/// Apply the fields `update` sets to `record`.
fn apply_update(record: &mut ChannelRecord, update: ChannelUpdate) {
    if let Some(title) = update.title {
        record.title = title;
    }
    if let Some(description) = update.description {
        record.description = description;
    }
    if let Some(project_id) = update.project_id {
        record.project_id = project_id;
    }
    if let Some(placement) = update.placement {
        record.placement = placement;
    }
}

impl ChannelStore {
    /// Store sharing `store`'s database, sealing mode and write
    /// durability.
    #[must_use]
    pub fn for_store(store: &crate::RocksStore) -> Self {
        let db = Arc::clone(store.db_handle());
        let cipher = store.cipher().cloned();
        Self {
            agents: AgentStore::with_cipher(Arc::clone(&db), cipher.clone()),
            db,
            cipher,
            sync_writes: store.sync_writes(),
            write_lock: Arc::new(Mutex::new(())),
            last_post_seq: Arc::new(Mutex::new(None)),
        }
    }

    pub(super) fn require_channel(&self, id: &str) -> Result<ChannelRecord, ChannelError> {
        self.get(id)?
            .ok_or_else(|| ChannelError::UnknownChannel(id.to_string()))
    }

    /// Create a channel run by `coordinator`, committing the channel (and
    /// its project, if any), its slug, the coordinator's agent row and
    /// its membership in one batch.
    ///
    /// # Errors
    /// [`ChannelError::InvalidSlug`], [`ChannelError::SlugTaken`],
    /// [`ChannelError::InvalidProjectId`] for an empty or blank project
    /// id, [`ChannelError::AlreadyMember`] when the coordinator is in a
    /// channel, [`ChannelError::Agent`] when its record breaks the agent
    /// key invariants; store, sealing and encoding failures. Nothing is
    /// written on error.
    pub fn create(
        &self,
        new: NewChannel,
        coordinator: &AgentRecord,
        now: DateTime<Utc>,
    ) -> Result<ChannelRecord, ChannelError> {
        let _guard = lock(&self.write_lock);
        let (record, batch, _) = self.prepare_create(new, coordinator, now)?;
        self.commit(batch, &format!("channel {}", record.id))?;
        tracing::info!(
            channel_id = %record.id,
            coordinator = %record.coordinator.to_hex(),
            "channel created"
        );
        Ok(record)
    }

    /// Validate, then build the full create batch and the partial one
    /// (just the agent row) a non-atomic writer would have committed.
    fn prepare_create(
        &self,
        new: NewChannel,
        coordinator: &AgentRecord,
        now: DateTime<Utc>,
    ) -> Result<(ChannelRecord, WriteBatch, WriteBatch), ChannelError> {
        let agent = coordinator.agent_id;
        self.check_create(&new, &agent)?;
        let record = ChannelRecord {
            id: uuid::Uuid::new_v4().to_string(),
            slug: new.slug,
            title: new.title,
            description: new.description,
            coordinator: agent,
            created_at: now,
            updated_at: now,
            archived: false,
            project_id: new.project_id,
            placement: new.placement,
        };
        let member = MemberRecord {
            agent_id: agent,
            role: MemberRole::Coordinator,
            parent: None,
            joined_at: now,
        };
        let (full, partial) = self.stage_create(&record, &member, coordinator)?;
        Ok((record, full, partial))
    }

    /// Refuse a create with a bad slug or project id, a taken slug, or a
    /// coordinator already in a channel.
    fn check_create(&self, new: &NewChannel, agent: &AgentId) -> Result<(), ChannelError> {
        if !valid_slug(&new.slug) {
            return Err(ChannelError::InvalidSlug(new.slug.clone()));
        }
        if let Some(project_id) = &new.project_id {
            validate_project_id(project_id)?;
        }
        let slugs = self.cf(cf::CHANNELS)?;
        let taken = self
            .db
            .get_cf(&slugs, slug_key(&new.slug))
            .map_err(|source| ChannelError::Backend {
                context: format!("reading slug {}", new.slug),
                source,
            })?;
        if taken.is_some() {
            return Err(ChannelError::SlugTaken(new.slug.clone()));
        }
        if let Some(channel) = self.channel_of(agent)? {
            return Err(ChannelError::AlreadyMember {
                agent: agent.to_hex(),
                channel,
            });
        }
        Ok(())
    }

    /// The full create batch (agent row, channel, slug, membership) and
    /// the partial one holding just the agent row.
    fn stage_create(
        &self,
        record: &ChannelRecord,
        member: &MemberRecord,
        coordinator: &AgentRecord,
    ) -> Result<(WriteBatch, WriteBatch), ChannelError> {
        let diff = RegistryDiff {
            agents_put: vec![coordinator.clone()],
            ..RegistryDiff::default()
        };
        let what = format!("channel {}", record.id);
        let channel_value = self.encode(record, &what)?;
        let slug_value = self.encode(&record.id, &format!("slug {}", record.slug))?;
        let channels = self.cf(cf::CHANNELS)?;

        let mut partial = WriteBatch::default();
        self.agents.stage(&diff, &mut partial)?;
        let mut full = WriteBatch::default();
        self.agents.stage(&diff, &mut full)?;
        full.put_cf(&channels, channel_key(&record.id), channel_value);
        full.put_cf(&channels, slug_key(&record.slug), slug_value);
        self.stage_member(&record.id, member, &mut full)?;
        Ok((full, partial))
    }

    /// The channel with `id`.
    ///
    /// # Errors
    /// [`ChannelError::InvalidId`]; store, unsealing and decoding
    /// failures.
    pub fn get(&self, id: &str) -> Result<Option<ChannelRecord>, ChannelError> {
        validate_id(id)?;
        self.read(cf::CHANNELS, &channel_key(id), &format!("channel {id}"))
    }

    /// The channel with `slug`, if any.
    ///
    /// # Errors
    /// Store, unsealing and decoding failures.
    pub fn by_slug(&self, slug: &str) -> Result<Option<ChannelRecord>, ChannelError> {
        if !valid_slug(slug) {
            return Ok(None);
        }
        let Some(id) =
            self.read::<String>(cf::CHANNELS, &slug_key(slug), &format!("slug {slug}"))?
        else {
            return Ok(None);
        };
        let record = self.get(&id)?;
        if record.is_none() {
            tracing::warn!(slug, channel_id = %id, "slug index points at a missing channel");
        }
        Ok(record)
    }

    /// Every channel, archived ones included, sorted by `created_at`,
    /// ties by slug.
    ///
    /// # Errors
    /// Store, unsealing and decoding failures.
    pub fn list(&self) -> Result<Vec<ChannelRecord>, ChannelError> {
        let rows = self.scan(cf::CHANNELS, CHANNEL_PREFIX, CHANNEL_PREFIX, "channels")?;
        let mut channels = Vec::with_capacity(rows.len());
        for (key, value) in rows {
            let id = String::from_utf8_lossy(key.get(CHANNEL_PREFIX.len()..).unwrap_or_default());
            let record: ChannelRecord = self.decode(&value, &format!("channel {id}"))?;
            if record.id != id {
                tracing::warn!(channel_id = %id, "skipping channel row whose key is not its id");
                continue;
            }
            channels.push(record);
        }
        channels.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.slug.cmp(&b.slug))
        });
        Ok(channels)
    }

    /// Change the title, description, project and/or placement of channel
    /// `id`, stamping `updated_at` with `now`.
    ///
    /// # Errors
    /// [`ChannelError::InvalidProjectId`] for an empty or blank project
    /// id, [`ChannelError::InvalidId`], [`ChannelError::UnknownChannel`];
    /// store, sealing and encoding failures. Nothing is written on error.
    pub fn update(
        &self,
        id: &str,
        update: ChannelUpdate,
        now: DateTime<Utc>,
    ) -> Result<ChannelRecord, ChannelError> {
        validate_update(&update)?;
        self.modify(id, now, |record| apply_update(record, update))
    }

    /// Apply `update` to channel `channel_id` as [`Self::update`] does,
    /// but put the changed `c/` row into `batch` instead of writing it,
    /// for a caller committing it together with rows of another store
    /// (e.g. the coordinator's agent row), then [`Self::write`]. Returns
    /// the record as it will be once `batch` is written.
    ///
    /// Takes no lock, like [`Self::stage_member`]: the record is read now
    /// and written whenever the caller commits, so an [`Self::update`] or
    /// [`Self::archive`] committed in between is overwritten. The caller
    /// must serialize its channel changes (aura-app runs them one at a
    /// time from its event loop). Holding the store's write lock here
    /// would not help, since it can't be held until the caller's write.
    /// On error `batch` is left as it was.
    ///
    /// # Errors
    /// As [`Self::update`].
    pub fn stage_update(
        &self,
        channel_id: &str,
        update: ChannelUpdate,
        now: DateTime<Utc>,
        batch: &mut WriteBatch,
    ) -> Result<ChannelRecord, ChannelError> {
        validate_update(&update)?;
        self.stage_change(
            channel_id,
            now,
            |record| apply_update(record, update),
            batch,
        )
    }

    /// Move channel `id` to the Archive. It keeps its slug and members.
    ///
    /// # Errors
    /// [`ChannelError::UnknownChannel`]; store, sealing and encoding
    /// failures.
    pub fn archive(&self, id: &str, now: DateTime<Utc>) -> Result<ChannelRecord, ChannelError> {
        self.modify(id, now, |record| record.archived = true)
    }

    /// Read channel `id`, `change` it and commit it, under the write lock.
    fn modify(
        &self,
        id: &str,
        now: DateTime<Utc>,
        change: impl FnOnce(&mut ChannelRecord),
    ) -> Result<ChannelRecord, ChannelError> {
        let _guard = lock(&self.write_lock);
        let mut batch = WriteBatch::default();
        let record = self.stage_change(id, now, change, &mut batch)?;
        self.commit(batch, &format!("channel {id}"))?;
        Ok(record)
    }

    /// Read channel `id`, `change` it, stamp `updated_at` and put it into
    /// `batch`. Takes no lock; `batch` is untouched on error.
    fn stage_change(
        &self,
        id: &str,
        now: DateTime<Utc>,
        change: impl FnOnce(&mut ChannelRecord),
        batch: &mut WriteBatch,
    ) -> Result<ChannelRecord, ChannelError> {
        let mut record = self.require_channel(id)?;
        change(&mut record);
        record.updated_at = now;
        let value = self.encode(&record, &format!("channel {id}"))?;
        let channels = self.cf(cf::CHANNELS)?;
        batch.put_cf(&channels, channel_key(id), value);
        Ok(record)
    }

    /// When the coordinator of channel `channel_id` last read the
    /// channel's activity, in Unix milliseconds; `None` when it never did.
    ///
    /// # Errors
    /// [`ChannelError::InvalidId`]; store failures.
    pub fn seen(&self, channel_id: &str) -> Result<Option<u64>, ChannelError> {
        validate_id(channel_id)?;
        self.read(
            cf::CHANNELS,
            &seen_key(channel_id),
            &format!("last read of channel {channel_id}"),
        )
    }

    /// Record that the coordinator of channel `channel_id` read its
    /// activity up to `at_ms`.
    ///
    /// # Errors
    /// [`ChannelError::InvalidId`]; sealing, encoding and store failures.
    pub fn set_seen(&self, channel_id: &str, at_ms: u64) -> Result<(), ChannelError> {
        validate_id(channel_id)?;
        let what = format!("last read of channel {channel_id}");
        let mut batch = WriteBatch::default();
        batch.put_cf(
            &self.cf(cf::CHANNELS)?,
            seen_key(channel_id),
            self.encode(&at_ms, &what)?,
        );
        self.commit(batch, &what)
    }
}

#[doc(hidden)]
impl ChannelStore {
    /// Fault-injected variant of [`Self::create`]; see
    /// [`crate::FaultAt`]. `BeforeBatchWrite` writes nothing,
    /// `AfterBatchWrite` commits the whole channel, and `InsideBatch`
    /// commits only the coordinator's agent row, the partial state a
    /// non-atomic writer leaves.
    ///
    /// # Errors
    /// Always fails: with [`ChannelError::InjectedFault`] once the writes
    /// of the fault mode are done, or earlier with the error that stopped
    /// them.
    pub fn create_with_fault(
        &self,
        new: NewChannel,
        coordinator: &AgentRecord,
        now: DateTime<Utc>,
        fault: crate::FaultAt,
    ) -> Result<ChannelRecord, ChannelError> {
        let _guard = lock(&self.write_lock);
        let (_, full, partial) = self.prepare_create(new, coordinator, now)?;
        match crate::rocks_store::write_batch_with_fault(
            &self.db,
            &self.write_opts(),
            full,
            partial,
            fault,
        ) {
            crate::rocks_store::FaultWriteError::Injected(fault) => {
                Err(ChannelError::InjectedFault(fault))
            }
            crate::rocks_store::FaultWriteError::Write(source) => Err(ChannelError::Backend {
                context: "writing channel batch".to_string(),
                source,
            }),
        }
    }
}
