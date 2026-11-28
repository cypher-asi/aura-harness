//! Summaries of the user's long messages, kept for the channels' HISTORY
//! tab so each message is summarized by a model once, across restarts.

use super::keys::summary_key;
use super::{ChannelError, ChannelStore};
use crate::cf;
use rocksdb::WriteBatch;

/// Longest summary key: a 256-bit hash in hex.
const MAX_KEY_LEN: usize = 64;

/// Check that `key` is 1 to [`MAX_KEY_LEN`] lowercase hex digits, so it
/// can't reach outside the `h/` rows.
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
    /// The summary kept for the message keyed `key`; `None` when there
    /// is none yet.
    ///
    /// # Errors
    /// [`ChannelError::InvalidSummaryKey`]; unsealing, decoding and store
    /// failures.
    pub fn post_summary(&self, key: &str) -> Result<Option<String>, ChannelError> {
        validate_summary_key(key)?;
        self.read(
            cf::CHANNELS,
            &summary_key(key),
            &format!("post summary {key}"),
        )
    }

    /// Keep each `(key, summary)` of `summaries`, replacing any kept
    /// before, in one batch: all of them land or none does.
    ///
    /// # Errors
    /// [`ChannelError::InvalidSummaryKey`] before anything is written;
    /// sealing, encoding and store failures.
    pub fn set_post_summaries(&self, summaries: &[(String, String)]) -> Result<(), ChannelError> {
        let family = self.cf(cf::CHANNELS)?;
        let mut batch = WriteBatch::default();
        for (key, summary) in summaries {
            validate_summary_key(key)?;
            let value = self.encode(summary, &format!("post summary {key}"))?;
            batch.put_cf(&family, summary_key(key), value);
        }
        self.commit(batch, "post summaries")
    }
}
