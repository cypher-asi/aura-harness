//! A message on its way: sent by an agent to a peer ([`send`]), then
//! given to the peer on the event loop ([`deliver`]).

use super::*;

/// Send `text` of `kind` from the agent of `circle` to `to`, about
/// `subject`, for the sender's todo `todo`: post it to the INBOX tab and
/// hand it to the event loop. Returns the subject. Without a subject, an
/// assignment is about its text's first line, being new work; another
/// message goes on with the subject the two last wrote about, if any.
///
/// # Errors
/// What the sender is told instead when the thread has carried
/// [`THREAD_REQUESTS`] requests already, or the terminal is shutting
/// down.
pub(super) fn send(
    hub: &AgentHub,
    circle: &Circle,
    kind: InboxKind,
    to: &Peer,
    subject: Option<&str>,
    text: &str,
    todo: Option<&str>,
) -> Result<String, String> {
    let envelope = Envelope {
        kind,
        id: new_id(),
        from: &circle.me,
        to: &to.agent,
    };
    let filed = envelope.file(&mut lock(&hub.inbox.state), subject, text, todo);
    let Some((subject, thread)) = filed else {
        return Err(thread_full(circle, to));
    };
    envelope.post(hub, &subject, &thread, text)?;
    Ok(subject)
}

/// What a message is and who it goes between.
struct Envelope<'a> {
    kind: InboxKind,
    id: String,
    from: &'a Named,
    to: &'a Named,
}

impl Envelope<'_> {
    /// Under the inbox's lock, `state`: settle what the message `text` is
    /// about ([`send`]) and its thread, count it against the thread when
    /// it is a request, and when the recipient answers on its own, have it
    /// owe an answer, for the sender's todo `todo`. Returns the subject
    /// and the thread; `None` when the thread has carried
    /// [`THREAD_REQUESTS`] requests already.
    fn file(
        &self,
        state: &mut State,
        subject: Option<&str>,
        text: &str,
        todo: Option<&str>,
    ) -> Option<(String, String)> {
        let pair = pair(self.from.agent_id, self.to.agent_id);
        let subject = subject
            .map(str::trim)
            .filter(|subject| !subject.is_empty())
            .map(str::to_string)
            .or_else(|| {
                (self.kind != InboxKind::Assignment)
                    .then(|| state.subjects.get(&pair).cloned())
                    .flatten()
            })
            .unwrap_or_else(|| subject_of(text));
        let thread = thread_id(self.from.agent_id, self.to.agent_id, &subject);
        if self.kind == InboxKind::Request {
            let carried = state.requests.entry(thread.clone()).or_default();
            if *carried >= THREAD_REQUESTS {
                return None;
            }
            *carried += 1;
        }
        state.subjects.insert(pair, subject.clone());
        if self.to.answers() {
            state.owed.entry(self.to.agent_id).or_default().push(Owed {
                id: self.id.clone(),
                kind: self.kind,
                from: self.from.agent_id,
                thread: thread.clone(),
                subject: subject.clone(),
                todo: todo.map(str::to_string),
                withdrawn: false,
            });
        }
        Some((subject, thread))
    }

    /// Post the message `text`, about `subject` in `thread`, to the INBOX
    /// tab and hand it to the event loop.
    ///
    /// # Errors
    /// What the sender is told when the terminal is shutting down.
    fn post(&self, hub: &AgentHub, subject: &str, thread: &str, text: &str) -> Result<(), String> {
        let (kind, from, recipient) = (self.kind, self.from, self.to);
        let header = inbox::header(kind, &self.id, from.short(), &from.name, subject);
        let mut prompt = format!("{header}\n\n{}", text.trim());
        if let Some(footer) = footer(kind, from, recipient.answers()) {
            prompt.push_str("\n\n");
            prompt.push_str(&footer);
        }
        hub.post_mail(mail_view(kind, thread, subject, (from, recipient), text));
        info!(
            from = %from.agent_id.to_hex(),
            to = %recipient.agent_id.to_hex(),
            ?kind,
            %subject,
            "Sending an inbox message"
        );
        let notice = Notice {
            to: recipient.agent_id,
            shown: shown(&header),
            prompt,
            inbox: Some(kind),
        };
        super::saved::send(hub, notice).map_err(|()| "the terminal is shutting down".to_string())
    }
}

