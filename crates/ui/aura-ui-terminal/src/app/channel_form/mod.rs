//! The channel form: "+ New channel" opens it to make a channel, and the
//! channel menu's "Edit channel…" opens it on a channel to change it.
//! Its fields are a slug, a title and a description, each a line edited
//! as the input line is (see [`edit_line`]), then the project the
//! channel's agents work in, a chooser of "None", every listed project,
//! in list order, and "New project", and where the members it starts run
//! ("Agents run": locally or in the cloud).
//!
//! Keys:
//!
//! - Tab or Down goes to the next field, Shift+Tab or Up to the one
//!   before; both wrap.
//! - On the project and on where agents run, Left goes to the choice
//!   before and Right or Space to the next one, wrapping; other typing is
//!   ignored there.
//! - Enter on "New project" opens the folder browser the projects list
//!   uses. Esc there, or a press off it, returns to the form. A picked
//!   folder creates the project without leaving the form, which selects
//!   it once the project is listed. Enter otherwise goes to the next
//!   field, and on the last one (Agents run) submits. Ctrl+Enter submits
//!   from any field. On "New project", before that project exists, it
//!   stays there and sends nothing.
//! - Esc closes the form and sends nothing, as does a press off it.
//!
//! The slug must match `[a-z0-9-]{1,40}`. Uppercase letters typed into it
//! are lowercased as they are typed, so `Release` becomes `release`; any
//! other character is kept, so the form can say why the slug is refused.
//! A submit with a bad slug sends nothing and puts the cursor back in the
//! slug. An empty title becomes the slug.
//!
//! Agents run in the cloud only in a project, whose folder their machines
//! start from: a submit with "In the cloud" and no project sends nothing
//! and puts the cursor on the project.
//!
//! A new channel's project starts as the one of the agent that was on
//! screen before CHANNELS took over, when that project is listed, and its
//! agents run locally. The choices follow the projects list while the
//! form is open; a chosen project that drops out of it falls back to
//! "None".
//!
//! Submitting a new channel sends [`UiEvent::CreateChannel`] and closes
//! the form; the session opens the channel once it has made it.
//!
//! Editing shows the slug but does not let it change: Tab skips it and
//! the cursor starts on the title. The channel's own project stays a
//! choice even when the projects list leaves it out, so saving without
//! touching it keeps it. An empty title keeps the one the channel has.
//! Submitting sends [`UiEvent::UpdateChannel`] with every field, so
//! "None" clears the project. Where agents run applies to the members the
//! channel starts from then on; the ones it has stay where they are.

use super::line_edit::edit_line;
use crate::events::{ChannelProjectView, ChannelSummary, ProjectSummary, UiEvent};
use crate::host::PlacementView;
use crossterm::event::{KeyCode, KeyEvent};
pub(crate) use field::ChannelField;
use std::path::{Path, PathBuf};

/// Longest slug, in characters.
const MAX_SLUG: usize = 40;

/// What the form says under a slug it refuses.
pub(crate) const SLUG_HINT: &str = "lowercase letters, digits and -, up to 40";

/// What the form says under "New project" before a folder is picked.
pub(crate) const NEW_PROJECT_HINT: &str = "Enter picks a folder";

/// What the project chooser is on.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
enum ProjectPick {
    /// No project
    #[default]
    None,
    /// A listed project, by id
    Listed(String),
    /// Make one by picking its folder
    New,
}

/// What the form does on submit.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) enum FormMode {
    /// Make a new channel
    #[default]
    Create,
    /// Change an existing channel
    Edit {
        /// Channel id
        channel_id: String,
        /// Its slug, which does not change
        slug: String,
        /// Its title, kept when the title field is left empty
        title: String,
        /// Its project, kept as a choice even if the projects list
        /// leaves it out
        project: Option<ChannelProjectView>,
    },
}

/// A field's text and its cursor, a byte offset on a char boundary.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct FieldText {
    text: String,
    cursor: usize,
}

