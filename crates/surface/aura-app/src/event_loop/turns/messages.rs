//! The user message the model gets for a prompt.

use super::*;

/// Start checkpointing the files the turn answering `text` changes.
pub(in crate::event_loop) fn begin_files(session: &AgentSession, text: &str) {
    if let Some(files) = &session.file_checkpoints {
        files.begin_turn(
            session.agent_id(),
            session.started_ms,
            session.messages.len(),
            text,
        );
    }
}

/// The user message the model gets for `prompt`: its images, then its
/// text with the mentioned files from the agent's workspace appended.
pub(in crate::event_loop) async fn model_message(
    session: &AgentSession,
    prompt: Prompt,
) -> Message {
    let Prompt {
        text,
        mentions,
        images,
        preamble,
        ..
    } = prompt;
    let text = if mentions && !mentioned_paths(&text).is_empty() {
        let expanded = expand_mentions_in(session.kernel.agent_workspace(), text).await;
        if !expanded.attached.is_empty() {
            info!(files = ?expanded.attached, "Attached mentioned files");
        }
        expanded.text
    } else {
        text
    };
    let text = match preamble {
        Some(preamble) => format!("{preamble}\n\n{text}"),
        None => text,
    };
    if images.is_empty() {
        return Message::user(text);
    }
    info!(count = images.len(), "Attached pasted images");
    user_message_with_images(text, images)
}

/// A user message holding `images` and then `text`. Anthropic advises
/// putting images before the text that refers to them.
fn user_message_with_images(text: String, images: Vec<ImageAttachment>) -> Message {
    let mut content: Vec<ContentBlock> = images
        .into_iter()
        .map(|image| ContentBlock::Image {
            source: ImageSource {
                source_type: "base64".to_string(),
                media_type: image.media_type,
                data: image.data,
            },
        })
        .collect();
    content.push(ContentBlock::Text { text });
    Message::new(Role::User, content)
}
