//! Channels: named rooms where a coordinator agent, the agents it
//! brings in and the user talk, stored in the agent-state database.
//!
//! # Key formats
//!
//! * [`crate::cf::CHANNELS`]:
//!   * `c/{channel_id}`: a JSON [`ChannelRecord`]. Its optional
//!     `project_id` is the one project the channel's agents work in.
//!   * `s/{slug}`: the JSON string of the id of the channel owning the
//!     slug (unique slug index).
//!   * `p/{channel_id}/{project_id}`: legacy, a JSON [`ProjectLink`] from
//!     when a channel could link many projects. Only
//!     [`ChannelStore::migrate_links`] (and the legacy
//!     [`ChannelStore::link_project`], [`ChannelStore::unlink_project`]
//!     and [`ChannelStore::projects`], kept until their callers move)
//!     touch these rows; the migration folds them into the `c/` record
//!     and deletes them.
//!   * `l/{channel_id}`: when the channel's coordinator last read its
//!     activity, JSON Unix milliseconds.
//! * [`crate::cf::CHANNEL_MEMBERS`]:
//!   * `m/{channel_id}/` followed by the 32 raw bytes of the agent id: a
//!     JSON [`MemberRecord`].
//!   * `a/` followed by the 32 raw bytes of the agent id: the JSON string
//!     of the id of the channel the agent is in (reverse index).
//! * [`crate::cf::CHANNEL_POSTS`], keyed
//!   `{channel_id}/{at_ms:020}/{seq:010}`: a JSON [`ChannelPost`]. Keys
//!   sort by time within a channel, ties by sequence number.
//!
//! A channel id is a UUID v4 string; ids containing `/` are rejected so
//! they can't bleed into a neighbouring key prefix. Every value is
//! sealed in sealed mode; keys are plaintext in both modes.
//!
//! # Invariants
//!
//! * The `s/` index and the `c/` rows are in step: every slug names
//!   exactly one channel, written in the same batch as its `c/` row.
//!   Archived channels keep their slug.
//! * The `m/` and `a/` rows are in step: a member row and its reverse
//!   index row are written and deleted in the same batch.
//! * An agent is in at most one channel. Check-then-write operations
//!   (slug uniqueness, membership) run under one write lock shared by
//!   every clone of a [`ChannelStore`]; callers that stage membership
//!   themselves ([`ChannelStore::stage_member`]) must check
//!   [`ChannelStore::channel_of`] under their own serialization.
//! * A channel and its coordinator (agent row, member row, reverse index)
//!   are committed together in one batch by [`ChannelStore::create`],
//!   the channel's project included.
//! * A channel has at most one project, held on its `c/` record. A
//!   stored project id is never empty or all whitespace. `c/` rows
//!   written before the field existed decode with no project.
//! * After [`ChannelStore::migrate_links`] a channel has no `p/` rows
//!   (unless its migration failed, or a legacy
//!   [`ChannelStore::link_project`] call adds one); each channel's
//!   migration is one batch, so its project is set and its `p/` rows
//!   deleted together.
//! * Post sequence numbers are unique across the database and grow with
//!   each append: the counter is seeded once from the highest sequence
//!   stored and appends are serialized, so sequence order is commit
//!   order.
//!
//! # Failure modes
//!
//! * Every multi-row change is one `WriteBatch`: all of it lands or none
//!   of it does.
//! * A value that fails to unseal (wrong key, tampering) or to decode
//!   fails the read: sealed values fail closed instead of dropping rows.
//! * A row whose key disagrees with its value (a `c/` row whose id is not
//!   its key, a member row whose agent id is not its key) is skipped with
//!   a `warn!` naming the key, never the payload. So is a slug index
//!   entry pointing at a missing channel.
//! * Sequence numbers above `9_999_999_999` still get unique keys, but
//!   two such posts with the same `at_ms` no longer sort by sequence.
//! * [`ChannelStore::stage_update`] takes no lock: an `update` or
//!   `archive` committed between staging and the caller's write is
//!   overwritten by the staged record. Callers serialize their channel
//!   changes, as with [`ChannelStore::stage_member`].
//! * [`ChannelStore::migrate_links`] logs a channel whose links cannot be
//!   migrated and carries on with the others; that channel keeps its rows
//!   and a rerun tries it again. Link rows with a malformed key are
//!   skipped (logged) and left in place.

mod error;
mod keys;
mod links;
mod members;
mod posts;
mod rows;
mod store;
mod types;
mod validate;

pub use error::ChannelError;
pub use types::{
    ChannelPost, ChannelRecord, ChannelUpdate, MemberRecord, MemberRole, NewChannel, NewPost,
    PostAuthor, ProjectLink,
};
pub use validate::valid_slug;

use crate::agents::AgentStore;
use crate::seal::SealCipher;
use rocksdb::{DBWithThreadMode, MultiThreaded};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

type Db = DBWithThreadMode<MultiThreaded>;

/// Channels, their members, projects and posts in the shared
/// agent-state `RocksDB`.
///
/// Every method is synchronous and blocking; async callers run them on
/// `spawn_blocking`.
#[derive(Clone)]
pub struct ChannelStore {
    db: Arc<Db>,
    /// `Some` in sealed mode.
    cipher: Option<Arc<SealCipher>>,
    sync_writes: bool,
    /// Stages the coordinator's agent row into the create batch.
    agents: AgentStore,
    /// Serializes check-then-write operations across clones.
    write_lock: Arc<Mutex<()>>,
    /// The last post sequence number assigned; `None` until seeded from
    /// the stored posts. Held across the post write so sequence order is
    /// commit order.
    last_post_seq: Arc<Mutex<Option<u64>>>,
}

impl std::fmt::Debug for ChannelStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChannelStore")
            .field("sealed", &self.cipher.is_some())
            .field("sync_writes", &self.sync_writes)
            .finish_non_exhaustive()
    }
}

/// Lock a mutex, recovering from poisoning: the guarded data is either
/// `()` or a counter only advanced after a successful write, so a panic
/// while holding it can't leave it inconsistent.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests;