impl FieldText {
    /// `text` with the cursor at its end.
    fn at_end(text: &str) -> Self {
        Self {
            text: text.to_string(),
            cursor: text.len(),
        }
    }
}

/// The channel form's state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ChannelForm {
    /// Whether it makes or changes a channel
    mode: FormMode,
    /// Slug, title and description, by [`ChannelField::text_index`]
    fields: [FieldText; 3],
    /// Projects it offers besides "None" and "New project", in order
    choices: Vec<ChannelProjectView>,
    /// Which project is chosen
    pick: ProjectPick,
    /// Folder of a project just asked for, until that project is listed
    pending_folder: Option<PathBuf>,
    /// Where the members the channel starts run
    placement: PlacementView,
    /// Field the keys go to
    focus: ChannelField,
}

/// The projects list as the form offers it.
fn listed_choices(projects: &[ProjectSummary]) -> Vec<ChannelProjectView> {
    projects
        .iter()
        .map(|project| ChannelProjectView {
            id: project.id.clone(),
            name: project.name.clone(),
            folder: project.folder.clone(),
        })
        .collect()
}

impl ChannelForm {
    /// An empty form for a new channel, on its slug, with `project`
    /// chosen when it is one of `projects`.
    fn create(projects: &[ProjectSummary], project: Option<String>) -> Self {
        let mut form = Self::default();
        form.set_projects(projects);
        form.pick = project.map_or(ProjectPick::None, ProjectPick::Listed);
        form.drop_vanished_project();
        form
    }

    /// A form filled from `channel`, to change it, on its title.
    fn edit(channel: &ChannelSummary, projects: &[ProjectSummary]) -> Self {
        let mut form = Self {
            mode: FormMode::Edit {
                channel_id: channel.id.clone(),
                slug: channel.slug.clone(),
                title: channel.title.clone(),
                project: channel.project.clone(),
            },
            fields: [
                FieldText::at_end(&channel.slug),
                FieldText::at_end(&channel.title),
                FieldText::at_end(&channel.description),
            ],
            pick: channel
                .project
                .as_ref()
                .map_or(ProjectPick::None, |project| {
                    ProjectPick::Listed(project.id.clone())
                }),
            placement: channel.placement,
            focus: ChannelField::Title,
            ..Self::default()
        };
        form.set_projects(projects);
        form
    }

    /// Whether it makes or changes a channel.
    pub(crate) const fn mode(&self) -> &FormMode {
        &self.mode
    }

    /// Whether it changes an existing channel.
    pub(crate) const fn is_edit(&self) -> bool {
        matches!(self.mode, FormMode::Edit { .. })
    }

    /// Text typed into `field`; empty for the choosers, which are no text.
    pub(crate) fn text(&self, field: ChannelField) -> &str {
        self.field(field).map_or("", |field| field.text.as_str())
    }

    /// Cursor of `field`, a byte offset into [`Self::text`].
    pub(crate) fn cursor(&self, field: ChannelField) -> usize {
        self.field(field).map_or(0, |field| field.cursor)
    }

    /// Field the keys go to.
    pub(crate) const fn focus(&self) -> ChannelField {
        self.focus
    }

    /// The chosen project; `None` for "None" and for "New project".
    pub(crate) fn project(&self) -> Option<&ChannelProjectView> {
        let ProjectPick::Listed(id) = &self.pick else {
            return None;
        };
        self.choices.iter().find(|choice| choice.id == *id)
    }

    /// Whether the chooser is on "New project".
    pub(crate) const fn is_new_project(&self) -> bool {
        matches!(self.pick, ProjectPick::New)
    }

    /// What the chooser shows: "None", "New project", or the project's name.
    pub(crate) fn project_label(&self) -> &str {
        if self.is_new_project() {
            "New project"
        } else if let Some(project) = self.project() {
            project.name.as_str()
        } else {
            "None"
        }
    }

    /// Folder of a project just asked for, until that project is listed.
    pub(crate) fn pending_folder(&self) -> Option<&Path> {
        self.pending_folder.as_deref()
    }

