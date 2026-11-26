//! Images pasted into the input, and pasting in general.
//!
//! Each image puts an `[image N]` chip in the input. Deleting the chip
//! drops the image, so what is sent is exactly what the input shows.

use super::image_preview::{ImagePreview, ImageView};
use super::{App, AppState, NotificationType};
use crate::clipboard::Clipboard;
use crate::events::ImageAttachment;
use crate::graphics::PlacementSlot;
use base64::Engine;
use std::path::PathBuf;
use std::sync::Arc;

/// Largest image accepted, in bytes: 3.75 MiB is 5 MiB once
/// base64-encoded, the most Anthropic takes per image.
pub const MAX_IMAGE_BYTES: usize = 3 * 1024 * 1024 + 768 * 1024;

/// Most images in one message, which keeps a request well under the
/// provider's total size limit.
pub const MAX_IMAGES: usize = 5;

/// Messages with images waiting for a busy agent whose previews are kept.
const MAX_QUEUED_MESSAGES: usize = 16;

/// File extensions a pasted path must have to be read as an image.
const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp"];

/// An image waiting in the input under its `[image N]` chip.
#[derive(Debug, Clone)]
pub(crate) struct PendingImage {
    label: usize,
    image: ImageAttachment,
    preview: Arc<ImagePreview>,
}

/// The chip shown in the input and the chat for image `label`.
#[must_use]
pub fn image_chip(label: usize) -> String {
    format!("[image {label}]")
}

/// Media type of `bytes`, from the file signature.
fn media_type_of(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

/// `2.4 MiB`, for size messages.
fn mib(bytes: usize) -> String {
    #[expect(
        clippy::cast_precision_loss,
        reason = "two decimal places of a file size"
    )]
    let mib = bytes as f64 / (1024.0 * 1024.0);
    format!("{mib:.2} MiB")
}

/// The image file a paste names, if it is one: a single line holding a
/// path (quoted, `file://`, or with `\ `-escaped blanks, as terminals
/// paste dropped files) to an existing file with an image extension.
fn pasted_image_path(text: &str) -> Option<PathBuf> {
    let text = text.trim();
    if text.is_empty() || text.contains('\n') {
        return None;
    }
    let unquoted = ['\'', '"']
        .iter()
        .find_map(|q| text.strip_prefix(*q)?.strip_suffix(*q))
        .unwrap_or(text);
    let path = match unquoted.strip_prefix("file://") {
        Some(url) => percent_decode(url),
        None => unquoted.replace("\\ ", " "),
    };
    let path = match path.strip_prefix("~/") {
        Some(rest) => dirs::home_dir()?.join(rest),
        None => PathBuf::from(path),
    };
    let extension = path.extension()?.to_str()?.to_lowercase();
    (IMAGE_EXTENSIONS.contains(&extension.as_str()) && path.is_file()).then_some(path)
}

