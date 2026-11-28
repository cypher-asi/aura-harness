//! Where a plan came from, as its frontmatter says: the keys
//! `create_plan` stamps (see `aura_exec_tools::plan_tools::PlanOrigin`).
//! A plan from before them has none, and every field is `None`.

use super::frontmatter::Frontmatter;
use chrono::{DateTime, Utc};

/// When, by whom and where a plan was written.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlanOrigin {
    /// When it was first written
    pub created_at: Option<DateTime<Utc>>,
    /// When it was last rewritten
    pub updated_at: Option<DateTime<Utc>>,
    /// Id (hex) of the agent that wrote it
    pub author: Option<String>,
    /// Model the author ran on
    pub model: Option<String>,
    /// A cloud machine's hostname, or `local`
    pub machine: Option<String>,
}

impl PlanOrigin {
    /// The origin `meta` records; a key that is missing, blank or not a
    /// time is left out.
    pub(super) fn from_frontmatter(meta: &Frontmatter) -> Self {
        Self {
            created_at: time(&meta.created_at),
            updated_at: time(&meta.updated_at),
            author: text(&meta.author),
            model: text(&meta.model),
            machine: text(&meta.machine),
        }
    }
}

fn text(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty() && value != "null" && value != "~").then(|| value.to_string())
}

fn time(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value.trim())
        .ok()
        .map(|at| at.with_timezone(&Utc))
}
