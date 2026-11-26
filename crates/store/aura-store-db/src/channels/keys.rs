//! Row keys of the channel column families; the formats are listed in
//! the [module docs](super#key-formats). Keys are plaintext in both
//! modes and must not change: they are the on-disk layout.

use super::validate::validate_id;
use aura_core_types::AgentId;

pub(super) const CHANNEL_PREFIX: &[u8] = b"c/";
const SLUG_PREFIX: &[u8] = b"s/";
const SEEN_PREFIX: &[u8] = b"l/";
pub(super) const LINK_PREFIX: &str = "p/";
const MEMBER_PREFIX: &str = "m/";
pub(super) const REVERSE_PREFIX: &[u8] = b"a/";

pub(super) fn channel_key(id: &str) -> Vec<u8> {
    [CHANNEL_PREFIX, id.as_bytes()].concat()
}

pub(super) fn slug_key(slug: &str) -> Vec<u8> {
    [SLUG_PREFIX, slug.as_bytes()].concat()
}

pub(super) fn seen_key(channel_id: &str) -> Vec<u8> {
    [SEEN_PREFIX, channel_id.as_bytes()].concat()
}

pub(super) fn link_prefix(channel_id: &str) -> Vec<u8> {
    format!("{LINK_PREFIX}{channel_id}/").into_bytes()
}

pub(super) fn link_key(channel_id: &str, project_id: &str) -> Vec<u8> {
    [link_prefix(channel_id), project_id.as_bytes().to_vec()].concat()
}

/// The channel id of a `p/{channel_id}/{project_id}` key, if the key has
/// that shape and the id is valid.
pub(super) fn link_key_channel(key: &[u8]) -> Option<&str> {
    let rest = key.strip_prefix(LINK_PREFIX.as_bytes())?;
    let end = rest.iter().position(|&b| b == b'/')?;
    let channel_id = std::str::from_utf8(rest.get(..end)?).ok()?;
    validate_id(channel_id).ok()?;
    Some(channel_id)
}

pub(super) fn member_prefix(channel_id: &str) -> Vec<u8> {
    format!("{MEMBER_PREFIX}{channel_id}/").into_bytes()
}

pub(super) fn member_key(channel_id: &str, agent: &AgentId) -> Vec<u8> {
    [member_prefix(channel_id).as_slice(), agent.as_bytes()].concat()
}

pub(super) fn reverse_key(agent: &AgentId) -> Vec<u8> {
    [REVERSE_PREFIX, agent.as_bytes()].concat()
}

pub(super) fn post_prefix(channel_id: &str) -> Vec<u8> {
    format!("{channel_id}/").into_bytes()
}

pub(super) fn post_key(channel_id: &str, at_ms: u64, seq: u64) -> Vec<u8> {
    format!("{channel_id}/{at_ms:020}/{seq:010}").into_bytes()
}

/// The sequence number at the end of a post key, if it parses.
pub(super) fn post_key_seq(key: &[u8]) -> Option<u64> {
    let tail = key.rsplit(|&b| b == b'/').next()?;
    std::str::from_utf8(tail).ok()?.parse().ok()
}
