//! Turning a [`UserMessage`] and its attachments into a model [`Message`].

use crate::protocol::{MessageAttachment, UserMessage};
use aura_model::{ContentBlock, ImageSource, Message, Role};
use base64::Engine;
use tracing::warn;

/// Build the user `Message` for a turn, inlining usable attachments.
pub(super) async fn build_user_message(msg: &UserMessage) -> Message {
    let Some(ref attachments) = msg.attachments else {
        return Message::user(&msg.content);
    };
    // Image AND text attachments both reach the model. The previous
    // image-only filter silently dropped text files (`.md`, `.json`,
    // `.sql`, source code from `@`-mentions, etc.), so the agent
    // saw bare text and reported "no file attached". Text payloads
    // are inlined as a `[File: name]\n\n<contents>` text block so
    // the model has a clear content boundary — same shape the
    // frontend uses for the optimistic chat preview, see
    // `interface/src/hooks/attachment-helpers.ts`.
    let usable_atts: Vec<_> = attachments
        .iter()
        .filter(|a| a.type_ == "image" || a.type_ == "text")
        .collect();
    if usable_atts.is_empty() {
        return Message::user(&msg.content);
    }
    let mut blocks: Vec<ContentBlock> = Vec::new();
    if !msg.content.is_empty() {
        blocks.push(ContentBlock::text(&msg.content));
    }
    for att in &usable_atts {
        if let Some(block) = attachment_block(att).await {
            blocks.push(block);
        }
    }
    if blocks.is_empty() {
        // Every attachment failed to materialize (decode error /
        // S3 fetch failure). Don't ship an empty message —
        // fall back to the bare text so the turn still goes.
        Message::user(&msg.content)
    } else {
        Message::new(Role::User, blocks)
    }
}

/// Materialize one attachment as a content block; `None` skips it.
async fn attachment_block(att: &MessageAttachment) -> Option<ContentBlock> {
    let payload_b64 = attachment_payload(att).await?;
    if att.type_ == "image" {
        Some(ContentBlock::Image {
            source: ImageSource {
                source_type: "base64".into(),
                media_type: att.media_type.clone(),
                data: payload_b64,
            },
        })
    } else {
        let decoded = decode_text_attachment(att, &payload_b64)?;
        let header = att.name.as_deref().unwrap_or("document");
        Some(ContentBlock::text(format!("[File: {header}]\n\n{decoded}")))
    }
}

/// Pull the base64 payload from inline `data` first, falling back to a
/// fetch from `source_url` (S3) when the sender chose the URL-only path.
async fn attachment_payload(att: &MessageAttachment) -> Option<String> {
    if let Some(ref url) = att.source_url {
        if att.data.is_empty() {
            return match fetch_attachment_data(url).await {
                Ok(data) => Some(data),
                Err(e) => {
                    warn!(url = %url, error = %e, "Failed to fetch attachment from URL, skipping");
                    None
                }
            };
        }
    }
    Some(att.data.clone())
}

fn decode_text_attachment(att: &MessageAttachment, payload_b64: &str) -> Option<String> {
    match base64::engine::general_purpose::STANDARD.decode(payload_b64) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(s) => Some(s),
            Err(e) => {
                warn!(name = ?att.name, error = %e, "Skipping non-UTF-8 text attachment");
                None
            }
        },
        Err(e) => {
            warn!(name = ?att.name, error = %e, "Skipping text attachment with invalid base64");
            None
        }
    }
}

/// Fetch attachment content from a URL (e.g. S3) and return as base64.
///
/// Only HTTPS URLs are accepted to prevent fetching from arbitrary sources.
async fn fetch_attachment_data(url: &str) -> Result<String, String> {
    if !url.starts_with("https://") {
        return Err("Only HTTPS URLs are allowed".into());
    }
    let bytes = reqwest::get(url)
        .await
        .map_err(|e| format!("fetch failed: {e}"))?
        .bytes()
        .await
        .map_err(|e| format!("read failed: {e}"))?;
    Ok(base64::engine::general_purpose::STANDARD.encode(&bytes))
}
