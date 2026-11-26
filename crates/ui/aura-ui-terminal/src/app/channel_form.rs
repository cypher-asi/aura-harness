//! The channel form: "+ New channel" opens it to make a channel, and the
//! channel menu's "Edit channel…" opens it on a channel to change it.
//! Its fields are a slug, a title and a description, each a line edited
//! as the input line is (see [`edit_line`]), then the project the
//! channel's agents work in, a chooser of "None" and every listed
//! project, in list order.
//!
//! Keys, as the form's hint lines say:
//!
//! - Tab or Down goes to the next field, Shift+Tab or Up to the one
//!   before; both wrap.
//! - On the project, Left goes to the choice before and Right or Space to
//!   the next one, wrapping; other typing is ignored there.
//! - Enter goes to the next field, and on the last one (Project)
//!   submits. Ctrl+Enter submits from any field.
//! - Esc closes the form and sends nothing.
//!
//! The slug must match `[a-z0-9-]{1,40}`. Uppercase letters typed into it
//! are lowercased as they are typed, so `Release` becomes `release`; any
//! other character is kept, so the inline hint can say why the slug is
//! refused. A submit with a bad slug sends nothing and puts the cursor
//! back in the slug. An empty title becomes the slug.
//!
//! A new channel's project starts as the one of the agent that was on
//! screen before CHANNELS took over, when that project is listed. The
//! choices follow the projects list while the form is open; a chosen
//! project that drops out of it falls back to "None".
//!
//! Submitting a new channel sends [`UiEvent::CreateChannel`] and closes
//! the form; the session opens the channel once it has made it.
//!
//! Editing shows the slug but does not let it change: Tab skips it and
//! the cursor starts on the title. The channel's own project stays a
//! choice even when the projects list leaves it out, so saving without
//! touching it keeps it. An empty title keeps the one the channel has.
//! Submitting sends [`UiEvent::UpdateChannel`] with every field, so
//! "None" clears the project.

use super::line_edit::edit_line;
use super::{App, AppState};
use crate::events::{ChannelProjectView, ChannelSummary, ProjectSummary, UiEvent};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Longest slug, in characters.
const MAX_SLUG: usize = 40;

/// What the form says under a slug it refuses.
pub(crate) const SLUG_HINT: &str = "lowercase letters, digits and -, up to 40";

/// A field of the form, in the order Tab walks them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum ChannelField {
    /// Short unique name the channel is addressed by
    #[default]
    Slug,
    /// What the channel is called
    Title,
    /// What the channel is for
    Description,
    /// The project the channel's agents work in
    Project,
}

impl ChannelField {
    /// Every field, top to bottom.
    pub(crate) const ALL: [Self; 4] = [Self::Slug, Self::Title, Self::Description, Self::Project];

