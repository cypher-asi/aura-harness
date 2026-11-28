//! The projects list in the agents panel: the projects, the highlighted
//! row, its scroll, the folded rows and a project being dragged.

use super::folder_picker::FolderPicker;
use super::project_drag::ProjectPress;
use crate::events::ProjectSummary;
use std::collections::HashSet;

/// The projects list: projects, highlight, scroll, folds and drags.
#[derive(Debug, Default)]
pub(crate) struct ProjectList {
    /// Listed projects, in list order
    pub(crate) projects: Vec<ProjectSummary>,
    /// Key of the project whose details are shown
    pub(crate) project_info: Option<String>,
    /// Highlighted row of the projects list (headers, agents, "+ New project")
    pub(crate) selected: usize,
    /// First visible row of the agents list
    pub(crate) scroll: usize,
    /// Root-agent ids whose project row is folded shut
    pub(crate) collapsed_projects: HashSet<String>,
    /// Ids of agents folded shut, hiding the agents they spawned
    pub(crate) collapsed_agents: HashSet<String>,
    /// The left button is down on a project's header: a click folds it, a
    /// drag moves it
    pub(crate) project_press: Option<ProjectPress>,
    /// Times a drag has reordered the projects, so the screen is redrawn
    pub(crate) project_moves: usize,
    /// Archived agents opened over the agents under them; every other one
    /// with agents under it is folded. Kept while another app is in front.
    pub(crate) opened_archived: HashSet<String>,
    /// Folder browser for a new project, while it is open
    pub(crate) folder_picker: Option<FolderPicker>,
}
