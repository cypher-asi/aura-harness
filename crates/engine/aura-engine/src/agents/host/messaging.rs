//! Envelopes, mail records and queueing messages for agents.

use std::sync::Arc;

use aura_core_types::time::now_ms;
use aura_core_types::AgentId;
use tracing::info;

use crate::agents::inbox::{InboxMessage, MessageKind};
use crate::agents::mail::{MailMessage, Resolved};
use crate::agents::remote::RemoteMail;

use super::{record_message, Body, Envelope, HostInner, ThreadChoice};

impl HostInner {
    // ------------------------------------------------------------------
    // messaging
    // ------------------------------------------------------------------

    /// A message from `from` (the user when `None`) to `to`, placed in
    /// the thread `choice` picks.
    pub(super) fn envelope(
        &self,
        from: Option<AgentId>,
        to: AgentId,
        kind: MessageKind,
        text: String,
        choice: ThreadChoice<'_>,
    ) -> Envelope {
        let (from_name, root) = {
            let agents = self.agents.lock();
            let sender = from.and_then(|id| agents.get(&id).map(|e| (e.name.clone(), e.root(id))));
            sender.unwrap_or_else(|| {
                let root = agents.get(&to).map_or(to, |e| e.root(to));
                ("user".to_string(), root)
            })
        };
        let resolved = match (choice, from) {
            (ThreadChoice::Existing(thread, subject), _) => Resolved {
                thread,
                subject,
                from_text: false,
            },
            (ThreadChoice::Resolve(subject), Some(from)) => {
                self.threads.lock().resolve(root, from, to, subject, &text)
            }
            (ThreadChoice::Resolve(_) | ThreadChoice::New, _) => {
                self.threads.lock().new_thread(root, &text)
            }
        };
        Envelope {
            from,
            from_name,
            kind,
            text,
            thread: resolved.thread,
            subject: resolved.subject,
            subject_from_text: resolved.from_text,
            waiter: None,
            events: None,
            cancel: None,
        }
    }

    /// Send `text` from `from` to `to`. With a `subject` the message
    /// joins that subject's thread; without one it continues the last
    /// thread between the two agents.
    pub(in crate::agents) fn deliver_message(
        self: &Arc<Self>,
        from: Option<AgentId>,
        to: AgentId,
        kind: MessageKind,
        text: String,
        subject: Option<&str>,
    ) -> Result<(), String> {
        let envelope = self.envelope(from, to, kind, text, ThreadChoice::Resolve(subject));
        self.enqueue(to, envelope)
    }

    /// Send the request `text` from `from` to `to` in a thread of its
    /// own, as a delegated task starts one.
    pub(in crate::agents) fn deliver_in_new_thread(
        self: &Arc<Self>,
        from: AgentId,
        to: AgentId,
        text: String,
    ) -> Result<(), String> {
        let envelope = self.envelope(
            Some(from),
            to,
            MessageKind::Request,
            text,
            ThreadChoice::New,
        );
        self.enqueue(to, envelope)
    }

    /// Give `mail` its id and note its thread as the pair's latest.
    /// Takes the threads lock, so it may be called with the agents lock
    /// held: the threads lock is never held while taking the agents lock.
    fn record_mail(&self, mail: &mut MailMessage) {
        mail.id = self
            .threads
            .lock()
            .record(mail.from, mail.to, &mail.thread, &mail.subject);
    }

    /// Show recorded `mail` to the mail observer. Call with no host lock
    /// held: the observer runs inline.
    fn show_mail(&self, mail: &MailMessage) {
        let observer = self.mail_observer.lock().clone();
        if let Some(observer) = observer {
            observer.on_mail(mail);
        }
    }

    /// Record `mail` and show it. Call with no host lock held.
    pub(super) fn post_mail(&self, mut mail: MailMessage) {
        self.record_mail(&mut mail);
        self.show_mail(&mail);
    }