/// What a message of `kind` from `from` ends with: how the answer gets
/// back, when the recipient `answers` on its own, else how to give one.
fn footer(kind: InboxKind, from: &Named, answers: bool) -> Option<String> {
    let from = from.short();
    match (kind, answers) {
        (InboxKind::Reply, _) => None,
        (InboxKind::Assignment, true) => Some(format!(
            "When it is done, end your turn with a short report: it goes back to {from} on its own."
        )),
        (InboxKind::Request, true) => Some(format!(
            "End your turn with your answer: it goes back to {from} on its own."
        )),
        (_, false) => Some(format!(
            "Answer {from} with `send_to_agent` if it needs an answer."
        )),
    }
}

/// What the sender is told when its thread with `to` is full.
fn thread_full(circle: &Circle, to: &Peer) -> String {
    let with = to.agent.short();
    let full = format!("Your thread with {with} has carried {THREAD_REQUESTS} messages.");
    match circle.coordinator() {
        Some(coordinator) if coordinator.agent.agent_id != to.agent.agent_id => format!(
            "{full} Stop going back and forth: take what is still open to {} in a short report.",
            coordinator.agent.short()
        ),
        _ if circle.coordinates() => {
            format!("{full} Decide it yourself, or tell the channel what is blocked.")
        }
        _ => format!("{full} Stop going back and forth and end your turn with what is still open."),
    }
}

/// The line a chat shows for a message with `header` sent into a running
/// turn, which the chat doesn't show itself.
pub(super) fn shown(header: &str) -> String {
    format!("✉ {}", header.lines().next().unwrap_or_default())
}

/// The INBOX tab's view of a message of `kind` in `thread`.
pub(super) fn mail_view(
    kind: InboxKind,
    thread: &str,
    subject: &str,
    (from, to): (&Named, &Named),
    text: &str,
) -> MailView {
    MailView {
        id: next_mail_id(),
        thread_id: thread.to_string(),
        subject: subject.to_string(),
        from_id: from.agent_id.to_hex(),
        from_name: from.full(),
        to_id: to.agent_id.to_hex(),
        to_name: to.full(),
        reply: kind == InboxKind::Reply,
        text: text.to_string(),
        sent_ms: now_ms(),
    }
}

/// `a` and `b` in one order, whichever of them writes.
fn pair(a: AgentId, b: AgentId) -> (AgentId, AgentId) {
    if a.to_hex() <= b.to_hex() {
        (a, b)
    } else {
        (b, a)
    }
}

/// The thread of `a` and `b` about `subject`, whichever of them writes.
pub(super) fn thread_id(a: AgentId, b: AgentId, subject: &str) -> String {
    let (low, high) = pair(a, b);
    let key = format!("{}:{}:{subject}", low.to_hex(), high.to_hex());
    let hash = blake3::hash(key.as_bytes()).to_hex();
    format!("inbox:{}", &hash.as_str()[..16])
}

/// A new message id, unique enough to find a message in a conversation.
pub(super) fn new_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..12].to_string()
}

/// Give the inbox message `notice` of `kind` to its agent. An assignment
/// is a new task; any other message starts a turn that carries on with
/// the agent's latest task. Either waits in the agent's queue while it
/// is busy: a message never goes into a running turn, which would make
/// the turn carry on past the reply it was writing, and never stops one.
/// A new member of a channel whose agents run in the cloud first starts
/// moving to a machine of its own, and the message waits for the move.
pub(in crate::event_loop) async fn deliver(
    state: &mut LoopState<'_>,
    notice: Notice,
    kind: InboxKind,
) {
    let Notice {
        to, shown, prompt, ..
    } = notice;
    super::saved::delivered(&state.hub, to, &prompt);
    info!(agent = %to, %shown, "Delivering an inbox message");
    if kind == InboxKind::Assignment {
        crate::event_loop::factory::hooks::on_assignment(state, to);
        // A member may have joined: every placed member reaches it now.
        crate::event_loop::cloud::refresh_reach(state).await;
    }
    // A member that can't have its machine fails, and its senders are
    // told: the message must not run here instead.
    if lock(&state.hub.cloud_starts).remove(&to)
        && !crate::event_loop::cloud::start_in_cloud(state, to).await
    {
        return;
    }
    let request = if kind == InboxKind::Assignment {
        Request::New
    } else {
        Request::Resume
    };
    let work = Work::Prompt {
        prompt: Prompt::inbox(prompt, request),
        shown: false,
    };
    super::submit(state, to, work).await;
}
