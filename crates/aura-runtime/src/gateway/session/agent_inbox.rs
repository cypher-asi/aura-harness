//! Messages from spawned agents to a chat session's agent.
//!
//! A chat run rejects user messages while a turn is running, so agent
//! messages wait in the session's [`AgentMailbox`] and the run's idle loop
//! starts a turn for each one.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, PoisonError, Weak};

use async_trait::async_trait;
use aura_engine::{InboxMessage, ParentInbox};
use tokio::sync::Notify;

/// Agent messages waiting for the session to be idle.
#[derive(Default)]
pub(crate) struct AgentMailbox {
    queue: Mutex<VecDeque<InboxMessage>>,
    ready: Notify,
}

impl AgentMailbox {
    pub(crate) fn pop(&self) -> Option<InboxMessage> {
        self.queue
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop_front()
    }

    /// Resolves once a message has been delivered since the last call.
    pub(crate) async fn ready(&self) {
        self.ready.notified().await;
    }

    fn push(&self, message: InboxMessage) {
        self.queue
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push_back(message);
        self.ready.notify_one();
    }
}

/// [`ParentInbox`] for a session's agent. Holds the mailbox weakly: once
/// the run is gone, delivery fails and the host records the message on
/// the agent's log instead.
pub(crate) struct SessionInbox(Weak<AgentMailbox>);

impl SessionInbox {
    pub(crate) fn new(mailbox: &Arc<AgentMailbox>) -> Self {
        Self(Arc::downgrade(mailbox))
    }
}

#[async_trait]
impl ParentInbox for SessionInbox {
    async fn deliver(&self, message: InboxMessage) -> Result<(), String> {
        let mailbox = self
            .0
            .upgrade()
            .ok_or_else(|| "the chat run has closed".to_string())?;
        mailbox.push(message);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aura_core_types::AgentId;
    use aura_engine::MessageKind;

    fn message(text: &str) -> InboxMessage {
        InboxMessage {
            to: AgentId::generate(),
            from: AgentId::generate(),
            from_name: "helper".into(),
            kind: MessageKind::Reply,
            text: text.into(),
        }
    }

    #[tokio::test]
    async fn delivered_messages_wake_the_session_in_order() {
        let mailbox = Arc::new(AgentMailbox::default());
        let inbox = SessionInbox::new(&mailbox);
        inbox.deliver(message("one")).await.unwrap();
        inbox.deliver(message("two")).await.unwrap();

        mailbox.ready().await;
        assert_eq!(mailbox.pop().unwrap().text, "one");
        assert_eq!(mailbox.pop().unwrap().text, "two");
        assert!(mailbox.pop().is_none());
    }

    #[tokio::test]
    async fn delivery_fails_once_the_session_is_gone() {
        let mailbox = Arc::new(AgentMailbox::default());
        let inbox = SessionInbox::new(&mailbox);
        drop(mailbox);
        assert!(inbox.deliver(message("late")).await.is_err());
    }
}
