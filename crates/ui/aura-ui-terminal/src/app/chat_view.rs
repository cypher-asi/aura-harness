//! The chat as last drawn: its rendered lines, where its rows, images and
//! buttons were, and what the next frame reuses.

use super::{image_preview, smooth, CodeCopyHit, ImageHit, ProseMeasure, ProseRow, ToggleRow};
use crate::renderer::ChatLineCache;
use ratatui::layout::Rect;
use std::collections::{HashMap, VecDeque};

/// The chat as last drawn, and the caches the next frame reuses.
#[derive(Debug)]
pub(crate) struct ChatView {
    /// Rendered lines of each message, reused between frames
    pub(crate) chat_lines: ChatLineCache,
    /// Rendered lines of each shell command, for the terminal pane
    pub(crate) terminal_lines: ChatLineCache,
    /// Total chat lines in the last frame
    pub(crate) line_count: usize,
    /// Chat rows of the last frame that expand or collapse a group of
    /// steps or one step when clicked
    pub(crate) toggle_rows: Vec<ToggleRow>,
    /// First chat line shown in the last frame
    pub(crate) first_line: usize,
    /// Width of the chat text area in the last frame (0 before the first)
    pub(crate) width: u16,
    /// Whether new activity blocks start expanded (`Ctrl+O` toggles)
    pub(crate) activity_expanded: bool,
    /// Groups of steps the user opened (`true`) or closed, by the id of
    /// their first message
    pub(crate) group_open: HashMap<u64, bool>,
    /// Current streaming message (being built)
    pub(crate) streaming_content: String,
    /// The chat's prose rows from the last frame
    pub(crate) prose_rows: Vec<ProseRow>,
    /// How wide prose is in the front end's proportional face, if it has one
    pub(crate) prose_measure: Option<ProseMeasure>,
    /// The chat as last drawn, for a front end that scrolls it by pixels
    pub(crate) smooth: smooth::SmoothScroll,
    /// Image thumbnails in the chat in the last frame
    pub(crate) image_hits: Vec<ImageHit>,
    /// Copy buttons of code blocks in the chat in the last frame
    pub(crate) code_copy_hits: Vec<CodeCopyHit>,
    /// Links in the chat's text on screen in the last frame
    pub(crate) link_hits: Vec<super::chat_links::ShownLink>,
    /// Links a click opened, which tests read instead of starting a browser
    #[cfg(test)]
    pub(crate) opened_links: Vec<String>,
    /// Thumbnails of the images waiting in the input, in the last frame
    pub(crate) pending_image_areas: Vec<(Rect, usize)>,
    /// Absolute cell rect where the detail-view image was drawn in the last
    /// frame; `None` when no image detail view is open or nothing was drawn.
    pub(crate) detail_image_rect: Option<Rect>,
    /// Images of messages sent to a busy agent, by message text, until
    /// the chat shows the message
    pub(crate) queued_images: VecDeque<(String, Vec<std::sync::Arc<image_preview::ImagePreview>>)>,
    /// Policy decisions of calls the turn in flight has not shown yet,
    /// until it shows them or ends
    pub(crate) early_policies: Vec<EarlyPolicy>,
}

/// A tool call's policy decision that came before the call: a cloud turn's
/// copied record can overtake its stream.
#[derive(Debug)]
pub(crate) struct EarlyPolicy {
    /// Tool use id of the call
    pub(crate) id: String,
    pub(crate) tool: String,
    pub(crate) decision: String,
}

impl Default for ChatView {
    /// Nothing drawn yet; the terminal pane's cache renders shell commands.
    fn default() -> Self {
        Self {
            chat_lines: ChatLineCache::default(),
            terminal_lines: ChatLineCache::terminal(),
            line_count: 0,
            toggle_rows: Vec::new(),
            first_line: 0,
            width: 0,
            activity_expanded: false,
            group_open: HashMap::new(),
            streaming_content: String::new(),
            prose_rows: Vec::new(),
            prose_measure: None,
            smooth: smooth::SmoothScroll::default(),
            image_hits: Vec::new(),
            code_copy_hits: Vec::new(),
            link_hits: Vec::new(),
            #[cfg(test)]
            opened_links: Vec::new(),
            pending_image_areas: Vec::new(),
            detail_image_rect: None,
            queued_images: VecDeque::new(),
            early_policies: Vec::new(),
        }
    }
}
