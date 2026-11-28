//! Summaries of the user's long messages, cached for a channel's HISTORY
//! tab so each message is summarized by a model once, across restarts.

use super::keys::summary_key;
use super::validate::validate_id;
use super::{ChannelError, ChannelStore};
use crate::cf;
use rocksdb::WriteBatch;

/// Longest summary key: a 256-bit hash in hex.
const MAX_KEY_LEN: usize = 64;

/// Check that `key` is 1 to [`MAX_KEY_LEN`] lowercase hex digits, so it
/// can't reach outside its channel's `h/` rows.
fn validate_summary_key(key: &str) -> Result<(), ChannelError> {
    let is_hex = key
        .bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    if key.is_empty() || key.len() > MAX_KEY_LEN || !is_hex {
        return Err(ChannelError::InvalidSummaryKey(key.to_string()));
    }
    Ok(())
}

impl ChannelStore {
    /// The summary stored for the message keyed `key` in channel
    /// `channel_id`; `None` when there is none yet.
    ///
    /// # Errors
    /// [`ChannelError::InvalidId`], [`ChannelError::InvalidSummaryKey`];
    /// unsealing, decoding and store failures.
    pub fn summary(&self, channel_id: &str, key: &str) -> Result<Option<String>, ChannelError> {
        validate_id(channel_id)?;
        validate_summary_key(key)?;
        self.read(
            cf::CHANNELS,
            &summary_key(channel_id, key),
            &format!("message summary {key} of channel {channel_id}"),
        )
    }

    /// Store `summary` for the message keyed `key` in channel
    /// `channel_id`, replacing any before it.
    ///
    /// # Errors
    /// [`ChannelError::InvalidId`], [`ChannelError::InvalidSummaryKey`];
    /// sealing, encoding and store failures.
    pub fn set_summary(
        &self,
        channel_id: &str,
        key: &str,
        summary: &str,
    ) -> Result<(), ChannelError> {
        validate_id(channel_id)?;
        validate_summary_key(key)?;
        let what = format!("message summary {key} of channel {channel_id}");
        let mut batch = WriteBatch::default();
        batch.put_cf(
            &self.cf(cf::CHANNELS)?,
            summary_key(channel_id, key),
            self.encode(&summary, &what)?,
        );
        self.commit(batch, &what)
    }
}
