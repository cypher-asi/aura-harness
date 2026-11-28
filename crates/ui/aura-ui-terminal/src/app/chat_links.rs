//! Links in the chat's text: where they were drawn, which one the
//! pointer is on, and opening the one clicked.
//!
//! Only web pages and mail addresses open. A link's target is whatever
//! the agent or another user wrote, and the system's opener would as
//! readily open a file, run a program or hand an odd scheme to whatever
//! handles it, so any other link is drawn as one but is not clickable.

use super::{contains, App, HoverTarget, LinkHit, NotificationType};
use ratatui::buffer::{Buffer, Cell};
use ratatui::layout::Rect;

/// Schemes a link may open with, matched without regard to case.
const OPENABLE_SCHEMES: [&str; 3] = ["http://", "https://", "mailto:"];

/// Whether a click may hand `url` to the system's opener: a web page or
/// a mail address, with something after the scheme, and nothing a URL
/// has to escape. On Windows the opener goes through `cmd /C start`,
/// which reads `&` and `%` itself, so a URL with either does not open.
pub(crate) fn is_openable_url(url: &str) -> bool {
    is_openable(url, cfg!(windows))
}

/// [`is_openable_url`], `through_cmd` when the opener is `cmd.exe`.
fn is_openable(url: &str, through_cmd: bool) -> bool {
    let Some(rest) = OPENABLE_SCHEMES
        .iter()
        .find_map(|scheme| strip_prefix_ignore_case(url, scheme))
    else {
        return false;
    };
    !rest.is_empty()
        && url
            .chars()
            .all(|c| is_url_char(c) && !(through_cmd && matches!(c, '&' | '%')))
}

/// Whether `c` may stand unescaped in a URL written in text: not
/// whitespace or a control character, nor one of the characters a URL
/// has to escape, which also end a URL written in prose.
pub(crate) fn is_url_char(c: char) -> bool {
    !c.is_whitespace()
        && !c.is_control()
        && !matches!(c, '<' | '>' | '"' | '`' | '{' | '}' | '|' | '\\' | '^')
}

/// `text` after `prefix`, when it starts with it in any case.
pub(crate) fn strip_prefix_ignore_case<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let head = text.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| &text[prefix.len()..])
}

/// A row of a link as drawn in the last frame, with what its cells held
/// then, so a link drawn over is forgotten.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ShownLink {
    hit: LinkHit,
    symbols: String,
}

impl App {
    /// Record where the chat drew its links this frame: `hits`, their
    /// columns as drawn into `buffer`. Rows off screen are left out.
    pub(crate) fn set_chat_links(&mut self, hits: Vec<LinkHit>, buffer: &Buffer) {
        let Some(area) = self.areas.chat_text else {
            self.chat.link_hits.clear();
            return;
        };
        let first = self.chat.first_line;
        self.chat.link_hits = hits
            .into_iter()
            .filter_map(|hit| {
                let symbols = drawn(buffer, (area, first), &hit)?;
                Some(ShownLink { hit, symbols })
            })
            .collect();
    }

    /// Forget the chat's links, as nothing drawn this frame has any.
    pub(crate) fn clear_chat_links(&mut self) {
        self.chat.link_hits.clear();
    }

    /// Forget the links a menu or overlay was drawn over, so a click on
    /// what covers one does not open it.
    pub(crate) fn drop_covered_links(&mut self, buffer: &Buffer) {
        let Some(area) = self.areas.chat_text else {
            self.chat.link_hits.clear();
            return;
        };
        let first = self.chat.first_line;
        self.chat
            .link_hits
            .retain(|shown| drawn(buffer, (area, first), &shown.hit).as_ref() == Some(&shown.symbols));
    }

    /// The link at a screen cell, using the last frame. A view over the
    /// chat hides the links.
    fn link_at(&self, column: u16, row: u16) -> Option<&LinkHit> {
        if self.detail.view.is_some() {
            return None;
        }
        let area = self
            .areas
            .chat_text
            .filter(|area| contains(Some(*area), column, row))?;
        let line = self.chat.first_line + usize::from(row - area.y);
        let x = column - area.x;
        self.chat
            .link_hits
            .iter()
            .map(|shown| &shown.hit)
            .find(|hit| hit.line == line && (hit.x..hit.x + hit.width).contains(&x))
    }

    /// The link at a screen cell as a hover target.
    pub(super) fn hovered_link(&self, column: u16, row: u16) -> Option<HoverTarget> {
        self.link_at(column, row)
            .map(|hit| HoverTarget::Link(hit.link))
    }

    /// Whether the pointer is on a link in the chat, which a front end
    /// with a pointer of its own shows as a hand.
    #[must_use]
    pub fn hovers_link(&self) -> bool {
        matches!(self.hover, Some(HoverTarget::Link(_)))
    }

    /// Open the link at a screen cell. Returns whether there was one.
    pub(super) fn open_link_at(&mut self, column: u16, row: u16) -> bool {
        let Some(url) = self.link_at(column, row).map(|hit| hit.url.clone()) else {
            return false;
        };
        self.open_link(&url);
        true
    }

    /// Hand `url` to the system's opener, saying so when it fails.
    fn open_link(&mut self, url: &str) {
        // Links that may not open get no hit when drawn; this keeps the
        // rule where the URL leaves the app, whatever drew the hit.
        if !is_openable_url(url) {
            self.notifications.current = Some((
                format!("Only web and mail links open, not {url}"),
                NotificationType::Warning,
            ));
            return;
        }
        #[cfg(test)]
        self.chat.opened_links.push(url.to_owned());
        #[cfg(not(test))]
        if let Err(error) = super::image_preview::open_with_system(url) {
            self.notifications.current = Some((
                format!("Could not open {url}: {error}"),
                NotificationType::Error,
            ));
        }
    }
}

/// What the cells of `hit` hold in `buffer`, the chat's text drawn in
/// `area` from chat line `first`; `None` when the row is not on screen.
fn drawn(buffer: &Buffer, (area, first): (Rect, usize), hit: &LinkHit) -> Option<String> {
    let offset = hit
        .line
        .checked_sub(first)
        .and_then(|offset| u16::try_from(offset).ok())
        .filter(|offset| *offset < area.height)?;
    let (x, y) = (area.x.saturating_add(hit.x), area.y + offset);
    Some(
        (x..x.saturating_add(hit.width))
            .filter_map(|x| buffer.cell((x, y)).map(Cell::symbol))
            .collect(),
    )
}

#[cfg(test)]
mod tests;