    /// Queue `envelope` for a hosted agent and wake it, or hand it to a
    /// root's inbox. A message from an agent is posted as mail once it is
    /// accepted; its id is given while the envelope is queued, so ids
    /// follow queue order.
    pub(super) fn enqueue(self: &Arc<Self>, to: AgentId, envelope: Envelope) -> Result<(), String> {
        let (root_inbox, mail) = {
            let mut agents = self.agents.lock();
            let Some(entry) = agents.get_mut(&to) else {
                drop(agents);
                return self.route_remote(to, envelope);
            };
            let mut mail = unsent_mail(&envelope, to, &entry.name);
            if let Some(mail) = mail.as_mut() {
                self.record_mail(mail);
            }
            match &mut entry.body {
                Body::Root { inbox } => (Some((inbox.clone(), envelope)), mail),
                Body::Hosted { queue, .. } => {
                    queue.push_back(envelope);
                    (None, mail)
                }
            }
        };
        match root_inbox {
            None => {
                if let Some(mail) = mail {
                    self.show_mail(&mail);
                }
                self.pump(to);
                Ok(())
            }
            Some((inbox, envelope)) => {
                let (Some(from), Some(mail)) = (envelope.from, mail) else {
                    return Err("roots only accept messages from agents".to_string());
                };
                self.show_mail(&mail);
                let message = InboxMessage {
                    to,
                    from,
                    from_name: envelope.from_name,
                    kind: envelope.kind,
                    text: envelope.text,
                    subject: envelope.subject,
                    thread: envelope.thread,
                };
                let store = self.store.clone();
                let observer = self.scheduler.record_observer().cloned();
                tokio::spawn(async move {
                    let delivered = match inbox {
                        Some(inbox) => inbox.deliver(message.clone()).await,
                        None => Err("no inbox".to_string()),
                    };
                    if let Err(err) = delivered {
                        info!(root = %to, %err, "root inbox unavailable; recording message");
                        record_message(&store, observer.as_ref(), &message);
                    }
                });
                Ok(())
            }
        }
    }

    /// Send `envelope` for `to` to the node holding it, or fail as an
    /// unknown agent when no node does.
    fn route_remote(&self, to: AgentId, envelope: Envelope) -> Result<(), String> {
        let router = self.remote.lock().clone();
        let unknown = || format!("unknown agent {to}");
        let Some(router) = router else {
            return Err(unknown());
        };
        let Some(to_name) = router.name_of(to) else {
            return Err(unknown());
        };
        let Some(from) = envelope.from else {
            return Err("only agents can message agents on other nodes".to_string());
        };
        let mut mail = unsent_mail(&envelope, to, &to_name);
        router.route(RemoteMail {
            from,
            from_name: envelope.from_name,
            to,
            kind: envelope.kind,
            thread: envelope.thread,
            subject: envelope.subject,
            text: envelope.text,
        })?;
        if let Some(mail) = mail.as_mut() {
            self.record_mail(mail);
            self.show_mail(mail);
        }
        Ok(())
    }

    /// The router's peers for agents under `root`.
    pub(in crate::agents) fn remote_peers(&self, root: AgentId) -> Vec<serde_json::Value> {
        self.remote
            .lock()
            .clone()
            .map(|router| router.peers(root))
            .unwrap_or_default()
    }

    /// Whether another node holds `agent`.
    pub(in crate::agents) fn is_remote(&self, agent: AgentId) -> bool {
        self.remote
            .lock()
            .clone()
            .is_some_and(|router| router.name_of(agent).is_some())
    }
}

/// `envelope` as mail to `to`, before [`HostInner::post_mail`] gives it
/// an id. `None` for a message from the user, which is not mail.
fn unsent_mail(envelope: &Envelope, to: AgentId, to_name: &str) -> Option<MailMessage> {
    let from = envelope.from?;
    Some(MailMessage {
        id: 0,
        thread: envelope.thread.clone(),
        subject: envelope.subject.clone(),
        from,
        from_name: envelope.from_name.clone(),
        to,
        to_name: to_name.to_string(),
        kind: envelope.kind,
        text: envelope.text.clone(),
        sent_ms: now_ms(),
        subject_from_text: envelope.subject_from_text,
    })
}
