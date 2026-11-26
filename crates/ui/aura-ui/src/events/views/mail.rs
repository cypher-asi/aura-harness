//! Messages between agents, for the INBOX tab.

/// One message between two agents, for the Kernel panel's INBOX tab.
/// Messages with the same `thread_id` are one conversation, the way an
/// email subject groups mail.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MailView {
    /// Unique per message, in the order messages were sent
    pub id: u64,
    /// The conversation the message belongs to
    pub thread_id: String,
    /// The conversation's subject
    pub subject: String,
    /// Hex id of the sender
    pub from_id: String,
    /// Sender's display name
    pub from_name: String,
    /// Hex id of the recipient
    pub to_id: String,
    /// Recipient's display name
    pub to_name: String,
    /// An answer or a finished task's result, rather than a new request
    pub reply: bool,
    /// Message body
    pub text: String,
    /// When it was sent, in Unix milliseconds
    pub sent_ms: u64,
}
