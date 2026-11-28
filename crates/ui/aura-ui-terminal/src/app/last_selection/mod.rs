//! What the agents panel had selected when the app last closed, kept in
//! a file between runs so the next run opens on it: the channel CHANNELS
//! had open, and the agent PROJECTS was on in each project.
//!
//! ## File
//!
//! [`SELECTION_FILE`], JSON, in the session's data directory beside the
//! agent registry and the desktop's tab arrangement: the ids in it are
//! that registry's. Every field is optional, so a file from an older or
//! newer build still reads.
//!
//! ## Failure modes
//!
//! A missing, unreadable or corrupt file reads as [`LastSelection::default`],
//! which leaves the app on its usual first screen; nothing is shown to the
//! user. Saving writes a temporary file and renames it over the old one,
//! so a crash mid-write leaves the previous selection, never half of one.
//! Ids that no longer name a listed channel or agent are skipped when the
//! selection is brought back (see `apps::remembered`).

use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};

#[cfg(test)]
mod tests;

/// Name of the file in the data directory.
pub const SELECTION_FILE: &str = "ui_selection.json";

/// Most projects whose agent is remembered; the oldest go first.
const MAX_PICKS: usize = 64;

/// Largest file read back: far more than [`MAX_PICKS`] picks take, so a
/// stray huge file is ignored rather than parsed.
const MAX_FILE_BYTES: u64 = 256 * 1024;

/// What the agents panel had selected, to select again next run.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LastSelection {
    /// Whether CHANNELS was the app in front
    pub channels_in_front: bool,
    /// Id of the channel CHANNELS had open, or would open on coming back
    pub channel: Option<String>,
    /// The agent PROJECTS was last on in each project, the latest first,
    /// one per project
    pub projects: Vec<ProjectPick>,
}

/// The agent PROJECTS was last on in one project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectPick {
    /// Key of the project (`AgentSummary::project_key`)
    pub project: String,
    /// Hex id of the agent
    pub agent: String,
}

impl LastSelection {
    /// The agent PROJECTS was on last, in any project.
    #[must_use]
    pub fn latest_agent(&self) -> Option<&str> {
        self.projects.first().map(|pick| pick.agent.as_str())
    }

    /// Where the selection is kept for the data directory `data_dir`.
    #[must_use]
    pub fn file_in(data_dir: &Path) -> PathBuf {
        data_dir.join(SELECTION_FILE)
    }

    /// The selection saved at `path`, or the default when there is none
    /// or it can't be read. Blocking: call it off the async runtime's
    /// worker threads.
    #[must_use]
    pub fn load(path: &Path) -> Self {
        match read_capped(path) {
            Ok(text) => Self::parse(&text),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Self::default(),
            Err(e) => {
                tracing::debug!(error = %e, path = %path.display(), "could not read the last selection");
                Self::default()
            }
        }
    }

    /// The selection in `text`, or the default when it isn't one.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        match serde_json::from_str::<Self>(text) {
            Ok(selection) => selection.normalized(),
            Err(e) => {
                tracing::debug!(error = %e, "the last selection is not valid; starting without it");
                Self::default()
            }
        }
    }

    /// Write the selection to `path` for [`Self::load`]. Blocking, like
    /// [`Self::load`].
    ///
    /// # Errors
    ///
    /// Fails when the file can't be written or moved into place.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        let text = serde_json::to_string_pretty(&self.clone().normalized())?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text + "\n")?;
        std::fs::rename(&tmp, path)
    }

    /// The same selection, but for a tab that should not open CHANNELS:
    /// what each project was on, and the channel to open on going there.
    #[must_use]
    pub fn without_channels_in_front(mut self) -> Self {
        self.channels_in_front = false;
        self
    }

    /// Note `agent` as the one PROJECTS was last on in `project`, ahead
    /// of every other project.
    pub fn note_pick(&mut self, project: &str, agent: &str) {
        note_pick(&mut self.projects, project, agent);
    }

    /// Drop empty ids and later picks of a project already picked, and
    /// keep at most [`MAX_PICKS`].
    fn normalized(mut self) -> Self {
        self.channel = self.channel.filter(|id| !id.is_empty());
        let mut kept: Vec<ProjectPick> = Vec::with_capacity(self.projects.len().min(MAX_PICKS));
        for pick in self.projects {
            let fresh = !kept.iter().any(|known| known.project == pick.project);
            if fresh && !pick.project.is_empty() && !pick.agent.is_empty() {
                kept.push(pick);
            }
        }
        kept.truncate(MAX_PICKS);
        self.projects = kept;
        self
    }
}

/// Put `agent` first in `picks` as `project`'s, replacing the project's
/// earlier pick.
pub(crate) fn note_pick(picks: &mut Vec<ProjectPick>, project: &str, agent: &str) {
    if project.is_empty() || agent.is_empty() {
        return;
    }
    picks.retain(|pick| pick.project != project);
    picks.insert(
        0,
        ProjectPick {
            project: project.to_string(),
            agent: agent.to_string(),
        },
    );
    picks.truncate(MAX_PICKS);
}

/// The text of the file at `path`, refusing one over [`MAX_FILE_BYTES`].
fn read_capped(path: &Path) -> io::Result<String> {
    let size = std::fs::metadata(path)?.len();
    if size > MAX_FILE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{size} bytes, more than {MAX_FILE_BYTES}"),
        ));
    }
    std::fs::read_to_string(path)
}
