//! [`AgentHost`]: the table of live agents and the code that spawns and
//! runs them.

mod lifecycle;
mod messaging;
mod outcome;
mod run;
mod spawn;
mod task;
mod types;

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use aura_agent::{AgentLoopConfig, AgentLoopEvent};
use aura_agent_kernel::{write_system_entry, ExecutorRouter, PolicyConfig, RecordObserver};
use aura_agent_subagent::{DefaultDerivation, SubagentDerivationConfig, SubagentRegistry};
use aura_core_types::{
    AgentId, AgentMode, AgentPermissions, AgentToolPermissions, SubagentResult, Transaction,
    TransactionType,
};
use aura_model::{Message, ToolDefinition};
use aura_store_db::Store;
use bytes::Bytes;
use dashmap::DashMap;
use parking_lot::Mutex;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use tracing::warn;

use super::hooks::AgentHooks;
use super::inbox::{InboxMessage, MessageKind, ParentInbox};
use super::mail::{MailObserver, ThreadId, Threads};
use super::observer::TurnObserver;
use super::remote::{RemoteMail, RemoteRouter};
use crate::scheduler::Scheduler;

pub use types::{AgentEnv, AgentHostConfig, AgentInfo, AgentState, LifecycleAction, RootAgent};

/// Everything fixed about how a hosted agent runs its turns.
struct RunSetup {
    loop_config: AgentLoopConfig,
    policy: PolicyConfig,
    router: ExecutorRouter,
    tools: Vec<ToolDefinition>,
    /// Per-turn time limit. `None` lets a turn run until it finishes or
    /// the agent is stopped.
    timeout: Option<Duration>,
}

/// A message waiting in a hosted agent's queue.
struct Envelope {
    from: Option<AgentId>,
    from_name: String,
    kind: MessageKind,
    text: String,
    /// Conversation the message belongs to; the turn's reply stays in it.
    thread: ThreadId,
    /// Subject of `thread`.
    subject: String,
    /// Whether this message opened `thread` and its subject was made from
    /// the message's text; see [`MailMessage::subject_from_text`].
    subject_from_text: bool,
    /// Receives the turn's result instead of routing it as a reply.
    waiter: Option<oneshot::Sender<SubagentResult>>,
    /// Streams the turn's events to an observer.
    events: Option<mpsc::Sender<AgentLoopEvent>>,
    /// Cancels just this turn.
    cancel: Option<CancellationToken>,
}

/// Which thread a new message goes in.
enum ThreadChoice<'a> {
    /// Join the thread with this subject, or with none, continue the last
    /// thread between sender and recipient.
    Resolve(Option<&'a str>),
    /// Start a new thread named after the message.
    New,
    /// Stay in a known thread with its subject.
    Existing(ThreadId, String),
}

enum Body {
    Root {
        inbox: Option<Arc<dyn ParentInbox>>,
    },
    Hosted {
        setup: Arc<RunSetup>,
        history: Vec<Message>,
        queue: VecDeque<Envelope>,
    },
}

struct AgentEntry {
    name: String,
    role: String,
    /// Ancestors, nearest first.
    chain: Vec<AgentId>,
    permissions: AgentPermissions,
    tool_permissions: Option<AgentToolPermissions>,
    mode: AgentMode,
    model: String,
    env: AgentEnv,
    /// Cancelled when this agent or any ancestor is stopped.
    stop: CancellationToken,
    /// A `task` child: removed, with its subtree, once its job is done
    /// and nothing else is queued for it.
    one_shot: bool,
    running: bool,
    paused: bool,
    turns: u32,
    last_reply: Option<String>,
    /// Thread and subject of the last request this agent took from its
    /// parent. An answer to a child's reply goes up to the parent in it.
    parent_thread: Option<(ThreadId, String)>,
    body: Body,
}

impl AgentEntry {
    fn parent(&self) -> Option<AgentId> {
        self.chain.first().copied()
    }

    fn root(&self, own_id: AgentId) -> AgentId {
        self.chain.last().copied().unwrap_or(own_id)
    }

    fn state(&self) -> AgentState {
        if matches!(self.body, Body::Root { .. }) {
            AgentState::Root
        } else if self.running {
            AgentState::Running
        } else if self.paused {
            AgentState::Paused
        } else {
            AgentState::Idle
        }
    }

    fn info(&self, agent_id: AgentId) -> AgentInfo {
        AgentInfo {
            agent_id,
            name: self.name.clone(),
            role: self.role.clone(),
            parent: self.parent(),
            depth: self.chain.len(),
            state: self.state(),
            model: self.model.clone(),
            turns: self.turns,
            queued: match &self.body {
                Body::Hosted { queue, .. } => queue.len(),
                Body::Root { .. } => 0,
            },
            last_reply: self.last_reply.clone(),
        }
    }
}

