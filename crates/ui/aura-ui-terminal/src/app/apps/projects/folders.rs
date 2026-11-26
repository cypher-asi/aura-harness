//! The folder browser for a new project, dropped folders, and a project's
//! details.

use crate::app::folder_picker::FolderPicker;
use crate::app::{App, AppState, NotificationType};
use crate::events::{ProjectSummary, UiEvent};
use std::path::PathBuf;

impl App {
    /// Ask for a folder, then open it as a project named after it.
    /// Browsing starts in the launch directory.
    pub(in crate::app) fn prompt_new_project(&mut self) {
        let start = std::env::current_dir()
            .ok()
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("/"));
        match FolderPicker::open(start) {
            Ok(picker) => {
                self.project_list.folder_picker = Some(picker);
                self.state = AppState::SelectingFolder;
                self.status = "Project folder".to_string();
            }
            Err(e) => {
                self.notifications.current = Some((
                    format!("Cannot browse folders: {e}"),
                    NotificationType::Error,
                ));
            }
        }
    }

    /// Open the folder browser for a new project, as Ctrl+N does.
    pub fn ask_for_project_folder(&mut self) {
        self.prompt_new_project();
    }

    /// Close the folder browser and open the folder as a project.
    pub(in crate::app) fn choose_project_folder(&mut self, folder: PathBuf) {
        self.project_list.folder_picker = None;
        self.state = AppState::Idle;
        self.status = "Ready".to_string();
        if let Some(tx) = &self.event_tx {
            let _ = tx.try_send(UiEvent::CreateProject { folder });
        }
    }

    /// Close the folder browser without creating a project.
    pub(in crate::app) fn cancel_folder_picker(&mut self) {
        self.project_list.folder_picker = None;
        self.state = AppState::Idle;
        self.status = "Ready".to_string();
        self.notifications.current = Some(("Cancelled".to_string(), NotificationType::Warning));
    }

    /// A path was dropped on the window. A folder becomes a project named
    /// after it, with the same event the folder browser sends; anything
    /// else is refused with a notice. Returns whether a project was asked
    /// for.
    pub fn drop_path(&mut self, path: &std::path::Path) -> bool {
        if !path.is_dir() {
            self.notifications.current = Some((
                "Drop a folder to start a project".to_string(),
                NotificationType::Warning,
            ));
            return false;
        }
        let Some(tx) = &self.event_tx else {
            return false;
        };
        let event = UiEvent::CreateProject {
            folder: path.to_path_buf(),
        };
        if tx.try_send(event).is_err() {
            self.notifications.current = Some((
                "Cannot add the project right now".to_string(),
                NotificationType::Error,
            ));
            return false;
        }
        true
    }

    /// Folder browser for a new project, while it is open.
    #[must_use]
    pub const fn folder_picker(&self) -> Option<&FolderPicker> {
        self.project_list.folder_picker.as_ref()
    }

    /// Keep the folder browser's highlight inside a list `visible_rows`
    /// tall, returning the first row to show.
    pub fn sync_folder_picker_scroll(&mut self, visible_rows: usize) -> usize {
        self.project_list
            .folder_picker
            .as_mut()
            .map_or(0, |picker| picker.sync_scroll(visible_rows))
    }

    /// Listed projects, in list order.
    #[must_use]
    pub fn projects(&self) -> &[ProjectSummary] {
        &self.project_list.projects
    }

    /// The project whose details are shown, while they are.
    #[must_use]
    pub fn project_info(&self) -> Option<&ProjectSummary> {
        if self.state != AppState::ShowingProjectInfo {
            return None;
        }
        let key = self.project_list.project_info.as_deref()?;
        self.project_list
            .projects
            .iter()
            .find(|project| project.id == key)
    }

    /// Show the details of the project with key `key`.
    pub(in crate::app) fn show_project_info(&mut self, key: &str) {
        if !self
            .project_list
            .projects
            .iter()
            .any(|project| project.id == key)
        {
            self.notifications.current = Some((
                "No details for that project".to_string(),
                NotificationType::Warning,
            ));
            return;
        }
        self.project_list.project_info = Some(key.to_string());
        self.state = AppState::ShowingProjectInfo;
    }

    /// Show the details of the project the active agent is in.
    pub(in crate::app) fn show_active_project_info(&mut self) {
        match self.project_key_of_agent(&self.active_agent_id) {
            Some(key) => self.show_project_info(&key),
            None => {
                self.notifications.current =
                    Some(("No project is open".to_string(), NotificationType::Warning));
            }
        }
    }

    /// Close the project details.
    pub(in crate::app) fn close_project_info(&mut self) {
        self.project_list.project_info = None;
        self.state = AppState::Idle;
    }
}
