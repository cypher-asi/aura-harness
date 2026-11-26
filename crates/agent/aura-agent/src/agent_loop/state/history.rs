//! [`History`]: the conversation shared with the last model request.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use aura_model::Message;

/// The conversation, shared with the model request last sent from it.
/// Changing it copies the messages only while that request still holds
/// them.
#[derive(Debug, Default)]
pub(crate) struct History {
    messages: Arc<Vec<Message>>,
    /// [`aura_context_compaction::estimate_message_chars`] of `messages`, or
    /// [`UNCOUNTED`] until asked for since they last changed
    chars: AtomicUsize,
}

const UNCOUNTED: usize = usize::MAX;

impl History {
    /// The messages, for a model request.
    pub(crate) fn shared(&self) -> Arc<Vec<Message>> {
        Arc::clone(&self.messages)
    }

    pub(crate) fn into_vec(self) -> Vec<Message> {
        Arc::unwrap_or_clone(self.messages)
    }

    /// [`aura_context_compaction::estimate_message_chars`] of the messages, counted
    /// once per change.
    pub(crate) fn chars(&self) -> usize {
        let chars = self.chars.load(Ordering::Relaxed);
        if chars != UNCOUNTED {
            return chars;
        }
        let chars = aura_context_compaction::estimate_message_chars(&self.messages);
        self.chars.store(chars, Ordering::Relaxed);
        chars
    }
}

impl Clone for History {
    fn clone(&self) -> Self {
        Self {
            messages: Arc::clone(&self.messages),
            chars: AtomicUsize::new(self.chars.load(Ordering::Relaxed)),
        }
    }
}

impl From<Vec<Message>> for History {
    fn from(messages: Vec<Message>) -> Self {
        Self {
            messages: Arc::new(messages),
            chars: AtomicUsize::new(UNCOUNTED),
        }
    }
}

impl std::ops::Deref for History {
    type Target = Vec<Message>;

    fn deref(&self) -> &Vec<Message> {
        &self.messages
    }
}

impl std::ops::DerefMut for History {
    fn deref_mut(&mut self) -> &mut Vec<Message> {
        *self.chars.get_mut() = UNCOUNTED;
        Arc::make_mut(&mut self.messages)
    }
}
