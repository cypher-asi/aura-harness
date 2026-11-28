//! Channel posts: appending with a database-wide sequence number and
//! reading a channel's posts in time order.

use super::keys::{post_key, post_key_seq, post_prefix};
use super::validate::validate_id;
use super::{lock, ChannelError, ChannelPost, ChannelStore, NewPost};
use crate::cf;
use rocksdb::{IteratorMode, WriteBatch};

impl ChannelStore {
    /// Append a post to channel `channel_id`, assigning it the next
    /// sequence number.
    ///
    /// # Errors
    /// [`ChannelError::UnknownChannel`]; store, sealing and encoding
    /// failures. No sequence number is used up on error.
    pub fn append_post(&self, channel_id: &str, new: NewPost) -> Result<ChannelPost, ChannelError> {
        self.require_channel(channel_id)?;
        let mut last = lock(&self.last_post_seq);
        let seq = match *last {
            Some(seq) => seq,
            None => self.max_post_seq()?,
        }
        .saturating_add(1);
        let post = ChannelPost {
            channel_id: channel_id.to_string(),
            seq,
            author: new.author,
            text: new.text,
            mentions: new.mentions,
            reply_to: new.reply_to,
            at_ms: new.at_ms,
            planner: new.planner,
        };
        let what = format!("post {seq} of channel {channel_id}");
        let value = self.encode(&post, &what)?;
        let mut batch = WriteBatch::default();
        batch.put_cf(
            &self.cf(cf::CHANNEL_POSTS)?,
            post_key(channel_id, post.at_ms, seq),
            value,
        );
        self.commit(batch, &what)?;
        *last = Some(seq);
        Ok(post)
    }

    /// The highest sequence number in any post key, 0 when there are
    /// none. Reads keys only, so nothing is unsealed.
    fn max_post_seq(&self) -> Result<u64, ChannelError> {
        let cf = self.cf(cf::CHANNEL_POSTS)?;
        let mut max = 0;
        for item in self.db.iterator_cf(&cf, IteratorMode::Start) {
            let (key, _) = item.map_err(|source| ChannelError::Backend {
                context: "reading channel posts".to_string(),
                source,
            })?;
            match post_key_seq(&key) {
                Some(seq) => max = max.max(seq),
                None => tracing::warn!(
                    key_len = key.len(),
                    "skipping post row with a malformed key"
                ),
            }
        }
        Ok(max)
    }

    /// Posts of channel `channel_id` in time order (ties by sequence
    /// number); with `since_ms`, only those with `at_ms > since_ms`.
    ///
    /// # Errors
    /// [`ChannelError::InvalidId`]; store, unsealing and decoding
    /// failures.
    pub fn posts(
        &self,
        channel_id: &str,
        since_ms: Option<u64>,
    ) -> Result<Vec<ChannelPost>, ChannelError> {
        validate_id(channel_id)?;
        let prefix = post_prefix(channel_id);
        let start = match since_ms {
            None => prefix.clone(),
            Some(since) => match since.checked_add(1) {
                Some(from) => format!("{channel_id}/{from:020}/").into_bytes(),
                None => return Ok(Vec::new()),
            },
        };
        let what = format!("posts of channel {channel_id}");
        self.scan(cf::CHANNEL_POSTS, &prefix, &start, &what)?
            .into_iter()
            .map(|(_, value)| self.decode(&value, &what))
            .collect()
    }
}