/// `%20`-style escapes of a `file://` URL decoded; bad escapes are kept.
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let escaped = (bytes[i] == b'%')
            .then(|| text.get(i + 1..i + 3))
            .flatten()
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        if let Some(byte) = escaped {
            out.push(byte);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

impl App {
    /// Attach image `bytes` to the message being typed and put its chip
    /// at the cursor.
    ///
    /// # Errors
    /// Returns why the image was refused: not a PNG, JPEG, GIF or WebP,
    /// too large, or one too many.
    pub fn attach_image(&mut self, bytes: &[u8]) -> Result<(), String> {
        if self.editing_queued() {
            return Err("Images can't be added to a queued message".to_string());
        }
        let media_type =
            media_type_of(bytes).ok_or("Only PNG, JPEG, GIF and WebP images can be attached")?;
        if bytes.len() > MAX_IMAGE_BYTES {
            return Err(format!(
                "Image is {}; the limit is {}",
                mib(bytes.len()),
                mib(MAX_IMAGE_BYTES)
            ));
        }
        self.prune_images();
        if self.images.len() >= MAX_IMAGES {
            return Err(format!("A message holds at most {MAX_IMAGES} images"));
        }
        let label = self.images.iter().map(|p| p.label).max().unwrap_or(0) + 1;
        self.images.push(PendingImage {
            label,
            image: ImageAttachment {
                media_type: media_type.to_string(),
                data: base64::engine::general_purpose::STANDARD.encode(bytes),
                bytes: bytes.len(),
            },
            preview: ImagePreview::new(bytes.to_vec(), media_type),
        });
        self.insert_chip(&image_chip(label));
        Ok(())
    }

    /// Images whose chip is still in the input.
    #[must_use]
    pub fn attached_images(&self) -> usize {
        self.images
            .iter()
            .filter(|p| self.input.contains(&image_chip(p.label)))
            .count()
    }

    /// Forget images whose chip was deleted from the input.
    fn prune_images(&mut self) {
        let input = &self.input;
        self.images.retain(|p| input.contains(&image_chip(p.label)));
    }

    /// The images to send with `text`, and their previews for the chat,
    /// emptying the list either way.
    pub(super) fn take_images(
        &mut self,
        text: &str,
    ) -> (Vec<ImageAttachment>, Vec<Arc<ImagePreview>>) {
        std::mem::take(&mut self.images)
            .into_iter()
            .filter(|p| text.contains(&image_chip(p.label)))
            .map(|p| (p.image, p.preview))
            .unzip()
    }

    /// Images waiting in the input whose chip is still there, with their
    /// labels, for the thumbnails above it.
    #[must_use]
    pub(crate) fn pending_previews(&self) -> Vec<(usize, Arc<ImagePreview>)> {
        self.images
            .iter()
            .filter(|p| self.input.contains(&image_chip(p.label)))
            .map(|p| (p.label, Arc::clone(&p.preview)))
            .collect()
    }

    /// Keep the previews of a message sent to a busy agent until the chat
    /// shows it.
    pub(super) fn queue_image_previews(&mut self, text: &str, previews: Vec<Arc<ImagePreview>>) {
        if previews.is_empty() {
            return;
        }
        self.queued_images.push_back((text.to_string(), previews));
        while self.queued_images.len() > MAX_QUEUED_MESSAGES {
            self.queued_images.pop_front();
        }
    }

    /// The previews queued for a message reading `text`, once.
    pub(super) fn take_queued_previews(&mut self, text: &str) -> Vec<Arc<ImagePreview>> {
        self.queued_images
            .iter()
            .position(|(queued, _)| queued == text)
            .and_then(|at| self.queued_images.remove(at))
            .map(|(_, previews)| previews)
            .unwrap_or_default()
    }

    /// Record where the chat drew image thumbnails this frame.
    pub(crate) fn set_chat_images(&mut self, hits: Vec<super::ImageHit>) {
        self.image_hits = hits;
    }

    /// Record where the thumbnails above the input were drawn this frame.
    pub(crate) fn set_pending_image_areas(&mut self, areas: Vec<(ratatui::layout::Rect, usize)>) {
        self.pending_image_areas = areas;
    }

    /// Record where the full-size image was drawn in the detail view this frame.
    pub(crate) fn set_detail_image_rect(&mut self, rect: Option<ratatui::layout::Rect>) {
        self.detail_image_rect = rect;
    }

    /// All image placements visible this frame, for the Kitty graphics
    /// protocol layer.
    ///
    /// Returns `(slot, rect, preview)` for:
    /// - chat thumbnails whose tile row is currently scrolled into view,
    /// - pending-input thumbnails,
    /// - the full-size detail view image.
    ///
    /// Images whose `graphics_png` is `None` (decode failed) are excluded;
    /// the caller should skip them or they would produce a no-op.
    ///
    /// A thumbnail's rect covers its picture only: not the caption under
    /// it, nor the extra width a long caption gives its tile, since the
    /// terminal stretches the image over the whole rect.
    pub(crate) fn visible_image_placements(
        &self,
    ) -> Vec<(PlacementSlot, ratatui::layout::Rect, Arc<ImagePreview>)> {
        let mut out = Vec::new();

        // ── Chat thumbnails ────────────────────────────────────────────────
        if let Some(area) = self.areas.chat_text {
            let first = self.chat_first_line;
            let last = first + usize::from(area.height);
            for hit in &self.image_hits {
                let Some(preview) = self
                    .messages
                    .iter()
                    .find(|m| m.version().0 == hit.message)
                    .and_then(|m| m.images().get(hit.index))
                    .filter(|p| p.graphics_png().is_some())
                    .map(Arc::clone)
                else {
                    continue;
                };
                // Clip the picture's rows to the visible scroll window.
                let top = hit.line.max(first);
                let bottom = (hit.line + preview.thumbnail().len()).min(last);
                if bottom <= top {
                    continue;
                }
                let rect = ratatui::layout::Rect {
                    x: area.x.saturating_add(hit.x),
                    y: area
                        .y
                        .saturating_add(u16::try_from(top - first).unwrap_or(u16::MAX)),
                    width: preview.thumbnail_width().min(hit.width),
                    height: u16::try_from(bottom - top).unwrap_or(u16::MAX),
                };
                out.push((PlacementSlot::ChatThumbnail, rect, preview));
            }
        }

        // ── Pending-input thumbnails ───────────────────────────────────────
        for &(tile, label) in &self.pending_image_areas {
            let Some(preview) = self
                .images
                .iter()
                .find(|p| p.label == label)
                .map(|p| Arc::clone(&p.preview))
                .filter(|p| p.graphics_png().is_some())
            else {
                continue;
            };
            let rect = ratatui::layout::Rect {
                width: tile.width.min(preview.thumbnail_width()),
                height: tile
                    .height
                    .min(u16::try_from(preview.thumbnail().len()).unwrap_or(u16::MAX)),
                ..tile
            };
            if !rect.is_empty() {
                out.push((PlacementSlot::PendingThumbnail, rect, preview));
            }
        }

        // ── Detail-view image ──────────────────────────────────────────────
        if let (Some(rect), Some(super::DetailView::Image(view))) =
            (self.detail_image_rect, &self.detail)
        {
            if !rect.is_empty() && view.preview.graphics_png().is_some() {
                out.push((PlacementSlot::Detail, rect, Arc::clone(&view.preview)));
            }
        }

        out
    }

    /// Open `preview` over the chat panel.
    fn open_image(&mut self, preview: Arc<ImagePreview>, label: usize) {
        let view = ImageView::new(preview, format!("IMAGE {label}"));
        self.open_detail(super::DetailView::Image(Box::new(view)));
    }

    /// The chat thumbnail at a screen cell, using the last frame.
    fn chat_image_at(&self, column: u16, row: u16) -> Option<super::ImageHit> {
        let area = self.areas.chat_text?;
        if !super::contains(Some(area), column, row) {
            return None;
        }
        let line = self.chat_first_line + usize::from(row - area.y);
        let x = column - area.x;
        self.image_hits
            .iter()
            .find(|hit| {
                (hit.line..hit.line + hit.rows).contains(&line)
                    && (hit.x..hit.x + hit.width).contains(&x)
            })
            .copied()
    }

    /// Open the image whose thumbnail is at a screen cell, in the chat or
    /// above the input. Returns whether one was opened.
    pub(super) fn open_image_at(&mut self, column: u16, row: u16) -> bool {
        if let Some(&(_, label)) = self
            .pending_image_areas
            .iter()
            .find(|(area, _)| super::contains(Some(*area), column, row))
        {
            let preview = self
                .images
                .iter()
                .find(|p| p.label == label)
                .map(|p| Arc::clone(&p.preview));
            if let Some(preview) = preview {
                self.open_image(preview, label);
                return true;
            }
        }
        if self.detail.is_some() {
            return false;
        }
        let Some(hit) = self.chat_image_at(column, row) else {
            return false;
        };
        let preview = self
            .messages
            .iter()
            .find(|m| m.version().0 == hit.message)
            .and_then(|m| m.images().get(hit.index))
            .map(Arc::clone);
        match preview {
            Some(preview) => {
                self.open_image(preview, hit.index + 1);
                true
            }
            None => false,
        }
    }

    /// The thumbnail at a screen cell, for the hover highlight.
    pub(super) fn image_at(&self, column: u16, row: u16) -> Option<super::HoverTarget> {
        if let Some(&(_, label)) = self
            .pending_image_areas
            .iter()
            .find(|(area, _)| super::contains(Some(*area), column, row))
        {
            return Some(super::HoverTarget::PendingImage(label));
        }
        if self.detail.is_some() {
            return None;
        }
        self.chat_image_at(column, row)
            .map(|hit| super::HoverTarget::ChatImage(hit.message, hit.index))
    }

    /// Open the image on screen in the system's image viewer.
    pub(super) fn open_image_externally(&mut self) {
        let Some(super::DetailView::Image(view)) = &self.detail else {
            return;
        };
        self.notification = Some(match view.open_externally() {
            Ok(path) => (
                format!("Opened {}", path.display()),
                NotificationType::Success,
            ),
            Err(e) => (
                format!("Cannot open the image: {e}"),
                NotificationType::Error,
            ),
        });
    }

    /// Put `chip` at the cursor with a blank on either side.
    fn insert_chip(&mut self, chip: &str) {
        let pos = self.cursor_pos.min(self.input.len());
        let before = &self.input[..pos];
        let after = &self.input[pos..];
        let lead = if before.is_empty() || before.ends_with(char::is_whitespace) {
            ""
        } else {
            " "
        };
        let trail = if after.starts_with(char::is_whitespace) {
            ""
        } else {
            " "
        };
        let text = format!("{lead}{chip}{trail}");
        self.input.insert_str(pos, &text);
        self.cursor_pos = pos + text.len();
    }

    /// Ask for the clipboard to be pasted before the next frame (Ctrl+V).
    /// Reading it runs a clipboard tool, which the terminal does outside
    /// key handling.
    pub(super) fn request_paste(&mut self) {
        self.paste_requested = true;
    }

    /// Whether Ctrl+V asked for the clipboard, once. The terminal reads it
    /// and hands it to [`Self::paste_clipboard`].
    pub fn take_paste_request(&mut self) -> bool {
        std::mem::take(&mut self.paste_requested)
    }

    /// Read the system clipboard and paste it, for frontends other than
    /// the terminal after [`Self::take_paste_request`].
    pub fn paste_from_clipboard(&mut self) {
        self.paste_clipboard(crate::clipboard::read());
    }

    /// Paste what the clipboard held: an image is attached, text is typed.
    pub(crate) fn paste_clipboard(&mut self, clipboard: std::io::Result<Clipboard>) {
        self.redraw = true;
        match clipboard {
            Ok(Clipboard::Image(bytes)) => {
                if let Err(problem) = self.attach_image(&bytes) {
                    self.notification = Some((problem, NotificationType::Warning));
                }
            }
            Ok(Clipboard::Text(text)) => self.handle_paste(&text),
            Ok(Clipboard::Empty) => {
                self.notification = Some((
                    "The clipboard is empty".to_string(),
                    NotificationType::Warning,
                ));
            }
            Err(e) => {
                self.notification = Some((
                    format!("Cannot read the clipboard: {e}"),
                    NotificationType::Error,
                ));
            }
        }
        self.sync_mentions();
    }

    /// Handle text pasted into the terminal. A path to an image file
    /// attaches the image; other text is inserted at the cursor, newlines
    /// included, instead of sending the message at the first one.
    pub fn handle_paste(&mut self, text: &str) {
        if self.detail.is_some() {
            return;
        }
        self.notification = None;
        self.clear_selection();
        match self.state {
            AppState::Idle | AppState::Processing => {
                if let Some(path) = pasted_image_path(text) {
                    self.attach_image_file(&path);
                } else {
                    self.insert_text(&text.replace("\r\n", "\n").replace('\r', "\n"));
                }
            }
            AppState::LoginEmail | AppState::LoginPassword | AppState::LinkCode => {
                self.insert_text(text.trim().replace(['\r', '\n'], "").as_str());
            }
            AppState::ChannelForm => self.paste_into_channel_form(text),
            AppState::AwaitingApproval
            | AppState::ShowingHelp
            | AppState::ShowingProjectInfo
            | AppState::SelectingModel
            | AppState::SelectingFolder => {}
        }
        self.sync_mentions();
    }

    fn attach_image_file(&mut self, path: &std::path::Path) {
        let too_big = std::fs::metadata(path)
            .ok()
            .and_then(|meta| usize::try_from(meta.len()).ok())
            .filter(|&len| len > MAX_IMAGE_BYTES);
        let result = match too_big {
            Some(len) => Err(format!(
                "Image is {}; the limit is {}",
                mib(len),
                mib(MAX_IMAGE_BYTES)
            )),
            None => std::fs::read(path)
                .map_err(|e| format!("Cannot read {}: {e}", path.display()))
                .and_then(|bytes| self.attach_image(&bytes)),
        };
        if let Err(problem) = result {
            self.notification = Some((problem, NotificationType::Warning));
        }
    }

    fn insert_text(&mut self, text: &str) {
        let pos = self.cursor_pos.min(self.input.len());
        self.input.insert_str(pos, text);
        self.cursor_pos = pos + text.len();
        self.reset_slash_menu();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_type_comes_from_the_signature() {
        assert_eq!(media_type_of(b"\x89PNG\r\n\x1a\nrest"), Some("image/png"));
        assert_eq!(media_type_of(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("image/jpeg"));
        assert_eq!(media_type_of(b"GIF89a.."), Some("image/gif"));
        assert_eq!(media_type_of(b"RIFF\0\0\0\0WEBPVP8 "), Some("image/webp"));
        assert_eq!(media_type_of(b"plain text"), None);
    }

    #[test]
    fn pasted_paths_are_unquoted_and_unescaped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("my shot.PNG");
        std::fs::write(&path, b"x").unwrap();
        let shown = path.display().to_string();

        assert_eq!(
            pasted_image_path(&format!("'{shown}'\n")),
            Some(path.clone())
        );
        assert_eq!(
            pasted_image_path(&shown.replace(' ', "\\ ")),
            Some(path.clone())
        );
        assert_eq!(
            pasted_image_path(&format!("file://{}", shown.replace(' ', "%20"))),
            Some(path.clone())
        );
        assert_eq!(pasted_image_path(&format!("{shown}\nmore")), None);
        let text = dir.path().join("notes.txt");
        std::fs::write(&text, b"x").unwrap();
        assert_eq!(pasted_image_path(&text.display().to_string()), None);
    }

    #[test]
    fn pending_placements_cover_the_picture_not_its_caption() {
        let img = image::RgbaImage::from_pixel(4, 4, image::Rgba([200, 10, 10, 255]));
        let mut png = std::io::Cursor::new(Vec::new());
        img.write_to(&mut png, image::ImageFormat::Png).unwrap();
        let mut app = App::new();
        app.attach_image(png.get_ref()).unwrap();
        let preview = Arc::clone(&app.images[0].preview);
        let cols = preview.thumbnail_width();
        let rows = u16::try_from(preview.thumbnail().len()).unwrap();
        // The tile is wider than the picture for its caption, and a row
        // taller for the caption itself.
        let tile = ratatui::layout::Rect {
            x: 3,
            y: 5,
            width: cols + 6,
            height: rows + 1,
        };
        app.set_pending_image_areas(vec![(tile, 1)]);
        let placements = app.visible_image_placements();
        assert_eq!(placements.len(), 1);
        assert_eq!(
            placements[0].1,
            ratatui::layout::Rect {
                width: cols,
                height: rows,
                ..tile
            }
        );
    }
}