    /// Remember `folder` until the project made from it is listed, and
    /// stay on "New project" meanwhile.
    fn await_created_folder(&mut self, folder: PathBuf) {
        self.pick = ProjectPick::New;
        self.pending_folder = Some(folder);
    }

    /// Where the members the channel starts run.
    pub(crate) const fn placement(&self) -> PlacementView {
        self.placement
    }

    /// Whether agents are to run in the cloud with no project to work
    /// in, which the form refuses.
    pub(crate) fn cloud_without_project(&self) -> bool {
        self.placement == PlacementView::Cloud && self.project().is_none()
    }

    /// Whether `field` takes the cursor: all but the slug of a channel
    /// being edited.
    pub(crate) fn is_focusable(&self, field: ChannelField) -> bool {
        !(self.is_edit() && field == ChannelField::Slug)
    }

    /// Whether the slug typed so far would be taken.
    pub(crate) fn slug_is_valid(&self) -> bool {
        is_valid_slug(self.text(ChannelField::Slug))
    }

    fn field(&self, field: ChannelField) -> Option<&FieldText> {
        self.fields.get(field.text_index()?)
    }

    fn field_mut(&mut self, field: ChannelField) -> Option<&mut FieldText> {
        self.fields.get_mut(field.text_index()?)
    }

    /// Move the cursor to the next focusable field, or the one before.
    fn move_focus(&mut self, forward: bool) {
        let step = |field: ChannelField| {
            if forward {
                field.next()
            } else {
                field.previous()
            }
        };
        let mut focus = step(self.focus);
        while !self.is_focusable(focus) {
            focus = step(focus);
        }
        self.focus = focus;
    }

    /// Offer `projects` (and, while editing, the channel's own project),
    /// dropping a chosen project that is no longer among them.
    fn set_projects(&mut self, projects: &[ProjectSummary]) {
        let mut choices = listed_choices(projects);
        if let FormMode::Edit {
            project: Some(own), ..
        } = &self.mode
        {
            if !choices.iter().any(|choice| choice.id == own.id) {
                choices.push(own.clone());
            }
        }
        self.choices = choices;
        self.claim_created_project();
        self.drop_vanished_project();
    }

    /// Select the project made from the folder the form is waiting on,
    /// once it is listed.
    fn claim_created_project(&mut self) {
        let Some(folder) = self.pending_folder.clone() else {
            return;
        };
        let id = self
            .choices
            .iter()
            .find(|choice| folders_match(&choice.folder, &folder))
            .map(|choice| choice.id.clone());
        if let Some(id) = id {
            self.pick = ProjectPick::Listed(id);
            self.pending_folder = None;
        }
    }

    fn drop_vanished_project(&mut self) {
        let ProjectPick::Listed(id) = &self.pick else {
            return;
        };
        if !self.choices.iter().any(|choice| choice.id == *id) {
            self.pick = ProjectPick::None;
            self.pending_folder = None;
        }
    }

    /// Choose the project after the chosen one, or the one before. "None"
    /// is first, "New project" is last, and the listed projects are
    /// between them. Leaving "New project" forgets a folder it was
    /// waiting on.
    fn cycle_project(&mut self, forward: bool) {
        let listed = self.choices.len();
        let positions = listed + 2;
        let current = match &self.pick {
            ProjectPick::None => 0,
            ProjectPick::Listed(id) => self
                .choices
                .iter()
                .position(|choice| choice.id == *id)
                .map_or(0, |index| index + 1),
            ProjectPick::New => listed + 1,
        };
        let next = if forward {
            (current + 1) % positions
        } else {
            (current + positions - 1) % positions
        };
        let leaving_new = matches!(self.pick, ProjectPick::New) && next != listed + 1;
        let listed_id = next
            .checked_sub(1)
            .filter(|index| *index < listed)
            .map(|index| self.choices[index].id.clone());
        self.pick = match listed_id {
            Some(id) => ProjectPick::Listed(id),
            None if next == listed + 1 => ProjectPick::New,
            None => ProjectPick::None,
        };
        if leaving_new {
            self.pending_folder = None;
        }
    }

