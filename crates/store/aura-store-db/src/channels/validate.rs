//! Checks on slugs, channel ids and project ids before they reach a key
//! or a record.

use super::{ChannelError, ChannelUpdate};

const MAX_SLUG_LEN: usize = 40;

/// Whether `slug` is 1 to 40 characters of `a-z`, `0-9` and `-`.
#[must_use]
pub fn valid_slug(slug: &str) -> bool {
    !slug.is_empty()
        && slug.len() <= MAX_SLUG_LEN
        && slug
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

pub(super) fn validate_id(id: &str) -> Result<(), ChannelError> {
    if id.is_empty() || id.contains('/') {
        return Err(ChannelError::InvalidId(id.to_string()));
    }
    Ok(())
}

pub(super) fn validate_project_id(project_id: &str) -> Result<(), ChannelError> {
    if project_id.trim().is_empty() {
        return Err(ChannelError::InvalidProjectId(project_id.to_string()));
    }
    Ok(())
}

/// Check the project id an update would set, if any.
pub(super) fn validate_update(update: &ChannelUpdate) -> Result<(), ChannelError> {
    match &update.project_id {
        Some(Some(project_id)) => validate_project_id(project_id),
        Some(None) | None => Ok(()),
    }
}