    /// What the field's label reads.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Slug => "Slug",
            Self::Title => "Title",
            Self::Description => "Description",
            Self::Project => "Project",
        }
    }

    /// Where the field's text is kept, for the fields that are text.
    const fn text_index(self) -> Option<usize> {
        match self {
            Self::Slug => Some(0),
            Self::Title => Some(1),
            Self::Description => Some(2),
            Self::Project => None,
        }
    }

    const fn next(self) -> Self {
        match self {
            Self::Slug => Self::Title,
            Self::Title => Self::Description,
            Self::Description => Self::Project,
            Self::Project => Self::Slug,
        }
    }

    const fn previous(self) -> Self {
        match self {
            Self::Slug => Self::Project,
            Self::Title => Self::Slug,
            Self::Description => Self::Title,
            Self::Project => Self::Description,
        }
    }
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
    /// Projects it offers besides "None", in order
    choices: Vec<ChannelProjectView>,
    /// Id of the chosen project; `None` for "None"
    project: Option<String>,
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

    /// Text typed into `field`; empty for the project, which is no text.
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
    /// Left, Right or Space to the project. Returns whether it was one.
    fn edit_key(&mut self, key: KeyEvent) -> bool {
        if self.focus == ChannelField::Project {
            match key.code {
                KeyCode::Left => self.cycle_project(false),
                KeyCode::Right | KeyCode::Char(' ') => self.cycle_project(true),
                _ => return false,
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
    /// into the project.
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

    /// The event the form sends, or `None` while the slug of a new
    /// channel is refused.
    fn submission(&self) -> Option<UiEvent> {
        let typed_title = self.text(ChannelField::Title).trim();
        let description = self.text(ChannelField::Description).trim().to_string();
        let project_id = self.project().map(|project| project.id.clone());
        match &self.mode {
            FormMode::Create => {
                if !self.slug_is_valid() {
                    return None;
                }
                let slug = self.text(ChannelField::Slug).to_string();
                let title = match typed_title {
                    "" => slug.clone(),
                    title => title.to_string(),
                };
                Some(UiEvent::CreateChannel {
                    slug,
                    title,
                    description,
                    project_id,
                })
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
                Some(UiEvent::UpdateChannel {
                    channel_id: channel_id.clone(),
                    title,
                    description,
                    project_id,
                })
            }
        }
    }
}

/// Whether `slug` matches `[a-z0-9-]{1,40}`.
pub(crate) fn is_valid_slug(slug: &str) -> bool {
    (1..=MAX_SLUG).contains(&slug.len())
        && slug
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

impl App {
    /// The channel form, while it is open.
    #[must_use]
    pub(crate) const fn channel_form(&self) -> Option<&ChannelForm> {
        self.channel_form.as_ref()
    }

    /// Open the new-channel form, empty, on its slug, with the project of
    /// the agent on screen before CHANNELS chosen.
    pub(crate) fn start_new_channel(&mut self) {
        let project = self.project_before_channels();
        self.open_channel_form(ChannelForm::create(&self.projects, project), "New channel");
    }

    /// Open the form on listed channel `channel_id`, filled with its
    /// title, description and project, to change them. Does nothing for
    /// a channel that is not listed.
    pub(crate) fn start_edit_channel(&mut self, channel_id: &str) {
        let Some(channel) = self.channels().iter().find(|c| c.id == channel_id) else {
            return;
        };
        let form = ChannelForm::edit(channel, &self.projects);
        let status = format!("Edit #{}", channel.slug);
        self.open_channel_form(form, &status);
    }

    fn open_channel_form(&mut self, form: ChannelForm, status: &str) {
        self.context_menu = None;
        self.channel_form = Some(form);
        self.state = AppState::ChannelForm;
        self.status = status.to_string();
    }

    /// The listed project of the agent on screen before CHANNELS took
    /// over (else of the agent on screen), if it has one.
    fn project_before_channels(&self) -> Option<String> {
        let agent_id = self.agent_before_channels();
        let project = self
            .agents
            .iter()
            .chain(&self.archived)
            .find(|agent| agent.id == agent_id)?
            .project_id
            .clone()?;
        self.projects
            .iter()
            .any(|listed| listed.id == project)
            .then_some(project)
    }

    /// The projects list changed: let the open form offer the new one.
    pub(super) fn projects_changed_under_channel_form(&mut self) {
        if let Some(form) = self.channel_form.as_mut() {
            form.set_projects(&self.projects);
        }
    }

    /// Close the form without sending anything.
    fn close_channel_form(&mut self) {
        self.channel_form = None;
        self.state = AppState::Idle;
        self.status = "Ready".to_string();
    }

    /// Paste `text` into the form's focused field.
    pub(super) fn paste_into_channel_form(&mut self, text: &str) {
        if let Some(form) = self.channel_form.as_mut() {
            form.insert(text);
        }
    }

    /// Keys while the form is open; see the module docs.
    pub(super) fn handle_channel_form_key(&mut self, key: KeyEvent) {
        let Some(form) = self.channel_form.as_mut() else {
            self.state = AppState::Idle;
            return;
        };
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => self.close_channel_form(),
            KeyCode::Enter if ctrl || form.focus == ChannelField::Project => {
                self.submit_channel_form();
            }
            KeyCode::Enter | KeyCode::Tab | KeyCode::Down => form.move_focus(true),
            KeyCode::BackTab | KeyCode::Up => form.move_focus(false),
            _ => {
                form.edit_key(key);
            }
        }
    }

    /// Send the form and close it, or, while the slug is refused, put
    /// the cursor back in the slug and keep the form open.
    fn submit_channel_form(&mut self) {
        let Some(form) = self.channel_form.as_mut() else {
            return;
        };
        match form.submission() {
            Some(event) => {
                self.close_channel_form();
                self.try_send_event(event);
            }
            None => form.focus = ChannelField::Slug,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(id: &str, name: &str) -> ProjectSummary {
        ProjectSummary {
            id: id.into(),
            name: name.into(),
            folder: format!("/src/{name}"),
            ..ProjectSummary::default()
        }
    }

    fn view(id: &str, name: &str) -> ChannelProjectView {
        ChannelProjectView {
            id: id.into(),
            name: name.into(),
            folder: format!("/src/{name}"),
        }
    }

    fn chosen(form: &ChannelForm) -> Option<&str> {
        form.project().map(|project| project.id.as_str())
    }

    #[test]
    fn slugs_are_lowercase_letters_digits_and_dashes_up_to_40() {
        for (slug, valid) in [
            ("release", true),
            ("v2-launch", true),
            ("-", true),
            (&"a".repeat(MAX_SLUG), true),
            ("", false),
            (&"a".repeat(MAX_SLUG + 1), false),
            ("Release", false),
            ("two words", false),
            ("under_score", false),
            ("café", false),
        ] {
            assert_eq!(is_valid_slug(slug), valid, "{slug:?}");
        }
    }

    #[test]
    fn typed_uppercase_is_lowercased_in_the_slug_only() {
        let mut form = ChannelForm::default();
        form.insert("Zode");
        form.focus = ChannelField::Title;
        form.insert("Zode\nrelease");
        assert_eq!(form.text(ChannelField::Slug), "zode");
        assert_eq!(form.text(ChannelField::Title), "Zoderelease");
    }

    #[test]
    fn an_empty_title_becomes_the_slug() {
        let mut form = ChannelForm::default();
        form.insert("zode");
        form.focus = ChannelField::Title;
        form.insert("   ");
        assert!(matches!(
            form.submission(),
            Some(UiEvent::CreateChannel { slug, title, description, project_id: None })
                if slug == "zode" && title == "zode" && description.is_empty()
        ));
    }

    #[test]
    fn tab_order_wraps_both_ways_through_the_project() {
        for field in ChannelField::ALL {
            assert_eq!(field.next().previous(), field);
        }
        let mut form = ChannelForm::default();
        let mut order = Vec::new();
        for _ in 0..ChannelField::ALL.len() {
            form.move_focus(true);
            order.push(form.focus());
        }
        assert_eq!(
            order,
            [
                ChannelField::Title,
                ChannelField::Description,
                ChannelField::Project,
                ChannelField::Slug
            ]
        );
        form.move_focus(false);
        assert_eq!(form.focus(), ChannelField::Project);
    }

    #[test]
    fn left_and_right_cycle_none_and_every_project_both_ways() {
        let projects = [project("p1", "alpha"), project("p2", "beta")];
        let mut form = ChannelForm::create(&projects, None);
        form.focus = ChannelField::Project;
        let mut forward = Vec::new();
        for _ in 0..3 {
            assert!(form.edit_key(KeyEvent::from(KeyCode::Right)));
            forward.push(chosen(&form).map(str::to_string));
        }
        assert_eq!(
            forward,
            [Some("p1".to_string()), Some("p2".to_string()), None]
        );
        assert!(form.edit_key(KeyEvent::from(KeyCode::Left)));
        assert_eq!(chosen(&form), Some("p2"));
        assert!(form.edit_key(KeyEvent::from(KeyCode::Char(' '))));
        assert_eq!(chosen(&form), None, "Space goes on, wrapping to None");
    }

    #[test]
    fn typing_on_the_project_is_ignored() {
        let mut form = ChannelForm::create(&[project("p1", "alpha")], None);
        form.focus = ChannelField::Project;
        assert!(!form.edit_key(KeyEvent::from(KeyCode::Char('a'))));
        form.insert("pasted");
        assert_eq!(chosen(&form), None);
        assert!(form.fields.iter().all(|field| field.text.is_empty()));
    }

    #[test]
    fn a_chosen_project_that_drops_out_of_the_list_falls_back_to_none() {
        let mut form = ChannelForm::create(
            &[project("p1", "alpha"), project("p2", "beta")],
            Some("p2".into()),
        );
        assert_eq!(chosen(&form), Some("p2"));
        form.set_projects(&[project("p2", "beta"), project("p3", "gamma")]);
        assert_eq!(chosen(&form), Some("p2"), "still listed");
        form.set_projects(&[project("p1", "alpha")]);
        assert_eq!(chosen(&form), None);
    }

    #[test]
    fn a_default_project_that_is_not_listed_is_none() {
        let form = ChannelForm::create(&[project("p1", "alpha")], Some("gone".into()));
        assert_eq!(chosen(&form), None);
    }

    #[test]
    fn editing_keeps_the_channels_own_project_and_skips_the_slug() {
        let channel = ChannelSummary {
            id: "c1".into(),
            slug: "zode".into(),
            title: "Zode".into(),
            project: Some(view("p9", "own")),
            ..ChannelSummary::default()
        };
        let mut form = ChannelForm::edit(&channel, &[project("p1", "alpha")]);
        assert_eq!(form.focus(), ChannelField::Title);
        assert_eq!(chosen(&form), Some("p9"), "kept though not listed");
        form.set_projects(&[]);
        assert_eq!(chosen(&form), Some("p9"));
        form.focus = ChannelField::Project;
        form.move_focus(true);
        assert_eq!(form.focus(), ChannelField::Title, "Tab skips the slug");
        form.move_focus(false);
        assert_eq!(form.focus(), ChannelField::Project);
    }

    #[test]
    fn an_empty_title_in_an_edit_keeps_the_old_one() {
        let channel = ChannelSummary {
            id: "c1".into(),
            slug: "zode".into(),
            title: "Zode release".into(),
            ..ChannelSummary::default()
        };
        let mut form = ChannelForm::edit(&channel, &[]);
        form.fields[1] = FieldText::default();
        assert!(matches!(
            form.submission(),
            Some(UiEvent::UpdateChannel { channel_id, title, project_id: None, .. })
                if channel_id == "c1" && title == "Zode release"
        ));
    }
}
