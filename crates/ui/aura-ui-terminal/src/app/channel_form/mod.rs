//! The channel form: "+ New channel" opens it to make a channel, and the
//! channel menu's "Edit channel…" opens it on a channel to change it.
//! Its fields are a slug, a title and a description, each a line edited
//! as the input line is (see [`edit_line`]), then the project the
//! channel's agents work in, a chooser of "None" and every listed
//! project, in list order, and where the members it starts run ("Agents
//! run": locally or in the cloud).
//!
//! Keys:
//!
//! - Tab or Down goes to the next field, Shift+Tab or Up to the one
//!   before; both wrap.
//! - On the project and on where agents run, Left goes to the choice
//!   before and Right or Space to the next one, wrapping; other typing is
//!   ignored there.
//! - Enter goes to the next field, and on the last one (Agents run)
//!   submits. Ctrl+Enter submits from any field.
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

/// Longest slug, in characters.
const MAX_SLUG: usize = 40;

/// What the form says under a slug it refuses.
pub(crate) const SLUG_HINT: &str = "lowercase letters, digits and -, up to 40";

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
    /// Projects it offers besides "None", in order
    choices: Vec<ChannelProjectView>,
    /// Id of the chosen project; `None` for "None"
    project: Option<String>,
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
        form.project = project;
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
            project: channel.project.as_ref().map(|project| project.id.clone()),
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

    /// The chosen project; `None` for "None".
    pub(crate) fn project(&self) -> Option<&ChannelProjectView> {
        let id = self.project.as_deref()?;
        self.choices.iter().find(|choice| choice.id == id)
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
        self.drop_vanished_project();
    }

    fn drop_vanished_project(&mut self) {
        if self.project().is_none() {
            self.project = None;
        }
    }

    /// Choose the project after the chosen one, or the one before; "None"
    /// comes before the first and after the last.
    fn cycle_project(&mut self, forward: bool) {
        // Position 0 is "None", then the choices from 1.
        let positions = self.choices.len() + 1;
        let current = self
            .project
            .as_deref()
            .and_then(|id| self.choices.iter().position(|choice| choice.id == id))
            .map_or(0, |index| index + 1);
        let next = if forward {
            (current + 1) % positions
        } else {
            (current + positions - 1) % positions
        };
        self.project = next
            .checked_sub(1)
            .and_then(|index| self.choices.get(index))
            .map(|choice| choice.id.clone());
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
    /// new channel while it is refused, or the project while agents are
    /// to run in the cloud without one.
    fn submission(&self) -> Result<UiEvent, ChannelField> {
        if !self.is_edit() && !self.slug_is_valid() {
            return Err(ChannelField::Slug);
        }
        if self.cloud_without_project() {
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
