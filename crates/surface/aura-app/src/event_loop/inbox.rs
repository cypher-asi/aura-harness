//! Agents talking to each other through their inboxes.
//!
//! A top-level agent reaches its peers with `send_to_agent`, `list_agents`
//! and `get_agent_state` ([`PeerHooks`]), and a channel's coordinator
//! hands them work with `assign` ([`AssignTool`]). An agent's peers:
//!
//! - a coordinator's are the other top-level members of its channel;
//! - a channel member's are the other top-level members of its channel,
//!   the coordinator among them, and the top-level agents of its project;
//! - any other top-level agent's are the top-level agents of its project.
//!
//! Subagents have no peers: like any target that isn't a peer, theirs
//! are reached through the background agents' hooks when those run.
//!
//! A message reaches its agent like a harness notice: into its running
//! turn, else as the prompt of a turn that carries on with its latest
//! task. An assignment is a task of its own instead, and waits while the
//! agent is busy. A message's first line says who sent it and what about
//! and its second names it ([`aura_ui_terminal::inbox`]), so chats fold
//! it to one line. Every message also goes to the INBOX tab.
//!
//! A request or an assignment is answered on its own: when the turn that
//! saw it ends, the agent's last words go back as a reply, which closes
//! the todo it was for. Coordinators answer nobody on their own, as what
//! they write goes to their channel, and replies are never answered. One
//! thread between two agents carries at most [`THREAD_REQUESTS`]
//! requests, so two agents can't keep each other busy forever.
//!
//! # Invariants
//!
//! * The channel store is read off the async runtime.
//! * Locks are never held across an `.await`.
//!
//! # Failure modes
//!
//! A channel that cannot be read leaves its agent without channel peers
//! (logged): messages to them fail as to an unknown agent. A message sent
//! as the terminal shuts down is lost.

use super::agent_events::AgentUi;
use super::channels;
use super::handlers::system_message;
use super::mail::{next_mail_id, subject_of};
use super::notices::Notice;
use super::processes::now_ms;
use super::steering::SteerSource;
use super::subagents::{assign_tool_definition, lock, AgentHub};
use super::turns::{Prompt, Request, Work};
use super::LoopState;
use crate::agent_definitions::COORDINATOR_DEFINITION;
use crate::agent_registry::RegisteredAgent;
use async_trait::async_trait;
use aura_core_types::{AgentId, ToolDefinition, ToolResult};
use aura_engine::AgentHooks;
use aura_exec_tools::todo::{task_report_line, ASSIGN_TOOL_NAME};
use aura_exec_tools::{AgentControlHook, AgentReadHook, Tool, ToolContext, ToolError};
use aura_model::{Message, Role};
use aura_store_db::{ChannelRecord, MemberRecord, MemberRole};
use aura_ui_terminal::events::MailView;
use aura_ui_terminal::inbox::{self, InboxKind};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, Weak};
use tracing::{info, warn};

/// Requests one thread between two agents carries; then they have to
/// settle it some other way.
pub(crate) const THREAD_REQUESTS: u32 = 8;

/// Longest reply, in characters.
const REPLY_CHARS: usize = 6000;

/// Longest last report `list_agents` and `get_agent_state` show, in
/// characters.
const REPORT_CHARS: usize = 300;

/// Longest reason a reply gives for work that stopped, in characters.
const STOPPED_CHARS: usize = 300;

/// What the inboxes keep on the hub.
#[derive(Default)]
pub(crate) struct Inbox {
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    /// The requests each agent owes an answer to, oldest first
    owed: HashMap<AgentId, Vec<Owed>>,
    /// Requests each thread has carried
    requests: HashMap<String, u32>,
    /// The subject each two agents last wrote about, whichever wrote
    /// ([`pair`])
    subjects: HashMap<(AgentId, AgentId), String>,
    /// Each agent's last reply: whether it finished, and what it said
    reports: HashMap<AgentId, (bool, String)>,
}

/// A request or assignment an agent owes an answer to.
#[derive(Debug, Clone)]
struct Owed {
    id: String,
    kind: InboxKind,
    from: AgentId,
    thread: String,
    subject: String,
    /// The sender's todo it is for
    todo: Option<String>,
}