/// Runs agents that other agents spawn.
///
/// One host serves a whole process. Surfaces register their top-level
/// agents with [`Self::register_root`] and attach [`Self::hooks_for`] to
/// each agent's tool resolver; every child gets the same hooks, so
/// children can spawn and message agents exactly like their parents.
#[derive(Clone)]
pub struct AgentHost {
    pub(super) inner: Arc<HostInner>,
}

pub(super) struct HostInner {
    store: Arc<dyn Store>,
    scheduler: Arc<Scheduler>,
    kinds: SubagentRegistry,
    derivation: DefaultDerivation,
    config: AgentHostConfig,
    agents: Mutex<HashMap<AgentId, AgentEntry>>,
    /// Serialises writes to one parent's record log from parallel spawns.
    parent_locks: DashMap<AgentId, Arc<tokio::sync::Mutex<()>>>,
    observer: Mutex<Option<Arc<dyn TurnObserver>>>,
    /// Which thread each message between agents goes in.
    threads: Mutex<Threads>,
    mail_observer: Mutex<Option<Arc<dyn MailObserver>>>,
    /// Carries mail to agents other nodes hold.
    remote: Mutex<Option<Arc<dyn RemoteRouter>>>,
}

impl AgentHost {
    #[must_use]
    pub fn new(store: Arc<dyn Store>, scheduler: Arc<Scheduler>, config: AgentHostConfig) -> Self {
        let derivation = DefaultDerivation::new(SubagentDerivationConfig {
            max_depth: config.max_depth,
            ..SubagentDerivationConfig::default()
        });
        Self {
            inner: Arc::new(HostInner {
                store,
                scheduler,
                kinds: SubagentRegistry::bundled(),
                derivation,
                config,
                agents: Mutex::new(HashMap::new()),
                parent_locks: DashMap::new(),
                observer: Mutex::new(None),
                threads: Mutex::new(Threads::default()),
                mail_observer: Mutex::new(None),
                remote: Mutex::new(None),
            }),
        }
    }

    /// Tell `observer` about every hosted agent's turns from now on.
    pub fn set_turn_observer(&self, observer: Arc<dyn TurnObserver>) {
        *self.inner.observer.lock() = Some(observer);
    }

    /// Show `observer` every message between agents from now on,
    /// including the results of `task` children returned to a waiting
    /// parent. Messages from the user are not mail and are not shown.
    pub fn set_mail_observer(&self, observer: Arc<dyn MailObserver>) {
        *self.inner.mail_observer.lock() = Some(observer);
    }

    /// Send mail for agents this host does not hold through `router`
    /// from now on.
    pub fn set_remote_router(&self, router: Arc<dyn RemoteRouter>) {
        *self.inner.remote.lock() = Some(router);
    }

    /// Hand `mail` from an agent on another node to the agent here it is
    /// for, in the thread it names.
    ///
    /// # Errors
    ///
    /// Fails when the recipient is unknown or stopped here.
    pub fn deliver_remote(&self, mail: RemoteMail) -> Result<(), String> {
        let envelope = Envelope {
            from: Some(mail.from),
            from_name: mail.from_name,
            kind: mail.kind,
            text: mail.text,
            thread: mail.thread,
            subject: mail.subject,
            // The sending node created the thread and marked its own mail;
            // asking again here would title the same thread twice.
            subject_from_text: false,
            waiter: None,
            events: None,
            cancel: None,
        };
        self.inner.enqueue(mail.to, envelope)
    }

    /// Register or refresh a top-level agent. Its existing children are
    /// kept; they keep the environment they were spawned with.
    pub fn register_root(&self, root: RootAgent) {
        let mut agents = self.inner.agents.lock();
        let stop = agents
            .get(&root.agent_id)
            .map_or_else(CancellationToken::new, |existing| existing.stop.clone());
        let (turns, last_reply) = agents
            .get(&root.agent_id)
            .map_or((0, None), |e| (e.turns, e.last_reply.clone()));
        agents.insert(
            root.agent_id,
            AgentEntry {
                name: root.name,
                role: "root".to_string(),
                chain: Vec::new(),
                permissions: root.permissions,
                tool_permissions: root.tool_permissions,
                mode: root.mode,
                model: root.model,
                env: root.env,
                stop,
                one_shot: false,
                running: false,
                paused: false,
                turns,
                last_reply,
                parent_thread: None,
                body: Body::Root { inbox: root.inbox },
            },
        );
    }

    /// Hooks that act on behalf of `caller`.
    #[must_use]
    pub fn hooks_for(&self, caller: AgentId) -> AgentHooks {
        AgentHooks::new(Arc::downgrade(&self.inner), caller)
    }

    #[must_use]
    pub fn info(&self, agent_id: AgentId) -> Option<AgentInfo> {
        self.inner
            .agents
            .lock()
            .get(&agent_id)
            .map(|entry| entry.info(agent_id))
    }

