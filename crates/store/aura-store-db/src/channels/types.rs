//! The values stored in the channel column families and the inputs of
//! [`ChannelStore`](super::ChannelStore) methods. Their JSON layout is
//! the on-disk format: field order and serde attributes are load-bearing.

use aura_core_types::AgentId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize};

/// One channel, the value of a `c/{channel_id}` row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelRecord {
    /// UUID v4 string; the row key.
    pub id: String,
    /// Unique short name, see [`valid_slug`](super::valid_slug).
    pub slug: String,
    /// Display title.
    pub title: String,
    /// What the channel is for.
    pub description: String,
    /// The agent that runs the channel.
    pub coordinator: AgentId,
    /// When the channel was created.
    pub created_at: DateTime<Utc>,
    /// When the title, description, project or archived flag last
    /// changed through [`ChannelStore::update`](super::ChannelStore::update) or
    /// [`ChannelStore::archive`](super::ChannelStore::archive).
    pub updated_at: DateTime<Utc>,
    /// Moved to the Archive; still holds its slug.
    #[serde(default)]
    pub archived: bool,
    /// The project the channel's agents work in; `None` for none. Absent
    /// in rows written before channels had a project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    /// Where the channel's new members run. Left out of rows of local
    /// channels, and absent in rows written before channels had a
    /// placement: both read as [`ChannelPlacement::Local`].
    #[serde(default, skip_serializing_if = "ChannelPlacement::is_local")]
    pub placement: ChannelPlacement,
}

/// Where the members a channel starts run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelPlacement {
    /// On this computer.
    #[default]
    Local,
    /// Each on a cloud machine of its own.
    Cloud,
}

impl ChannelPlacement {
    /// Whether new members run on this computer.
    #[must_use]
    pub const fn is_local(&self) -> bool {
        matches!(self, Self::Local)
    }
}

/// What [`ChannelStore::create`](super::ChannelStore::create) needs to
/// make a channel.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewChannel {
    /// Unique short name, see [`valid_slug`](super::valid_slug).
    pub slug: String,
    /// Display title.
    pub title: String,
    /// What the channel is for.
    pub description: String,
    /// The project the channel's agents work in, if any; must not be
    /// empty or blank.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    /// Where the channel's new members run.
    #[serde(default)]
    pub placement: ChannelPlacement,
}

/// Fields [`ChannelStore::update`](super::ChannelStore::update) changes;
/// `None` leaves one as it is.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelUpdate {
    /// New title.
    pub title: Option<String>,
    /// New description.
    pub description: Option<String>,
    /// New project: `Some(None)` clears it, `Some(Some(id))` sets it (the
    /// id must not be empty or blank). In JSON an absent field is `None`
    /// and `null` is `Some(None)`.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_option"
    )]
    pub project_id: Option<Option<String>>,
    /// Where the channel's new members run from now on; members it has
    /// stay where they are.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placement: Option<ChannelPlacement>,
}

/// Deserialize a field that is present (possibly `null`) as `Some`, so
/// together with `#[serde(default)]` an absent field stays `None` and
/// `null` becomes `Some(None)`.
fn present_option<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

/// A project linked to a channel, the value of a legacy
/// `p/{channel_id}/{project_id}` row; see
/// [`ChannelStore::migrate_links`](super::ChannelStore::migrate_links).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectLink {
    /// The linked project.
    pub project_id: String,
    /// When it was linked.
    pub linked_at: DateTime<Utc>,
}

/// What an agent does in a channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemberRole {
    /// Runs the channel.
    Coordinator,
    /// Brought in as a full member.
    Agent,
    /// Spawned by another member for a subtask.
    Subagent,
}

/// One member of a channel, the value of an `m/` row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemberRecord {
    /// The member; the last 32 bytes of the row key.
    pub agent_id: AgentId,
    /// What it does in the channel.
    pub role: MemberRole,
    /// The member that brought it in, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<AgentId>,
    /// When it joined.
    pub joined_at: DateTime<Utc>,
}

/// Who wrote a post.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PostAuthor {
    /// The human user.
    User,
    /// An agent.
    Agent(AgentId),
}

/// One post in a channel, the value of a [`crate::cf::CHANNEL_POSTS`]
/// row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelPost {
    /// The channel posted to.
    pub channel_id: String,
    /// Unique, growing sequence number assigned on append.
    pub seq: u64,
    /// Who wrote it.
    pub author: PostAuthor,
    /// The text.
    pub text: String,
    /// Agents mentioned.
    #[serde(default)]
    pub mentions: Vec<AgentId>,
    /// `seq` of the post replied to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<u64>,
    /// When it was posted, milliseconds since the Unix epoch.
    pub at_ms: u64,
    /// For the user's `/plan` request, the member started to plan it. Such
    /// a post is not part of the channel's conversation: only its HISTORY
    /// lists it. Absent in rows written before it existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub planner: Option<AgentId>,
}

/// What [`ChannelStore::append_post`](super::ChannelStore::append_post)
/// needs to make a post.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewPost {
    /// Who wrote it.
    pub author: PostAuthor,
    /// The text.
    pub text: String,
    /// Agents mentioned.
    pub mentions: Vec<AgentId>,
    /// `seq` of the post replied to.
    pub reply_to: Option<u64>,
    /// When it was posted, milliseconds since the Unix epoch.
    pub at_ms: u64,
    /// For a `/plan` request, the member planning it
    /// ([`ChannelPost::planner`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub planner: Option<AgentId>,
}
