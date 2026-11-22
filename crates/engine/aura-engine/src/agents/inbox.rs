//! Delivery to root agents.
//!
//! The host runs child agents itself, but a root agent's turns belong to
//! the surface that owns its conversation (a chat run, the terminal). A
//! child's reply or a detached task's result reaches a root through that
//! surface's [`ParentInbox`].

use async_trait::async_trait;
use aura_core_types::AgentId;

use super::mail::ThreadId;

/// Whether a message expects an answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageKind {
    /// A new message or task. When a hosted agent handles it, its final
    /// answer is sent back to the sender.
    Request,
    /// An answer or a finished task's result. Never answered automatically,
    /// which keeps two agents from replying to each other forever.
    Reply,
}

/// A message for a root agent.
#[derive(Debug, Clone)]
pub struct InboxMessage {
    /// Root agent the message is for.
    pub to: AgentId,
    /// Agent that sent it.
    pub from: AgentId,
    /// Sender's display name.
    pub from_name: String,
    /// Whether the sender expects an answer.
    pub kind: MessageKind,
    /// Message body.
    pub text: String,
    /// Subject of the message's thread.
    pub subject: String,
    /// Thread the message belongs to.
    pub thread: ThreadId,
}

impl InboxMessage {
    /// The message as the root agent's model should read it.
    #[must_use]
    pub fn render(&self) -> String {
        match self.kind {
            MessageKind::Request => format!(
                "Message from agent {} ({}):\nSubject: {}\n\n{}\n\nTo answer, call send_to_agent \
                 with agent_id \"{}\" and subject \"{}\"; use a new subject for a new topic.",
                self.from_name, self.from, self.subject, self.text, self.from, self.subject
            ),
            MessageKind::Reply => format!(
                "Agent {} ({}) reported back:\nSubject: {}\n\n{}",
                self.from_name, self.from, self.subject, self.text
            ),
        }
    }
}

/// Delivers messages to a root agent.
#[async_trait]
pub trait ParentInbox: Send + Sync {
    /// Hand `message` to the root's conversation. An error makes the host
    /// append the message to the root's record instead, so it is not lost.
    async fn deliver(&self, message: InboxMessage) -> Result<(), String>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(kind: MessageKind) -> InboxMessage {
        InboxMessage {
            to: AgentId::generate(),
            from: AgentId::generate(),
            from_name: "scout".into(),
            kind,
            text: "Found two issues".into(),
            subject: "Parser review".into(),
            thread: ThreadId("t3".into()),
        }
    }

    #[test]
    fn a_request_says_how_to_keep_the_thread() {
        let message = message(MessageKind::Request);
        let text = message.render();
        assert!(text.starts_with(&format!("Message from agent scout ({}):\n", message.from)));
        assert!(text.contains("Subject: Parser review\n"), "{text}");
        assert!(
            text.ends_with(&format!(
                "To answer, call send_to_agent with agent_id \"{}\" and subject \"Parser \
                 review\"; use a new subject for a new topic.",
                message.from
            )),
            "{text}"
        );
    }

    #[test]
    fn a_reply_names_its_subject() {
        let message = message(MessageKind::Reply);
        assert_eq!(
            message.render(),
            format!(
                "Agent scout ({}) reported back:\nSubject: Parser review\n\nFound two issues",
                message.from
            )
        );
    }
}
