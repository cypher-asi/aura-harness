//! The form "+ New channel" opens: a slug, a title and a description,
//! each a line edited as the input line is (see [`edit_line`]).
//!
//! Keys, as the form's hint line says:
//!
//! - Tab or Down goes to the next field, Shift+Tab or Up to the one
//!   before; both wrap.
//! - Enter goes to the next field, and on the last one (Description)
//!   submits. Ctrl+Enter submits from any field.
//! - Esc closes the form and sends nothing.
//!
//! The slug must match `[a-z0-9-]{1,40}`. Uppercase letters typed into it
//! are lowercased as they are typed, so `Release` becomes `release`; any
//! other character is kept, so the inline hint can say why the slug is
//! refused. A submit with a bad slug sends nothing and puts the cursor
//! back in the slug. An empty title becomes the slug.
//!
//! Submitting sends [`UiEvent::CreateChannel`] and closes the form; the
//! session opens the channel once it has made it.

use super::line_edit::edit_line;
use super::{App, AppState};
use crate::events::UiEvent;
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
}

impl ChannelField {
    /// Every field, top to bottom.
    pub(crate) const ALL: [Self; 3] = [Self::Slug, Self::Title, Self::Description];

    /// What the field's label reads.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Slug => "Slug",
            Self::Title => "Title",
            Self::Description => "Description",
        }
    }

    const fn index(self) -> usize {
        match self {
            Self::Slug => 0,
            Self::Title => 1,
            Self::Description => 2,
        }
    }

    const fn next(self) -> Self {
        match self {
            Self::Slug => Self::Title,
            Self::Title => Self::Description,
            Self::Description => Self::Slug,
        }
    }

    const fn previous(self) -> Self {
        match self {
            Self::Slug => Self::Description,
            Self::Title => Self::Slug,
            Self::Description => Self::Title,
        }
    }
}

/// A field's text and its cursor, a byte offset on a char boundary.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct FieldText {
    text: String,
    cursor: usize,
}

/// The new-channel form's state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ChannelForm {
    /// Slug, title and description, in [`ChannelField::ALL`] order
    fields: [FieldText; 3],
    /// Field the keys type into
    focus: ChannelField,
}

impl ChannelForm {
    /// Text typed into `field`.
    pub(crate) fn text(&self, field: ChannelField) -> &str {
        &self.field(field).text
    }

    /// Cursor of `field`, a byte offset into [`Self::text`].
    pub(crate) fn cursor(&self, field: ChannelField) -> usize {
        self.field(field).cursor
    }

    /// Field the keys type into.
    pub(crate) const fn focus(&self) -> ChannelField {
        self.focus
    }

    /// Whether the slug typed so far would be taken.
    pub(crate) fn slug_is_valid(&self) -> bool {
        is_valid_slug(self.text(ChannelField::Slug))
    }

    fn field(&self, field: ChannelField) -> &FieldText {
        &self.fields[field.index()]
    }

    fn field_mut(&mut self, field: ChannelField) -> &mut FieldText {
        &mut self.fields[field.index()]
    }

    /// Apply an editing key to the focused field. Returns whether it was
    /// one.
    fn edit(&mut self, key: KeyEvent) -> bool {
        let key = match (self.focus, key.code) {
            (ChannelField::Slug, KeyCode::Char(c)) if c.is_ascii_uppercase() => {
                KeyEvent::new(KeyCode::Char(c.to_ascii_lowercase()), key.modifiers)
            }
            _ => key,
        };
        let field = self.field_mut(self.focus);
        edit_line(&mut field.text, &mut field.cursor, key).is_some()
    }

    /// Type `text` into the focused field at its cursor, as a paste does.
    /// Line breaks are dropped: every field is one line.
    fn insert(&mut self, text: &str) {
        let slug = self.focus == ChannelField::Slug;
        let field = self.field_mut(self.focus);
        for c in text.chars().filter(|c| !matches!(c, '\r' | '\n')) {
            let c = if slug { c.to_ascii_lowercase() } else { c };
            let pos = field.cursor.min(field.text.len());
            field.text.insert(pos, c);
            field.cursor = pos + c.len_utf8();
        }
    }

    /// The event the form sends, or `None` while the slug is refused.
    fn submission(&self) -> Option<UiEvent> {
        if !self.slug_is_valid() {
            return None;
        }
        let slug = self.text(ChannelField::Slug).to_string();
        let title = match self.text(ChannelField::Title).trim() {
            "" => slug.clone(),
            title => title.to_string(),
        };
        let description = self.text(ChannelField::Description).trim().to_string();
        Some(UiEvent::CreateChannel {
            slug,
            title,
            description,
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

impl App {
    /// The new-channel form, while it is open.
    #[must_use]
    pub(crate) const fn channel_form(&self) -> Option<&ChannelForm> {
        self.channel_form.as_ref()
    }

    /// Open the new-channel form, empty, on its slug.
    pub(crate) fn start_new_channel(&mut self) {
        self.context_menu = None;
        self.channel_form = Some(ChannelForm::default());
        self.state = AppState::NewChannel;
        self.status = "New channel".to_string();
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
            KeyCode::Enter if ctrl || form.focus == ChannelField::Description => {
                self.submit_channel_form();
            }
            KeyCode::Enter | KeyCode::Tab | KeyCode::Down => form.focus = form.focus.next(),
            KeyCode::BackTab | KeyCode::Up => form.focus = form.focus.previous(),
            _ => {
                form.edit(key);
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
            Some(UiEvent::CreateChannel { slug, title, description })
                if slug == "zode" && title == "zode" && description.is_empty()
        ));
    }

    #[test]
    fn tab_order_wraps_both_ways() {
        for field in ChannelField::ALL {
            assert_eq!(field.next().previous(), field);
        }
        assert_eq!(ChannelField::Description.next(), ChannelField::Slug);
    }
}