#[cfg(test)]
impl Inbox {
    /// How many answers `agent` owes.
    pub(crate) fn owed_by(&self, agent: AgentId) -> usize {
        lock(&self.state).owed.get(&agent).map_or(0, Vec::len)
    }
}

/// An agent as the inbox names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Named {
    pub agent_id: AgentId,
    pub code: Option<String>,
    pub name: String,
    pub definition: Option<String>,
}

impl Named {
    fn of(agent: &RegisteredAgent) -> Self {
        Self {
            agent_id: agent.agent_id(),
            code: agent.code.clone(),
            name: agent.name.clone(),
            definition: agent.definition.clone(),
        }
    }

    /// Its code, else its name.
    fn short(&self) -> &str {
        self.code.as_deref().unwrap_or(&self.name)
    }

    /// `REN4 (rust-engineer #4)`, or its name when it has no code.
    fn full(&self) -> String {
        match &self.code {
            Some(code) => format!("{code} ({})", self.name),
            None => self.name.clone(),
        }
    }

    /// Whether it answers requests on its own: every agent but a
    /// coordinator, whose words go to its channel.
    fn answers(&self) -> bool {
        self.definition.as_deref() != Some(COORDINATOR_DEFINITION)
    }
}

/// How a peer is related to the agent reaching it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Relation {
    /// The coordinator of the agent's channel
    Coordinator,
    /// Another member of the agent's channel
    Member,
    /// A top-level agent of the agent's project
    Project,
}

impl Relation {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Coordinator => "coordinator",
            Self::Member => "member",
            Self::Project => "project",
        }
    }
}

/// An agent another can reach through its inbox.
#[derive(Debug, Clone)]
pub(crate) struct Peer {
    pub agent: Named,
    pub relation: Relation,
}

/// Who an agent is, the channel it is in and the peers it can reach.
#[derive(Debug, Clone)]
pub(crate) struct Circle {
    pub me: Named,
    /// The slug of the agent's channel, and whether it coordinates it
    pub channel: Option<(String, bool)>,
    pub peers: Vec<Peer>,
}

impl Circle {
    /// Whether the agent coordinates its channel.
    pub(crate) fn coordinates(&self) -> bool {
        matches!(self.channel, Some((_, true)))
    }

    /// The peer `target` names: by code, hex id or name (any case), with
    /// `coordinator` naming the channel's coordinator.
    pub(crate) fn find(&self, target: &str) -> Option<&Peer> {
        let target = target.trim().trim_start_matches('@');
        if target.eq_ignore_ascii_case("coordinator") {
            return self.coordinator();
        }
        let hex = target.to_ascii_lowercase();
        self.peers.iter().find(|peer| {
            peer.agent
                .code
                .as_deref()
                .is_some_and(|code| code.eq_ignore_ascii_case(target))
                || peer.agent.agent_id.to_hex() == hex
                || peer.agent.name.eq_ignore_ascii_case(target)
        })
    }

    fn coordinator(&self) -> Option<&Peer> {
        self.peers
            .iter()
            .find(|peer| peer.relation == Relation::Coordinator)
    }

    fn slug(&self) -> &str {
        self.channel.as_ref().map_or("", |(slug, _)| slug.as_str())
    }
}

