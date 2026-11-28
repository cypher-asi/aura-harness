//! Queued work as the file keeps it, and back: every kind of work, not
//! just inbox messages, so a restart drops none of it unsaid.
//!
//! # Invariant
//!
//! Images are kept up to [`MAX_IMAGE_BYTES`] in all, oldest first. An
//! image past that is left out and counted on its message, whose chat
//! says so when it comes back ([`dropped_images_note`]).

use super::file::{SavedImage, SavedQueued, SavedRequest, SavedWorkKind};
use crate::event_loop::turns::{Prompt, Request, Work};
use aura_core_types::AgentId;
use aura_ui::events::ImageAttachment;

/// Most image data kept across every queued message, base64 bytes.
pub(super) const MAX_IMAGE_BYTES: usize = 4 * 1024 * 1024;

/// `items`, each agent's queue oldest first, as the file keeps them.
pub(super) fn saved_items(items: &[(AgentId, &Work)]) -> Vec<SavedQueued> {
    let mut budget = MAX_IMAGE_BYTES;
    items
        .iter()
        .map(|(agent, work)| saved_item(*agent, work, &mut budget))
        .collect()
}

/// `work` of `agent` as the file keeps it, its images taken from
/// `budget` while it lasts.
fn saved_item(agent: AgentId, work: &Work, budget: &mut usize) -> SavedQueued {
    let agent = agent.to_hex();
    match work {
        Work::Prompt { prompt, .. } => {
            let (images, images_dropped) = kept_images(&prompt.images, budget);
            SavedQueued {
                agent,
                work: SavedWorkKind::Prompt,
                text: prompt.text.clone(),
                request: saved_request(prompt.request),
                mentions: prompt.mentions,
                preamble: prompt.preamble.clone(),
                images,
                images_dropped,
                ..SavedQueued::default()
            }
        }
        Work::Command { command, cols } => SavedQueued {
            agent,
            work: SavedWorkKind::Command,
            text: command.clone(),
            cols: *cols,
            ..SavedQueued::default()
        },
        Work::Compact { focus } => SavedQueued {
            agent,
            work: SavedWorkKind::Compact,
            argument: focus.clone(),
            ..SavedQueued::default()
        },
        Work::Build { path } => SavedQueued {
            agent,
            work: SavedWorkKind::Build,
            argument: path.clone(),
            ..SavedQueued::default()
        },
    }
}

/// The images of `images` that fit in `budget`, and how many didn't.
fn kept_images(images: &[ImageAttachment], budget: &mut usize) -> (Vec<SavedImage>, usize) {
    let mut kept = Vec::new();
    let mut dropped = 0;
    for image in images {
        if image.data.len() <= *budget {
            *budget -= image.data.len();
            kept.push(SavedImage {
                media_type: image.media_type.clone(),
                data: image.data.clone(),
                bytes: image.bytes,
            });
        } else {
            dropped += 1;
        }
    }
    (kept, dropped)
}

/// The work `saved` keeps, as it is queued again.
pub(super) fn work_of(saved: SavedQueued) -> Work {
    match saved.work {
        SavedWorkKind::Prompt => Work::Prompt {
            prompt: Prompt {
                text: saved.text,
                mentions: saved.mentions,
                images: saved.images.into_iter().map(image_of).collect(),
                request: request_of(saved.request),
                preamble: saved.preamble,
            },
            shown: false,
        },
        SavedWorkKind::Command => Work::Command {
            command: saved.text,
            cols: saved.cols,
        },
        SavedWorkKind::Compact => Work::Compact {
            focus: saved.argument,
        },
        SavedWorkKind::Build => Work::Build {
            path: saved.argument,
        },
    }
}

fn image_of(saved: SavedImage) -> ImageAttachment {
    ImageAttachment {
        media_type: saved.media_type,
        data: saved.data,
        bytes: saved.bytes,
    }
}

/// What a chat says of a queued message that came back from a restart
/// without `dropped` images that were too large to keep.
pub(super) fn dropped_images_note(dropped: usize) -> String {
    format!(
        "A queued message came back from the app's restart without {dropped} image(s): \
         they were too large to keep. Paste them again if the agent needs them."
    )
}

const fn saved_request(request: Request) -> SavedRequest {
    match request {
        Request::New => SavedRequest::New,
        Request::Build => SavedRequest::Build,
        Request::Resume => SavedRequest::Resume,
    }
}

const fn request_of(request: SavedRequest) -> Request {
    match request {
        SavedRequest::New => Request::New,
        SavedRequest::Build => Request::Build,
        SavedRequest::Resume => Request::Resume,
    }
}