    /// Apply a key to the focused field: an editing key to a text field,
    /// Left, Right or Space to a chooser. Returns whether it was one.
    fn edit_key(&mut self, key: KeyEvent) -> bool {
        if matches!(self.focus, ChannelField::Project | ChannelField::Placement) {
            let forward = match key.code {
                KeyCode::Left => false,
                KeyCode::Right | KeyCode::Char(' ') => true,
                _ => return false,
            };
            if self.focus == ChannelField::Project {
                self.cycle_project(forward);
            } else {
                // Two choices: either way is the other one.
                self.placement = self.placement.flipped();
            }
            return true;
        }
        let key = match (self.focus, key.code) {
            (ChannelField::Slug, KeyCode::Char(c)) if c.is_ascii_uppercase() => {
                KeyEvent::new(KeyCode::Char(c.to_ascii_lowercase()), key.modifiers)
            }
            _ => key,
        };
        let Some(field) = self.field_mut(self.focus) else {
            return false;
        };
        edit_line(&mut field.text, &mut field.cursor, key).is_some()
    }

    /// Type `text` into the focused field at its cursor, as a paste does.
    /// Line breaks are dropped: every field is one line. Nothing goes
    /// into the choosers.
    fn insert(&mut self, text: &str) {
        let slug = self.focus == ChannelField::Slug;
        let Some(field) = self.field_mut(self.focus) else {
            return;
        };
        for c in text.chars().filter(|c| !matches!(c, '\r' | '\n')) {
            let c = if slug { c.to_ascii_lowercase() } else { c };
            let pos = field.cursor.min(field.text.len());
            field.text.insert(pos, c);
            field.cursor = pos + c.len_utf8();
        }
    }

    /// The event the form sends, or the field to fix first: the slug of a
    /// new channel while it is refused, the project while "New project"
    /// has no project yet, or the project while agents are to run in the
    /// cloud without one.
    fn submission(&self) -> Result<UiEvent, ChannelField> {
        if !self.is_edit() && !self.slug_is_valid() {
            return Err(ChannelField::Slug);
        }
        if self.is_new_project() || self.cloud_without_project() {
            return Err(ChannelField::Project);
        }
        let typed_title = self.text(ChannelField::Title).trim();
        let description = self.text(ChannelField::Description).trim().to_string();
        let project_id = self.project().map(|project| project.id.clone());
        let placement = self.placement;
        Ok(match &self.mode {
            FormMode::Create => {
                let slug = self.text(ChannelField::Slug).to_string();
                let title = match typed_title {
                    "" => slug.clone(),
                    title => title.to_string(),
                };
                UiEvent::CreateChannel {
                    slug,
                    title,
                    description,
                    project_id,
                    placement,
                }
            }
            FormMode::Edit {
                channel_id,
                slug,
                title,
                ..
            } => {
                let title = match (typed_title, title.trim()) {
                    ("", "") => slug.clone(),
                    ("", _) => title.clone(),
                    (typed, _) => typed.to_string(),
                };
                UiEvent::UpdateChannel {
                    channel_id: channel_id.clone(),
                    title,
                    description,
                    project_id,
                    placement,
                }
            }
        })
    }
}

/// Whether `listed` is the folder `picked`, once both are canonical when
/// they can be. The registry stores a canonical path; the browser may
/// hand back one that is not.
fn folders_match(listed: &str, picked: &Path) -> bool {
    let listed = Path::new(listed);
    if listed == picked {
        return true;
    }
    match (listed.canonicalize(), picked.canonicalize()) {
        (Ok(listed), Ok(picked)) => listed == picked,
        _ => false,
    }
}

/// Whether `slug` matches `[a-z0-9-]{1,40}`.
pub(crate) fn is_valid_slug(slug: &str) -> bool {
    (1..=MAX_SLUG).contains(&slug.len())
        && slug
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

mod app;
mod field;
#[cfg(test)]
mod tests;
