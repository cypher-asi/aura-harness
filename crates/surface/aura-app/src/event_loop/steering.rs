//! Messages for an agent whose turn is running.
//!
//! Instead of waiting for the turn to end, a message goes straight into
//! the running agent loop, which adds it to the conversation before its
//! next model call. A message that arrives just as the turn finishes may
//! never reach the model; [`Steering::finish`] finds those so the event
//! loop can run them as the next turn instead of dropping them.

use super::subagents::lock;
use aura_agent::{AgentRunnerHandle, SessionId, UserInput};
use aura_model::{ContentBlock, Message, Role};
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

/// Who a mid-turn message is from, which decides how the model sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SteerSource {
    /// Typed by the user.
    User,
    /// Written by the harness, e.g. a background job exiting. Run as a
    /// turn of its own, it carries on with the agent's latest task rather
    /// than starting one (see [`super::tasks`]).
    Harness,
}

impl SteerSource {
    pub(crate) const fn prefix(self) -> &'static str {
        match self {
            Self::User => "The user sent this message while you were working:\n",
            Self::Harness => "Harness notice:\n",
        }
    }
}

/// A message sent into a turn: what the model sees and what was sent.
struct Sent {
    source: SteerSource,
    wrapped: String,
    text: String,
}

/// Mid-turn input for one running turn.
#[derive(Clone)]
pub(crate) struct Steering {
    handle: AgentRunnerHandle,
    sent: Arc<Mutex<Vec<Sent>>>,
}

impl Steering {
    pub(crate) fn new(cancel: &CancellationToken) -> Self {
        Self {
            handle: AgentRunnerHandle::new(SessionId::new_v4(), cancel.clone()),
            sent: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub(crate) fn handle(&self) -> &AgentRunnerHandle {
        &self.handle
    }

    /// Send `text` into the running turn, with the model getting `body`:
    /// the text itself, or the text with mentioned files appended. A
    /// message the model never got runs next as `text`, so it is expanded
    /// again then. Returns `false` when the turn already stopped taking
    /// input.
    pub(crate) async fn send(&self, source: SteerSource, text: String, body: String) -> bool {
        let wrapped = format!("{}{body}", source.prefix());
        let delivered = self
            .handle
            .send_user_input(UserInput::Message(wrapped.clone()))
            .await
            .is_ok();
        if delivered {
            lock(&self.sent).push(Sent {
                source,
                wrapped,
                text,
            });
        }
        delivered
    }

    /// Stop taking input and split what was sent into messages the model
    /// saw and messages it never got, both oldest first.
    pub(crate) fn finish(&self, messages: &[Message]) -> (Vec<String>, Vec<(SteerSource, String)>) {
        self.handle.close();
        let sent = std::mem::take(&mut *lock(&self.sent));
        let user_text: Vec<&str> = messages
            .iter()
            .filter(|m| m.role == Role::User)
            .flat_map(|m| m.content.iter())
            .filter_map(|block| match block {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        let (seen, unseen): (Vec<Sent>, Vec<Sent>) = sent
            .into_iter()
            .partition(|sent| user_text.iter().any(|t| t.contains(&sent.wrapped)));
        (
            seen.into_iter().map(|s| s.text).collect(),
            unseen.into_iter().map(|s| (s.source, s.text)).collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn finish_separates_seen_from_unseen_messages() {
        let steering = Steering::new(&CancellationToken::new());
        let send = |source, text: &str, body: &str| {
            steering.send(source, text.to_string(), body.to_string())
        };
        assert!(send(SteerSource::User, "first @a.rs", "first @a.rs\n\nfile").await);
        assert!(send(SteerSource::Harness, "second @b.rs", "second @b.rs\n\nfile").await);
        let messages = vec![
            Message::user("task"),
            Message::assistant("working"),
            Message::user(format!("{}first @a.rs\n\nfile", SteerSource::User.prefix())),
        ];

        let (seen, unseen) = steering.finish(&messages);

        assert_eq!(seen, vec!["first @a.rs".to_string()]);
        assert_eq!(
            unseen,
            vec![(SteerSource::Harness, "second @b.rs".to_string())],
            "a message the model never got runs again as typed"
        );
        assert!(
            !send(SteerSource::User, "late", "late").await,
            "closed after finish"
        );
    }
}
