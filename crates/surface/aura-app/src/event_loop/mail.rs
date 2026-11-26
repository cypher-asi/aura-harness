//! Mail between agents for the INBOX tab.
//!
//! Two sources feed it, and both end as a [`MailItem`] on one unbounded
//! channel that the event loop forwards to the UI as
//! [`UiCommand::AddMail`]:
//!
//! - The [`aura_engine::AgentHost`] reports every message between hosted
//!   agents to a [`Mailbox`].
//! - The terminal's permanent-agent `task` calls run outside the host, so
//!   [`TaskMail`] posts the request and its result itself, in a thread of
//!   its own named `task:<child hex>` (the Processes tab's row id).
//! - Messages between agents' inboxes (see [`super::inbox`]) are posted
//!   by the inbox, in threads named `inbox:<hash>`.
//!
//! Agent ids are hex ([`AgentId::to_hex`]), the same form the process
//! table uses, so the UI can match mail to the agents of a run.

use super::processes::task_id;
use aura_core_types::time::now_ms;
use aura_core_types::AgentId;
use aura_engine::{MailMessage, MailObserver, MessageKind};
use aura_ui::events::MailView;
use aura_ui::UiCommand;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::mpsc;
use tracing::debug;

/// Where mail for the UI is sent.
pub type MailSender = mpsc::UnboundedSender<MailItem>;

/// A message for the INBOX tab, and who to title its thread through.
#[derive(Debug, Clone, PartialEq)]
pub struct MailItem {
    pub view: MailView,
    /// Sender whose kernel writes the thread a subject, when the message
    /// opened a thread named after its first line.
    pub title_from: Option<AgentId>,
}

/// The first id the terminal gives mail it posts itself. The host numbers
/// its own messages from 1, so starting this high keeps the two apart.
const FIRST_TASK_MAIL_ID: u64 = 1 << 40;

/// The next id the terminal gives mail it posts itself, shared by the
/// process.
static NEXT_TASK_MAIL_ID: AtomicU64 = AtomicU64::new(FIRST_TASK_MAIL_ID);

/// A new id for mail the terminal posts itself ([`TaskMail`] and inbox
/// messages), never one the host gives.
pub(crate) fn next_mail_id() -> u64 {
    NEXT_TASK_MAIL_ID.fetch_add(1, Ordering::Relaxed)
}

/// Longest subject taken from a `task` prompt, in characters.
const SUBJECT_CHARS: usize = 60;

/// Subject of a `task` whose prompt has no text.
const EMPTY_SUBJECT: &str = "(no subject)";

/// Hands the host's mail to the event loop.
pub struct Mailbox {
    tx: MailSender,
}

impl Mailbox {
    #[must_use]
    pub fn new(tx: MailSender) -> Self {
        Self { tx }
    }
}

impl MailObserver for Mailbox {
    fn on_mail(&self, message: &MailMessage) {
        // The event loop has ended (the terminal is shutting down), so
        // nobody is left to show the message.
        let item = MailItem {
            view: mail_view(message),
            title_from: message.subject_from_text.then_some(message.from),
        };
        if self.tx.send(item).is_err() {
            debug!(id = message.id, "Mail after the event loop ended; dropped");
        }
    }
}

/// `message` as the UI shows it.
#[must_use]
pub fn mail_view(message: &MailMessage) -> MailView {
    MailView {
        id: message.id,
        thread_id: message.thread.0.clone(),
        subject: message.subject.clone(),
        from_id: message.from.to_hex(),
        from_name: message.from_name.clone(),
        to_id: message.to.to_hex(),
        to_name: message.to_name.clone(),
        reply: message.kind == MessageKind::Reply,
        text: message.text.clone(),
        sent_ms: message.sent_ms,
    }
}

/// Show `view` in the INBOX tab.
pub(super) async fn forward(commands: &mpsc::Sender<UiCommand>, view: MailView) {
    // A closed channel means the UI is gone; the loop is about to end.
    let _ = commands.send(UiCommand::AddMail(view)).await;
}