    /// Every agent in `root`'s tree, the root first.
    #[must_use]
    pub fn tree(&self, root: AgentId) -> Vec<AgentInfo> {
        self.inner.tree(root)
    }

    /// Send `text` to `to` as if from `from` (the user when `None`).
    ///
    /// # Errors
    /// Fails when `to` is unknown or stopped.
    pub fn send(&self, from: Option<AgentId>, to: AgentId, text: String) -> Result<(), String> {
        self.inner
            .deliver_message(from, to, MessageKind::Request, text, None)
    }

    /// Apply `action` to `target` and its descendants.
    ///
    /// # Errors
    /// Fails when `target` is unknown or a root.
    pub fn lifecycle(&self, target: AgentId, action: LifecycleAction) -> Result<(), String> {
        self.inner.lifecycle(target, action)
    }

    /// Stop every agent under `root` and forget the root.
    pub fn remove_root(&self, root: AgentId) {
        let dropped = {
            let mut agents = self.inner.agents.lock();
            self.inner.remove_subtree(&mut agents, root, true)
        };
        self.inner.notify_dropped(dropped);
    }
}

impl HostInner {
    pub(super) fn tree(&self, root: AgentId) -> Vec<AgentInfo> {
        let agents = self.agents.lock();
        let mut out: Vec<AgentInfo> = agents
            .iter()
            .filter(|(id, entry)| entry.root(**id) == root)
            .map(|(id, entry)| entry.info(*id))
            .collect();
        out.sort_by_key(|info| (info.depth, info.name.clone()));
        out
    }

    pub(super) fn env_of(&self, agent_id: AgentId) -> Option<AgentEnv> {
        self.agents.lock().get(&agent_id).map(|e| e.env.clone())
    }

    pub(super) fn info(&self, agent_id: AgentId) -> Option<AgentInfo> {
        self.agents.lock().get(&agent_id).map(|e| e.info(agent_id))
    }

    pub(super) fn root_of(&self, agent_id: AgentId) -> Option<AgentId> {
        self.agents.lock().get(&agent_id).map(|e| e.root(agent_id))
    }

    /// Resolve `target` (full hex id, a hex prefix of 8+ characters, or a
    /// name) to an agent in `caller`'s tree.
    pub(super) fn resolve(&self, caller: AgentId, target: &str) -> Option<AgentId> {
        let target = target.trim();
        let agents = self.agents.lock();
        let root = agents.get(&caller)?.root(caller);
        let in_tree = |(id, entry): &(&AgentId, &AgentEntry)| entry.root(**id) == root;
        if let Ok(id) = AgentId::from_hex(target) {
            return agents.get(&id).filter(|e| e.root(id) == root).map(|_| id);
        }
        let lower = target.to_ascii_lowercase();
        if lower.len() >= 8 && lower.chars().all(|c| c.is_ascii_hexdigit()) {
            let mut matches = agents
                .iter()
                .filter(in_tree)
                .filter(|(id, _)| id.to_hex().starts_with(&lower));
            if let (Some((id, _)), None) = (matches.next(), matches.next()) {
                return Some(*id);
            }
        }
        let mut by_name = agents
            .iter()
            .filter(in_tree)
            .filter(|(_, entry)| entry.name == target);
        match (by_name.next(), by_name.next()) {
            (Some((id, _)), None) => Some(*id),
            _ => None,
        }
    }

    fn parent_lock(&self, parent: AgentId) -> Arc<tokio::sync::Mutex<()>> {
        self.parent_locks
            .entry(parent)
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }

    fn running_count(&self) -> usize {
        self.agents.lock().values().filter(|e| e.running).count()
    }
}

/// Append a message a root could not receive live to its record.
fn record_message(
    store: &Arc<dyn Store>,
    observer: Option<&Arc<dyn RecordObserver>>,
    message: &InboxMessage,
) {
    let payload = serde_json::json!({
        "kind": "agent_message",
        "from_agent_id": message.from,
        "from_name": message.from_name,
        "reply": message.kind == MessageKind::Reply,
        "subject": message.subject,
        "thread": message.thread.0,
        "text": message.text,
    });
    let tx = Transaction::new_chained(
        message.to,
        TransactionType::System,
        Bytes::from(payload.to_string().into_bytes()),
        None,
    );
    if let Err(err) = write_observed(store, observer, message.to, &tx) {
        warn!(root = %message.to, %err, "could not record agent message");
    }
}

/// Append a System-shaped entry and show it to `observer`, the way a
/// kernel does for its own appends.
fn write_observed(
    store: &Arc<dyn Store>,
    observer: Option<&Arc<dyn RecordObserver>>,
    agent_id: AgentId,
    tx: &Transaction,
) -> Result<u64, aura_agent_kernel::KernelError> {
    let entry = write_system_entry(store, agent_id, tx)?;
    if let Some(observer) = observer {
        observer.on_entry(&entry);
    }
    Ok(entry.seq)
}