/// The circle of `caller`; `None` for an agent the registry doesn't have
/// or another agent spawned. Reads the channel store synchronously: on
/// the async runtime use [`circle`].
pub(crate) fn circle_of(hub: &AgentHub, caller: AgentId) -> Option<Circle> {
    let channel = read_channel(hub, caller);
    let registry = lock(&hub.registry);
    let me = registry.find(caller)?;
    if !registry.is_top_level_agent(caller) {
        return None;
    }
    let hidden = lock(&hub.hidden_members);
    let mut peers: Vec<Peer> = Vec::new();
    let mut slot = None;
    if let Some((channel, members)) = &channel {
        let coordinates = channel.coordinator == caller;
        slot = Some((channel.slug.clone(), coordinates));
        for member in members {
            if member.agent_id == caller || member.role == MemberRole::Subagent {
                continue;
            }
            let Some(agent) = registry.find(member.agent_id) else {
                continue;
            };
            if agent.archived || !registry.is_top_level_agent(member.agent_id) {
                continue;
            }
            let relation = if member.agent_id == channel.coordinator {
                Relation::Coordinator
            } else {
                Relation::Member
            };
            peers.push(Peer {
                agent: Named::of(agent),
                relation,
            });
        }
        if coordinates {
            return Some(Circle {
                me: Named::of(me),
                channel: slot,
                peers,
            });
        }
    }
    if let Some(project) = me
        .project_id
        .as_deref()
        .filter(|_| !hidden.contains(&caller))
    {
        for agent in registry.agents() {
            let id = agent.agent_id();
            let reachable = id != caller
                && !agent.archived
                && !agent.removed
                && agent.project_id.as_deref() == Some(project)
                && agent.definition.as_deref() != Some(COORDINATOR_DEFINITION)
                && !agent.is_compare()
                && !agent.is_compare_arm()
                && !hidden.contains(&id)
                && registry.is_top_level_agent(id)
                && !peers.iter().any(|peer| peer.agent.agent_id == id);
            if reachable {
                peers.push(Peer {
                    agent: Named::of(agent),
                    relation: Relation::Project,
                });
            }
        }
    }
    Some(Circle {
        me: Named::of(me),
        channel: slot,
        peers,
    })
}

/// [`circle_of`] off the async runtime.
pub(crate) async fn circle(hub: &Arc<AgentHub>, caller: AgentId) -> Option<Circle> {
    let hub = Arc::clone(hub);
    match tokio::task::spawn_blocking(move || circle_of(&hub, caller)).await {
        Ok(circle) => circle,
        Err(error) => {
            warn!(%error, "Could not look up an agent's peers");
            None
        }
    }
}

/// The channel `agent` is in, if it isn't archived, with its members;
/// `None` when it can't be read (logged).
fn read_channel(hub: &AgentHub, agent: AgentId) -> Option<(ChannelRecord, Vec<MemberRecord>)> {
    if !lock(&hub.channel_members).contains(&agent) {
        return None;
    }
    let store = hub.channels.as_ref()?;
    let read = || -> Result<_, aura_store_db::ChannelError> {
        let Some(channel_id) = store.channel_of(&agent)? else {
            return Ok(None);
        };
        let Some(channel) = store.get(&channel_id)?.filter(|c| !c.archived) else {
            return Ok(None);
        };
        let members = store.members(&channel.id)?;
        Ok(Some((channel, members)))
    };
    read().unwrap_or_else(|error| {
        warn!(agent = %agent.to_hex(), %error, "Could not read an agent's channel for its peers");
        None
    })
}

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
fn send(
    hub: &AgentHub,
    circle: &Circle,
    kind: InboxKind,
    to: &Peer,
    subject: Option<&str>,
    text: &str,
    todo: Option<&str>,
) -> Result<String, String> {
    let from = &circle.me;
    let recipient = &to.agent;
    let id = new_id();
    let answers = recipient.answers();
    let (subject, thread) = {
        let mut state = lock(&hub.inbox.state);
        let pair = pair(from.agent_id, recipient.agent_id);
        let subject = subject
            .map(str::trim)
            .filter(|subject| !subject.is_empty())
            .map(str::to_string)
            .or_else(|| {
                (kind != InboxKind::Assignment)
                    .then(|| state.subjects.get(&pair).cloned())
                    .flatten()
            })
            .unwrap_or_else(|| subject_of(text));
        let thread = thread_id(from.agent_id, recipient.agent_id, &subject);
        if kind == InboxKind::Request {
            let carried = state.requests.entry(thread.clone()).or_default();
            if *carried >= THREAD_REQUESTS {
                return Err(thread_full(circle, to));
            }
            *carried += 1;
        }
        state.subjects.insert(pair, subject.clone());
        if answers {
            state
                .owed
                .entry(recipient.agent_id)
                .or_default()
                .push(Owed {
                    id: id.clone(),
                    kind,
                    from: from.agent_id,
                    thread: thread.clone(),
                    subject: subject.clone(),
                    todo: todo.map(str::to_string),
                });
        }
        (subject, thread)
    };
    let header = inbox::header(kind, &id, from.short(), &from.name, &subject);
    let mut prompt = format!("{header}\n\n{}", text.trim());
    if let Some(footer) = footer(kind, from, answers) {
        prompt.push_str("\n\n");
        prompt.push_str(&footer);
    }
    hub.post_mail(mail_view(kind, &thread, &subject, (from, recipient), text));
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
    hub.notices
        .send(notice)
        .map_err(|_| "the terminal is shutting down".to_string())?;
    Ok(subject)
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
fn shown(header: &str) -> String {
    format!("✉ {}", header.lines().next().unwrap_or_default())
}

/// The INBOX tab's view of a message of `kind` in `thread`.
fn mail_view(
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
fn thread_id(a: AgentId, b: AgentId, subject: &str) -> String {
    let (low, high) = pair(a, b);
    let key = format!("{}:{}:{subject}", low.to_hex(), high.to_hex());
    let hash = blake3::hash(key.as_bytes()).to_hex();
    format!("inbox:{}", &hash.as_str()[..16])
}

/// A new message id, unique enough to find a message in a conversation.
fn new_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..12].to_string()
}