/// The thread of one permanent-agent `task`: the parent's request to the
/// instance, and the result of whichever instance answered it.
#[derive(Debug, Clone)]
pub(crate) struct TaskMail {
    parent: AgentId,
    parent_name: String,
    child: AgentId,
    child_name: String,
    subject: String,
}

impl TaskMail {
    /// The thread of the task `parent` (shown as `parent_name`) gave
    /// `child` (shown as `child_name`), named after `prompt`.
    pub(crate) fn new(
        parent: AgentId,
        parent_name: String,
        child: AgentId,
        child_name: String,
        prompt: &str,
    ) -> Self {
        Self {
            parent,
            parent_name,
            child,
            child_name,
            subject: subject_of(prompt),
        }
    }

    /// The parent asks the instance `prompt`.
    pub(crate) fn request(&self, prompt: &str) -> MailView {
        self.message(
            (self.parent, &self.parent_name),
            (self.child, &self.child_name),
            false,
            prompt.to_string(),
        )
    }

    /// Instance `replier` (shown as `replier_name`) reports how the task
    /// went: its reply, or why it has none. After a Gate retry that is
    /// the retry's instance, not the first; the thread stays the first
    /// instance's.
    pub(crate) fn reply(
        &self,
        replier: AgentId,
        replier_name: &str,
        outcome: Result<String, String>,
    ) -> MailView {
        let text = match outcome {
            Ok(reply) => reply,
            Err(reason) => format!("failed: {reason}"),
        };
        self.message(
            (replier, replier_name),
            (self.parent, &self.parent_name),
            true,
            text,
        )
    }

    fn message(
        &self,
        (from, from_name): (AgentId, &str),
        (to, to_name): (AgentId, &str),
        reply: bool,
        text: String,
    ) -> MailView {
        MailView {
            id: next_mail_id(),
            thread_id: task_id(self.child),
            subject: self.subject.clone(),
            from_id: from.to_hex(),
            from_name: from_name.to_string(),
            to_id: to.to_hex(),
            to_name: to_name.to_string(),
            reply,
            text,
            sent_ms: now_ms(),
        }
    }
}

