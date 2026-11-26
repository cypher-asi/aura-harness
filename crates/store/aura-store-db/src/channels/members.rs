//! Channel membership: the `m/` member rows and the `a/` reverse index,
//! always written and deleted together.

use super::keys::{member_key, member_prefix, reverse_key, REVERSE_PREFIX};
use super::validate::validate_id;
use super::{lock, ChannelError, ChannelStore, MemberRecord, MemberRole};
use crate::cf;
use aura_core_types::AgentId;
use rocksdb::WriteBatch;

impl ChannelStore {
    /// Add `member` to channel `channel_id` in one batch (member row and
    /// reverse index). Adding an agent already in this channel is a
    /// no-op that keeps the existing record.
    ///
    /// # Errors
    /// [`ChannelError::UnknownChannel`], [`ChannelError::AlreadyMember`]
    /// when the agent is in another channel; store, sealing and encoding
    /// failures.
    pub fn add_member(&self, channel_id: &str, member: &MemberRecord) -> Result<(), ChannelError> {
        let _guard = lock(&self.write_lock);
        self.require_channel(channel_id)?;
        match self.channel_of(&member.agent_id)? {
            Some(channel) if channel == channel_id => return Ok(()),
            Some(channel) => {
                return Err(ChannelError::AlreadyMember {
                    agent: member.agent_id.to_hex(),
                    channel,
                })
            }
            None => {}
        }
        let mut batch = WriteBatch::default();
        self.stage_member(channel_id, member, &mut batch)?;
        self.commit(
            batch,
            &format!(
                "member {} of channel {channel_id}",
                member.agent_id.to_hex()
            ),
        )
    }

    /// Add the member row and reverse index of `member` to `batch`, for a
    /// caller committing them together with rows of another store (e.g.
    /// a new agent's row), then [`Self::write`].
    ///
    /// Performs no checks: the caller must first confirm, under its own
    /// serialization, that the channel exists and that
    /// [`Self::channel_of`] the agent is `None`, or the one-channel-per-
    /// agent invariant can break. On error `batch` is left as it was.
    ///
    /// # Errors
    /// [`ChannelError::InvalidId`]; sealing, encoding and
    /// missing-column-family failures.
    pub fn stage_member(
        &self,
        channel_id: &str,
        member: &MemberRecord,
        batch: &mut WriteBatch,
    ) -> Result<(), ChannelError> {
        validate_id(channel_id)?;
        let agent = member.agent_id;
        let what = format!("member {} of channel {channel_id}", agent.to_hex());
        let member_value = self.encode(member, &what)?;
        let reverse_value = self.encode(&channel_id, &format!("channel of {}", agent.to_hex()))?;
        let members = self.cf(cf::CHANNEL_MEMBERS)?;
        batch.put_cf(&members, member_key(channel_id, &agent), member_value);
        batch.put_cf(&members, reverse_key(&agent), reverse_value);
        Ok(())
    }

    /// Commit a batch built with [`Self::stage_member`] (and other
    /// stores' `stage` methods) in one write.
    ///
    /// # Errors
    /// Store failures; nothing is written on error.
    pub fn write(&self, batch: WriteBatch) -> Result<(), ChannelError> {
        self.commit(batch, "channel batch")
    }

    /// Remove `agent` from channel `channel_id`. The reverse index is
    /// only deleted when it points at this channel; removing an agent
    /// that isn't a member is a no-op.
    ///
    /// # Errors
    /// [`ChannelError::InvalidId`]; store, unsealing and decoding
    /// failures.
    pub fn remove_member(&self, channel_id: &str, agent: &AgentId) -> Result<(), ChannelError> {
        validate_id(channel_id)?;
        let _guard = lock(&self.write_lock);
        let members = self.cf(cf::CHANNEL_MEMBERS)?;
        let mut batch = WriteBatch::default();
        batch.delete_cf(&members, member_key(channel_id, agent));
        if self.channel_of(agent)?.as_deref() == Some(channel_id) {
            batch.delete_cf(&members, reverse_key(agent));
        }
        self.commit(
            batch,
            &format!("removing member {} of channel {channel_id}", agent.to_hex()),
        )
    }

    /// Members of channel `channel_id`: the coordinator first, then by
    /// `joined_at`, ties by agent id.
    ///
    /// # Errors
    /// [`ChannelError::InvalidId`]; store, unsealing and decoding
    /// failures.
    pub fn members(&self, channel_id: &str) -> Result<Vec<MemberRecord>, ChannelError> {
        validate_id(channel_id)?;
        let prefix = member_prefix(channel_id);
        let what = format!("members of channel {channel_id}");
        let rows = self.scan(cf::CHANNEL_MEMBERS, &prefix, &prefix, &what)?;
        let mut members = Vec::with_capacity(rows.len());
        for (key, value) in rows {
            let Some(agent) = key
                .get(prefix.len()..)
                .and_then(|tail| <[u8; 32]>::try_from(tail).ok())
                .map(AgentId::new)
            else {
                tracing::warn!(
                    channel_id,
                    key_len = key.len(),
                    "skipping member row with a malformed key"
                );
                continue;
            };
            let record: MemberRecord = self.decode(&value, &what)?;
            if record.agent_id != agent {
                tracing::warn!(
                    channel_id,
                    agent_id = %agent.to_hex(),
                    "skipping member row whose key is not its agent id"
                );
                continue;
            }
            members.push(record);
        }
        // Stable sort: rows came in agent id order, so ties stay in it.
        members.sort_by_key(|member| (member.role != MemberRole::Coordinator, member.joined_at));
        Ok(members)
    }

    /// The id of the channel `agent` is in, if any.
    ///
    /// # Errors
    /// Store, unsealing and decoding failures.
    pub fn channel_of(&self, agent: &AgentId) -> Result<Option<String>, ChannelError> {
        self.read(
            cf::CHANNEL_MEMBERS,
            &reverse_key(agent),
            &format!("channel of {}", agent.to_hex()),
        )
    }

    /// Every channel member, as `(channel id, agent)`, from the reverse
    /// index. Rows with a malformed key are skipped (logged).
    ///
    /// # Errors
    /// Store, unsealing and decoding failures.
    pub fn all_members(&self) -> Result<Vec<(String, AgentId)>, ChannelError> {
        let rows = self.scan(
            cf::CHANNEL_MEMBERS,
            REVERSE_PREFIX,
            REVERSE_PREFIX,
            "channel members",
        )?;
        let mut members = Vec::with_capacity(rows.len());
        for (key, value) in rows {
            let Some(agent) = key
                .get(REVERSE_PREFIX.len()..)
                .and_then(|tail| <[u8; 32]>::try_from(tail).ok())
                .map(AgentId::new)
            else {
                tracing::warn!(
                    key_len = key.len(),
                    "skipping channel reverse-index row with a malformed key"
                );
                continue;
            };
            let channel_id: String = self.decode(&value, "channel of a member")?;
            members.push((channel_id, agent));
        }
        Ok(members)
    }
}