/// `text` trimmed and cut to `max` characters, marked when cut.
fn capped(text: &str, max: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max {
        return text.to_string();
    }
    let cut: String = text.chars().take(max).collect();
    format!("{cut}… [cut at {max} characters]")
}

/// Give the inbox message `notice` of `kind` to its agent. An assignment
/// is a new task, waiting while the agent is busy. Other messages go into
/// the agent's running turn if it has one that still takes input, with a
/// line in its chat; else they start a turn that carries on with its
/// latest task.
pub(super) async fn deliver(state: &mut LoopState<'_>, notice: Notice, kind: InboxKind) {
    let Notice {
        to, shown, prompt, ..
    } = notice;
    info!(agent = %to, %shown, "Delivering an inbox message");
    if kind != InboxKind::Assignment {
        if let Some(steering) = state.hub.steering(to) {
            if steering
                .send(SteerSource::Harness, prompt.clone(), prompt.clone())
                .await
            {
                AgentUi::new(state.commands.clone(), to)
                    .send(system_message(shown))
                    .await;
                return;
            }
        }
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

/// After `agent_id`'s work ended well: answer what it was asked
/// ([`answer_seen`]).
pub(super) fn answer(state: &LoopState<'_>, agent_id: AgentId) {
    if let Some(session) = state.idle.get(&agent_id) {
        answer_seen(&state.hub, agent_id, &session.messages);
    }
}

/// After `agent_id`'s work failed or was stopped, `why`: tell the senders
/// of what it was asked that no answer is coming ([`give_up_seen`]).
pub(super) fn give_up(state: &LoopState<'_>, agent_id: AgentId, why: &str) {
    let messages = state
        .idle
        .get(&agent_id)
        .map(|session| session.messages.as_slice());
    give_up_seen(&state.hub, agent_id, messages, why);
}

/// Answer each request `agent_id` owes whose message is in `messages`,
/// its conversation after a turn that ended well, with the last thing it
/// said after it. Requests whose messages it hasn't seen yet stay owed.
pub(crate) fn answer_seen(hub: &AgentHub, agent_id: AgentId, messages: &[Message]) {
    for (owed, at) in take_seen(hub, agent_id, messages) {
        let said = messages
            .get(at + 1..)
            .and_then(last_words)
            .unwrap_or_else(|| "(finished without a word)".to_string());
        reply(hub, agent_id, &owed, true, &said);
    }
}

/// Tell the senders of the requests `agent_id` owes that no answer is
/// coming, its work having stopped `why`: those whose messages are in
/// `messages`, its conversation, else (without one) the oldest, which
/// was the one that ran.
pub(crate) fn give_up_seen(
    hub: &AgentHub,
    agent_id: AgentId,
    messages: Option<&[Message]>,
    why: &str,
) {
    let dropped: Vec<Owed> = match messages {
        Some(messages) => take_seen(hub, agent_id, messages)
            .into_iter()
            .map(|(owed, _)| owed)
            .collect(),
        None => take_oldest(hub, agent_id).into_iter().collect(),
    };
    for owed in dropped {
        let why = capped(why, STOPPED_CHARS);
        reply(
            hub,
            agent_id,
            &owed,
            false,
            &format!("Stopped before finishing: {why}"),
        );
    }
}

/// Take the requests `agent_id` owes whose messages are in `messages`,
/// each with where its message is.
fn take_seen(hub: &AgentHub, agent_id: AgentId, messages: &[Message]) -> Vec<(Owed, usize)> {
    let mut state = lock(&hub.inbox.state);
    let Some(owed) = state.owed.get_mut(&agent_id) else {
        return Vec::new();
    };
    let mut seen = Vec::new();
    owed.retain(
        |request| match asked_at(messages, &inbox::marker(request.kind, &request.id)) {
            Some(at) => {
                seen.push((request.clone(), at));
                false
            }
            None => true,
        },
    );
    if owed.is_empty() {
        state.owed.remove(&agent_id);
    }
    seen
}

/// Take the oldest request `agent_id` owes.
fn take_oldest(hub: &AgentHub, agent_id: AgentId) -> Option<Owed> {
    let mut state = lock(&hub.inbox.state);
    let owed = state.owed.get_mut(&agent_id)?;
    let oldest = (!owed.is_empty()).then(|| owed.remove(0));
    if owed.is_empty() {
        state.owed.remove(&agent_id);
    }
    oldest
}

/// Where the user message carrying `marker` is in `messages`.
fn asked_at(messages: &[Message], marker: &str) -> Option<usize> {
    messages
        .iter()
        .rposition(|m| m.role == Role::User && m.text_content().contains(marker))
}

/// The last thing the model said in `messages`, if anything.
fn last_words(messages: &[Message]) -> Option<String> {
    messages
        .iter()
        .rev()
        .filter(|m| m.role == Role::Assistant)
        .map(Message::text_content)
        .find(|text| !text.trim().is_empty())
}

/// Send `agent_id`'s answer `said` to `owed`, `ok` when it finished the
/// work, closing the sender's todo when it named one.
fn reply(hub: &AgentHub, agent_id: AgentId, owed: &Owed, ok: bool, said: &str) {
    let (from, to) = {
        let registry = lock(&hub.registry);
        (
            registry.find(agent_id).map(Named::of),
            registry.find(owed.from).map(Named::of),
        )
    };
    let (Some(from), Some(to)) = (from, to) else {
        warn!(agent = %agent_id.to_hex(), "An inbox reply's agents are gone; not sent");
        return;
    };
    let id = new_id();
    let header = inbox::header(
        InboxKind::Reply,
        &id,
        from.short(),
        &from.name,
        &owed.subject,
    );
    let mut prompt = format!("{header}\n\n{}", capped(said, REPLY_CHARS));
    if let Some(todo) = &owed.todo {
        prompt.push('\n');
        prompt.push_str(&task_report_line(todo, ok));
    }
    lock(&hub.inbox.state)
        .reports
        .insert(agent_id, (ok, capped(said, REPORT_CHARS)));
    hub.post_mail(mail_view(
        InboxKind::Reply,
        &owed.thread,
        &owed.subject,
        (&from, &to),
        said,
    ));
    let notice = Notice {
        to: owed.from,
        shown: shown(&header),
        prompt,
        inbox: Some(InboxKind::Reply),
    };
    if hub.notices.send(notice).is_err() {
        warn!(agent = %agent_id.to_hex(), "Inbox reply after the event loop ended; dropped");
    }
}

/// What `list_agents` and `get_agent_state` say about `peer`.
fn peer_row(hub: &AgentHub, peer: &Peer) -> serde_json::Value {
    let (owes, report) = {
        let state = lock(&hub.inbox.state);
        (
            state.owed.get(&peer.agent.agent_id).map_or(0, Vec::len),
            state.reports.get(&peer.agent.agent_id).cloned(),
        )
    };
    let mut row = serde_json::json!({
        "agent_id": peer.agent.agent_id.to_hex(),
        "code": peer.agent.code,
        "name": peer.agent.name,
        "definition": peer.agent.definition,
        "relation": peer.relation.as_str(),
        "working": hub.is_running(peer.agent.agent_id),
        "messages_to_answer": owes,
    });
    if let Some((finished, text)) = report {
        row["last_report"] = serde_json::json!({ "finished": finished, "text": text });
    }
    row
}

fn unknown_agent(target: &str) -> String {
    format!("unknown agent '{target}'; call list_agents to see the agents you can reach")
}

/// The hub, for the tools of kernels built before it exists; set once it
/// does ([`Self::set`]).
#[derive(Clone, Default)]
pub struct HubLink(Arc<OnceLock<Weak<AgentHub>>>);

impl HubLink {
    /// Point the link at `hub`. Only the first call counts.
    pub fn set(&self, hub: &Arc<AgentHub>) {
        let _ = self.0.set(Arc::downgrade(hub));
    }

    /// The hub, unless it isn't set yet or is gone.
    pub(crate) fn hub(&self) -> Option<Arc<AgentHub>> {
        self.0.get()?.upgrade()
    }
}

/// An agent's `send_to_agent`, `list_agents` and `get_agent_state`: they
/// reach its peers through their inboxes, and hand anything else to the
/// background agents' hooks when those run.
pub struct PeerHooks {
    link: HubLink,
    caller: AgentId,
    host: Option<Arc<AgentHooks>>,
}

impl PeerHooks {
    /// The hooks of `caller`'s tools, falling back to `host`.
    #[must_use]
    pub fn new(link: HubLink, caller: AgentId, host: Option<Arc<AgentHooks>>) -> Self {
        Self { link, caller, host }
    }

    async fn circle(&self) -> Option<(Arc<AgentHub>, Circle)> {
        let hub = self.link.hub()?;
        let circle = circle(&hub, self.caller).await?;
        Some((hub, circle))
    }

    /// Send `text` to the peer `target` names, about `subject`. `None`
    /// when `target` names no peer.
    async fn send_to_peer(
        &self,
        target: &str,
        text: &str,
        subject: Option<&str>,
    ) -> Option<Result<(), String>> {
        let (hub, circle) = self.circle().await?;
        let peer = circle.find(target)?;
        Some(send(&hub, &circle, InboxKind::Request, peer, subject, text, None).map(|_| ()))
    }

    fn host(&self, target: &str) -> Result<&AgentHooks, String> {
        self.host.as_deref().ok_or_else(|| unknown_agent(target))
    }
}

#[async_trait]
impl AgentControlHook for PeerHooks {
    async fn deliver_message(
        &self,
        target_agent_id: &str,
        parent_agent_id: Option<&str>,
        originating_user_id: Option<&str>,
        project_id: Option<&str>,
        content: &str,
        attachments: Option<serde_json::Value>,
        model: Option<&str>,
    ) -> Result<(), String> {
        self.deliver_message_in_thread(
            target_agent_id,
            parent_agent_id,
            originating_user_id,
            project_id,
            content,
            attachments,
            model,
            None,
        )
        .await
    }

    async fn deliver_message_in_thread(
        &self,
        target_agent_id: &str,
        parent_agent_id: Option<&str>,
        originating_user_id: Option<&str>,
        project_id: Option<&str>,
        content: &str,
        attachments: Option<serde_json::Value>,
        model: Option<&str>,
        subject: Option<&str>,
    ) -> Result<(), String> {
        if let Some(sent) = self.send_to_peer(target_agent_id, content, subject).await {
            return sent;
        }
        self.host(target_agent_id)?
            .deliver_message_in_thread(
                target_agent_id,
                parent_agent_id,
                originating_user_id,
                project_id,
                content,
                attachments,
                model,
                subject,
            )
            .await
    }

    async fn lifecycle(
        &self,
        target_agent_id: &str,
        parent_agent_id: Option<&str>,
        originating_user_id: Option<&str>,
        action: &str,
    ) -> Result<(), String> {
        self.host(target_agent_id)?
            .lifecycle(
                target_agent_id,
                parent_agent_id,
                originating_user_id,
                action,
            )
            .await
    }

    async fn delegate_task(
        &self,
        target_agent_id: &str,
        parent_agent_id: Option<&str>,
        originating_user_id: Option<&str>,
        task: &str,
        context: Option<&serde_json::Value>,
        model: Option<&str>,
    ) -> Result<(), String> {
        let text = match context {
            Some(context) => format!("{task}\n\nContext: {context}"),
            None => task.to_string(),
        };
        if let Some(sent) = self.send_to_peer(target_agent_id, &text, None).await {
            return sent;
        }
        self.host(target_agent_id)?
            .delegate_task(
                target_agent_id,
                parent_agent_id,
                originating_user_id,
                task,
                context,
                model,
            )
            .await
    }
}

#[async_trait]
impl AgentReadHook for PeerHooks {
    async fn snapshot(&self, target_agent_id: &str) -> Result<serde_json::Value, String> {
        if let Some((hub, circle)) = self.circle().await {
            if let Some(peer) = circle.find(target_agent_id) {
                return Ok(peer_row(&hub, peer));
            }
        }
        self.host(target_agent_id)?.snapshot(target_agent_id).await
    }

    async fn list_agents(&self, org_id: Option<&str>) -> Result<serde_json::Value, String> {
        let mut agents = Vec::new();
        if let Some((hub, circle)) = self.circle().await {
            if self.host.is_none() {
                agents.push(serde_json::json!({
                    "agent_id": circle.me.agent_id.to_hex(),
                    "code": circle.me.code,
                    "name": circle.me.name,
                    "is_you": true,
                }));
            }
            agents.extend(circle.peers.iter().map(|peer| peer_row(&hub, peer)));
        }
        if let Some(host) = &self.host {
            match host.list_agents(org_id).await? {
                serde_json::Value::Array(more) => agents.extend(more),
                other => agents.push(other),
            }
        }
        Ok(serde_json::Value::Array(agents))
    }
}

/// What `assign` is called with.
#[derive(Debug, Deserialize)]
pub(crate) struct AssignArgs {
    /// Code of the member to give the work to
    #[serde(default)]
    pub to: Option<String>,
    /// Permanent agent to start a new member from
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub subject: String,
    pub prompt: String,
    #[serde(default)]
    pub todo_id: Option<String>,
}

/// `assign`: how a channel's coordinator hands work to the channel's
/// members ([`assign`]).
pub struct AssignTool {
    link: HubLink,
}

impl AssignTool {
    #[must_use]
    pub fn new(link: HubLink) -> Self {
        Self { link }
    }
}

#[async_trait]
impl Tool for AssignTool {
    fn name(&self) -> &str {
        ASSIGN_TOOL_NAME
    }

    fn definition(&self) -> ToolDefinition {
        // Turns offer the schema listing the current permanent agents.
        self.link
            .hub()
            .and_then(|hub| assign_tool_definition(&lock(&hub.definitions)))
            .unwrap_or_else(|| {
                ToolDefinition::new(
                    ASSIGN_TOOL_NAME,
                    "Hand a piece of the channel's work to a member.",
                    serde_json::json!({"type": "object"}),
                )
            })
    }

    async fn execute(
        &self,
        ctx: &ToolContext,
        args: serde_json::Value,
    ) -> Result<ToolResult, ToolError> {
        let args: AssignArgs = serde_json::from_value(args)
            .map_err(|e| ToolError::InvalidArguments(format!("assign: {e}")))?;
        let (Some(caller), Some(hub)) = (ctx.caller_agent_id, self.link.hub()) else {
            return Ok(ToolResult::failure(
                ASSIGN_TOOL_NAME,
                "assign: only a channel's coordinator assigns work",
            ));
        };
        Ok(match assign(&hub, caller, args).await {
            Ok(said) => ToolResult::success(ASSIGN_TOOL_NAME, said),
            Err(said) => ToolResult::failure(ASSIGN_TOOL_NAME, said),
        })
    }
}

/// Hand the work `args` describe from the coordinator `caller` to a member
/// of its channel: the one `to` names, else a new one started from the
/// permanent agent `agent`. Returns what the coordinator is told.
///
/// # Errors
/// What the coordinator is told when nothing was assigned.
pub(crate) async fn assign(
    hub: &Arc<AgentHub>,
    caller: AgentId,
    args: AssignArgs,
) -> Result<String, String> {
    let circle = circle(hub, caller)
        .await
        .filter(Circle::coordinates)
        .ok_or_else(|| "assign: only a channel's coordinator assigns work".to_string())?;
    let brief = args.prompt.trim();
    if brief.is_empty() {
        return Err(
            "assign: the prompt is empty; brief the goal, the context and what done looks like"
                .to_string(),
        );
    }
    let named = |field: &Option<String>| -> Option<String> {
        field
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    let (peer, new) = match (named(&args.to), named(&args.agent)) {
        (Some(to), _) => {
            let peer = circle
                .find(&to)
                .filter(|peer| peer.relation == Relation::Member)
                .cloned()
                .ok_or_else(|| no_member(&circle, &to))?;
            (peer, false)
        }
        (None, Some(definition)) => (start_member(hub, caller, &definition).await?, true),
        (None, None) => {
            return Err(
                "assign: name a member in `to`, or a permanent agent in `agent` \
                        to start a new member"
                    .to_string(),
            )
        }
    };
    let subject = Some(args.subject.as_str()).filter(|subject| !subject.trim().is_empty());
    send(
        hub,
        &circle,
        InboxKind::Assignment,
        &peer,
        subject,
        brief,
        args.todo_id.as_deref(),
    )?;
    let status = if new {
        "new member"
    } else if hub.is_running(peer.agent.agent_id) {
        "busy: it starts when free"
    } else {
        "starting now"
    };
    Ok(format!(
        "Assigned to {} ({status}). Its report will arrive in your inbox.",
        peer.agent.full()
    ))
}

/// What the coordinator of `circle` is told when `to` names no member.
fn no_member(circle: &Circle, to: &str) -> String {
    let members: Vec<String> = circle
        .peers
        .iter()
        .filter(|peer| peer.relation == Relation::Member)
        .map(|peer| peer.agent.full())
        .collect();
    let slug = circle.slug();
    if members.is_empty() {
        format!(
            "assign: #{slug} has no member `{to}`, nor any other yet; name a permanent agent in \
             `agent` to start one"
        )
    } else {
        format!(
            "assign: #{slug} has no member `{to}`. Members: {}. Or name a permanent agent in \
             `agent` to start a new one.",
            members.join(", ")
        )
    }
}

/// Start a new member of the channel `coordinator` runs from the
/// permanent agent `definition`, and show it in the agent lists.
async fn start_member(
    hub: &Arc<AgentHub>,
    coordinator: AgentId,
    definition: &str,
) -> Result<Peer, String> {
    let definition = match lock(&hub.definitions).get(definition) {
        Some(found) if !found.is_coordinator() => found.name.clone(),
        Some(_) => {
            return Err(
                "assign: coordinators only run channels; pick another permanent agent".to_string(),
            )
        }
        None => {
            return Err(format!(
                "assign: there is no permanent agent `{definition}`"
            ))
        }
    };
    let adding = Arc::clone(hub);
    let added =
        tokio::task::spawn_blocking(move || channels::add_agent(&adding, coordinator, &definition))
            .await;
    let agent = match added {
        Ok(Ok(agent)) => agent,
        Ok(Err(error)) => return Err(format!("assign: could not start a new member: {error:#}")),
        Err(error) => return Err(format!("assign: could not start a new member: {error}")),
    };
    hub.send_agent_list().await;
    Ok(Peer {
        agent: Named::of(&agent),
        relation: Relation::Member,
    })
}

#[cfg(test)]
mod tests;
