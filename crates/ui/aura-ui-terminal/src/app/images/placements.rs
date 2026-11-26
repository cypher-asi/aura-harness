//! Where images were drawn this frame, and the placements the Kitty
//! graphics layer puts over them.

use super::super::{App, DetailView, ImageHit};
use super::ImagePreview;
use crate::graphics::PlacementSlot;
use ratatui::layout::Rect;
use std::sync::Arc;

/// One image placement: its slot, where it goes, and what it shows.
type Placement = (PlacementSlot, Rect, Arc<ImagePreview>);

impl App {
    /// Record where the chat drew image thumbnails this frame.
    pub(crate) fn set_chat_images(&mut self, hits: Vec<ImageHit>) {
        self.image_hits = hits;
    }

    /// Record where the thumbnails above the input were drawn this frame.
    pub(crate) fn set_pending_image_areas(&mut self, areas: Vec<(Rect, usize)>) {
        self.pending_image_areas = areas;
    }

    /// Record where the full-size image was drawn in the detail view this frame.
    pub(crate) fn set_detail_image_rect(&mut self, rect: Option<Rect>) {
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
    pub(crate) fn visible_image_placements(&self) -> Vec<Placement> {
        let mut out = self.chat_thumbnail_placements();
        out.extend(self.pending_thumbnail_placements());
        out.extend(self.detail_image_placement());
        out
    }

    /// Chat thumbnails, clipped to the visible scroll window.
    fn chat_thumbnail_placements(&self) -> Vec<Placement> {
        let mut out = Vec::new();
        let Some(area) = self.areas.chat_text else {
            return out;
        };
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
            let rect = Rect {
                x: area.x.saturating_add(hit.x),
                y: area
                    .y
                    .saturating_add(u16::try_from(top - first).unwrap_or(u16::MAX)),
                width: preview.thumbnail_width().min(hit.width),
                height: u16::try_from(bottom - top).unwrap_or(u16::MAX),
            };
            out.push((PlacementSlot::ChatThumbnail, rect, preview));
        }
        out
    }

    /// Thumbnails of the images waiting in the input.
    fn pending_thumbnail_placements(&self) -> Vec<Placement> {
        let mut out = Vec::new();
        for &(tile, label) in &self.pending_image_areas {
            let Some(preview) = self
                .input
                .images
                .iter()
                .find(|p| p.label == label)
                .map(|p| Arc::clone(&p.preview))
                .filter(|p| p.graphics_png().is_some())
            else {
                continue;
            };
            let rect = Rect {
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
        out
    }

    /// The full-size image of the detail view, when one is open.
    fn detail_image_placement(&self) -> Option<Placement> {
        if let (Some(rect), Some(DetailView::Image(view))) = (self.detail_image_rect, &self.detail)
        {
            if !rect.is_empty() && view.preview.graphics_png().is_some() {
                return Some((PlacementSlot::Detail, rect, Arc::clone(&view.preview)));
            }
        }
        None
    }
}