/// The first non-empty line of `prompt`, whitespace collapsed, cut to
/// [`SUBJECT_CHARS`] characters with `…` when cut: the subject of a
/// message that names none.
pub(crate) fn subject_of(prompt: &str) -> String {
    let Some(line) = prompt.lines().map(str::trim).find(|line| !line.is_empty()) else {
        return EMPTY_SUBJECT.to_string();
    };
    let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = line.chars();
    let head: String = chars.by_ref().take(SUBJECT_CHARS).collect();
    if chars.next().is_some() {
        format!("{}…", head.trim_end())
    } else {
        head
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aura_engine::ThreadId;

    fn message(kind: MessageKind) -> MailMessage {
        MailMessage {
            id: 7,
            thread: ThreadId("t3".into()),
            subject: "Parser review".into(),
            from: AgentId::generate(),
            from_name: "Lead".into(),
            to: AgentId::generate(),
            to_name: "scout".into(),
            kind,
            text: "Check the tokenizer".into(),
            sent_ms: 1_700_000_000_000,
            subject_from_text: false,
        }
    }

    #[test]
    fn mail_view_maps_fields_and_hex_ids() {
        let request = message(MessageKind::Request);
        let view = mail_view(&request);
        assert_eq!(
            view,
            MailView {
                id: 7,
                thread_id: "t3".into(),
                subject: "Parser review".into(),
                from_id: request.from.to_hex(),
                from_name: "Lead".into(),
                to_id: request.to.to_hex(),
                to_name: "scout".into(),
                reply: false,
                text: "Check the tokenizer".into(),
                sent_ms: 1_700_000_000_000,
            }
        );
        assert!(mail_view(&message(MessageKind::Reply)).reply);
    }

    #[test]
    fn mailbox_forwards_into_the_channel_and_survives_a_closed_one() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mailbox = Mailbox::new(tx);
        let sent = message(MessageKind::Request);
        mailbox.on_mail(&sent);
        assert_eq!(
            rx.try_recv().unwrap(),
            MailItem {
                view: mail_view(&sent),
                title_from: None,
            }
        );

        drop(rx);
        mailbox.on_mail(&sent);
    }

    #[test]
    fn only_a_thread_named_after_its_first_line_is_titled_by_its_sender() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mailbox = Mailbox::new(tx);
        let opened = MailMessage {
            subject_from_text: true,
            ..message(MessageKind::Request)
        };
        let named = message(MessageKind::Request);

        mailbox.on_mail(&opened);
        mailbox.on_mail(&named);

        assert_eq!(rx.try_recv().unwrap().title_from, Some(opened.from));
        assert_eq!(rx.try_recv().unwrap().title_from, None);
    }

    #[tokio::test]
    async fn forwarded_mail_becomes_add_mail() {
        let (commands, mut ui) = mpsc::channel(4);
        let view = mail_view(&message(MessageKind::Reply));
        forward(&commands, view.clone()).await;
        match ui.recv().await {
            Some(UiCommand::AddMail(got)) => assert_eq!(got, view),
            other => panic!("expected AddMail, got {other:?}"),
        }
    }

    #[test]
    fn task_mail_is_a_request_then_a_reply_in_the_childs_thread() {
        let parent = AgentId::generate();
        let child = AgentId::generate();
        let mail = TaskMail::new(
            parent,
            "Lead".into(),
            child,
            "reviewer #1".into(),
            "\n  Review   the diff\nThen report",
        );

        let request = mail.request("Review the diff\nThen report");
        let reply = mail.reply(child, "reviewer #1", Ok("Looks good.".into()));
        let failed = mail.reply(child, "reviewer #1", Err("timed out".into()));

        let thread = format!("task:{}", child.to_hex());
        for view in [&request, &reply, &failed] {
            assert_eq!(view.thread_id, thread);
            assert_eq!(view.subject, "Review the diff");
            assert!(view.id >= FIRST_TASK_MAIL_ID);
        }
        assert!(request.id < reply.id && reply.id < failed.id);

        assert!(!request.reply);
        assert_eq!(
            (request.from_id.as_str(), request.to_id.as_str()),
            (parent.to_hex().as_str(), child.to_hex().as_str())
        );
        assert_eq!(
            (request.from_name.as_str(), request.to_name.as_str()),
            ("Lead", "reviewer #1")
        );
        assert_eq!(request.text, "Review the diff\nThen report");

        assert!(reply.reply);
        assert_eq!(
            (reply.from_id.as_str(), reply.to_id.as_str()),
            (child.to_hex().as_str(), parent.to_hex().as_str())
        );
        assert_eq!(reply.from_name, "reviewer #1");
        assert_eq!(reply.text, "Looks good.");
        assert_eq!(failed.text, "failed: timed out");
    }

    #[test]
    fn a_retried_task_reply_comes_from_the_retry_in_the_first_thread() {
        let parent = AgentId::generate();
        let first = AgentId::generate();
        let retry = AgentId::generate();
        let mail = TaskMail::new(parent, "Lead".into(), first, "reviewer #1".into(), "Review");

        let reply = mail.reply(retry, "reviewer #2", Ok("Fixed.".into()));

        assert_eq!(reply.thread_id, format!("task:{}", first.to_hex()));
        assert_eq!(reply.from_id, retry.to_hex());
        assert_eq!(reply.from_name, "reviewer #2");
        assert_eq!(
            (reply.to_id.as_str(), reply.to_name.as_str()),
            (parent.to_hex().as_str(), "Lead")
        );
        assert!(reply.reply);
    }

    #[test]
    fn task_subjects_are_cut_on_char_boundaries() {
        let cut = subject_of(&"é".repeat(80));
        assert_eq!(cut.chars().count(), SUBJECT_CHARS + 1);
        assert!(cut.ends_with('…'));
        assert_eq!(subject_of(&"a".repeat(60)), "a".repeat(60));
        assert_eq!(subject_of(" \n\t"), EMPTY_SUBJECT);
    }
}
