//! Opening, driving and closing the channel form from the app.

use super::{ChannelField, ChannelForm};
use crate::app::{App, Dialog};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

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
        self.open_channel_form(ChannelForm::create(&self.project_list.projects, project));
    }

    /// Open the form on listed channel `channel_id`, filled with its
    /// title, description, project and placement, to change them. Does
    /// nothing for a channel that is not listed.
    pub(crate) fn start_edit_channel(&mut self, channel_id: &str) {
        let Some(channel) = self.channels().iter().find(|c| c.id == channel_id) else {
            return;
        };
        let form = ChannelForm::edit(channel, &self.project_list.projects);
        self.open_channel_form(form);
    }

    fn open_channel_form(&mut self, form: ChannelForm) {
        self.open_dialog(Dialog::ChannelForm);
        self.channel_form = Some(form);
    }

    /// The listed project of the agent on screen before CHANNELS took
    /// over (else of the agent on screen), if it has one.
    fn project_before_channels(&self) -> Option<String> {
        let agent_id = self.agent_before_channels();
        let project = self
            .agents
            .iter()
            .chain(&self.archived.items)
            .find(|agent| agent.id == agent_id)?
            .project_id
            .clone()?;
        self.project_list
            .projects
            .iter()
            .any(|listed| listed.id == project)
            .then_some(project)
    }

    /// The projects list changed: let the open form offer the new one.
    pub(in crate::app) fn projects_changed_under_channel_form(&mut self) {
        if let Some(form) = self.channel_form.as_mut() {
            form.set_projects(&self.project_list.projects);
        }
    }

    /// Close the form without sending anything.
    fn close_channel_form(&mut self) {
        self.close_dialog();
    }

    /// Paste `text` into the form's focused field.
    pub(in crate::app) fn paste_into_channel_form(&mut self, text: &str) {
        if let Some(form) = self.channel_form.as_mut() {
            form.insert(text);
        }
    }

    /// Keys while the form is open, as the channel form's module docs
    /// list them.
    pub(in crate::app) fn handle_channel_form_key(&mut self, key: KeyEvent) {
        let Some(form) = self.channel_form.as_mut() else {
            self.close_dialog();
            return;
        };
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => self.close_channel_form(),
            KeyCode::Enter if ctrl || form.focus == ChannelField::Placement => {
                self.submit_channel_form();
            }
            KeyCode::Enter | KeyCode::Tab | KeyCode::Down => form.move_focus(true),
            KeyCode::BackTab | KeyCode::Up => form.move_focus(false),
            _ => {
                form.edit_key(key);
            }
        }
    }

    /// Send the form and close it, or, while it refuses to, put the
    /// cursor on the field to fix and keep the form open.
    fn submit_channel_form(&mut self) {
        let Some(form) = self.channel_form.as_mut() else {
            return;
        };
        match form.submission() {
            Ok(event) => {
                self.close_channel_form();
                self.try_send_event(event);
            }
            Err(field) => form.focus = field,
        }
    }
}
