//! [`AgentHost`]: the table of live agents and the code that spawns and
//! runs them.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use aura_agent::{AgentLoopConfig, AgentLoopEvent};
use aura_agent_kernel::{write_system_record, ExecutorRouter, KernelSpawnHook, PolicyConfig};
use aura_agent_kernel::{ChildAgentSpec, SpawnHook, SpawnOutcome};
use aura_agent_subagent::{
    narrow_permissions, overrides_from_request, parent_context_from_request, DefaultDerivation,
    SubagentDerivation, SubagentDerivationConfig, SubagentRegistry,
};
use aura_core_types::{
    AgentId, AgentMode, AgentPermissions, AgentToolPermissions, SpawnMode, SubagentDispatchRequest,
    SubagentExit, SubagentResult, Transaction, TransactionType, UserToolDefaults,
};
use aura_model_reasoner::{Message, ToolDefinition};
use aura_store_db::Store;
use aura_tools::{AgentControlHook, AgentReadHook};
use bytes::Bytes;
use dashmap::DashMap;
use parking_lot::Mutex;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use super::factory::{ChildKernelFactory, ChildKernelRequest};
use super::hooks::AgentHooks;
use super::inbox::{InboxMessage, MessageKind, ParentInbox};
use super::setup;
use crate::scheduler::{ScheduleOverrides, Scheduler};

/// Limits for an [`AgentHost`].
#[derive(Debug, Clone)]
pub struct AgentHostConfig {
    /// Longest allowed chain of spawns below a root agent.
    pub max_depth: u32,
    /// Most child agents that may be running a turn at once. Spawns past
    /// this are rejected; messages to existing agents just queue.
    pub max_running: usize,
}

impl Default for AgentHostConfig {
    fn default() -> Self {
        Self {
            max_depth: 8,
            max_running: 32,
        }
    }
}

/// What every agent in one tree shares. Set when the root registers and
/// inherited by each child at spawn time.
#[derive(Clone)]
pub struct AgentEnv {
    /// Builds each child's tools.
    pub factory: Arc<dyn ChildKernelFactory>,
    /// Workspace root for child runs as `(workspace_base,
    /// use_workspace_base_as_root)`. `None` gives each child the
    /// scheduler's per-agent directory.
    pub workspace: Option<(PathBuf, bool)>,
    /// User-level tool defaults for the tree.
    pub user_tool_defaults: UserToolDefaults,
    /// End user at the root of the tree.
    pub originating_user_id: Option<String>,
    /// Replaces the local `spawn_agent` behaviour, e.g. to create agents
    /// in aura-os instead.
    pub spawn_override: Option<Arc<dyn SpawnHook>>,
    /// Handles messaging and lifecycle calls for agents outside the host.
    pub remote_control: Option<Arc<dyn AgentControlHook>>,
    /// Handles reads for agents outside the host.
    pub remote_read: Option<Arc<dyn AgentReadHook>>,
}

impl AgentEnv {
    /// An environment with no workspace override and no remote hooks.
    #[must_use]
    pub fn new(factory: Arc<dyn ChildKernelFactory>) -> Self {
        Self {
            factory,
            workspace: None,
            user_tool_defaults: UserToolDefaults::full_access(),
            originating_user_id: None,
            spawn_override: None,
            remote_control: None,
            remote_read: None,
        }
    }
}

/// A top-level agent whose turns a surface (chat run, terminal) drives.
pub struct RootAgent {
    pub agent_id: AgentId,
    pub name: String,
    pub permissions: AgentPermissions,
    pub tool_permissions: Option<AgentToolPermissions>,
    pub mode: AgentMode,
    /// Model children inherit when nothing else picks one.
    pub model: String,
    pub env: AgentEnv,
    /// Where replies to the root go. `None` appends them to its record.
    pub inbox: Option<Arc<dyn ParentInbox>>,
}

/// Lifecycle state of an agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentState {
    /// Driven by a surface, not the host.
    Root,
    /// Waiting for a message.
    Idle,
    /// Running a turn.
    Running,
    /// Holding messages until resumed.
    Paused,
}

impl AgentState {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Root => "root",
            Self::Idle => "idle",
            Self::Running => "running",
            Self::Paused => "paused",
        }
    }
}

/// Read-only view of one agent.
#[derive(Debug, Clone)]
pub struct AgentInfo {
    pub agent_id: AgentId,
    pub name: String,
    pub role: String,
    pub parent: Option<AgentId>,
    /// Number of ancestors.
    pub depth: usize,
    pub state: AgentState,
    pub model: String,
    /// Finished turns.
    pub turns: u32,
    /// Messages waiting to be handled.
    pub queued: usize,
    pub last_reply: Option<String>,
}

impl AgentInfo {
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "agent_id": self.agent_id.to_hex(),
            "name": self.name,
            "role": self.role,
            "parent_agent_id": self.parent.map(|p| p.to_hex()),
            "depth": self.depth,
            "state": self.state.as_str(),
            "model": self.model,
            "turns": self.turns,
            "queued": self.queued,
            "last_reply": self.last_reply,
        })
    }
}

/// Lifecycle actions accepted by [`AgentHost::lifecycle`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleAction {
    Pause,
    Resume,
    Stop,
    /// Clear the conversation and wake the agent up again.
    Restart,
}

impl LifecycleAction {
    /// Parse the `agent_lifecycle` tool's action string.
    #[must_use]
    pub fn parse(action: &str) -> Option<Self> {
        match action {
            "pause" => Some(Self::Pause),
            "resume" => Some(Self::Resume),
            "stop" => Some(Self::Stop),
            "restart" => Some(Self::Restart),
            _ => None,
        }
    }
}

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
    /// Receives the turn's result instead of routing it as a reply.
    waiter: Option<oneshot::Sender<SubagentResult>>,
    /// Streams the turn's events to an observer.
    events: Option<mpsc::Sender<AgentLoopEvent>>,
    /// Cancels just this turn.
    cancel: Option<CancellationToken>,
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

/// What a new child is, before it is set up.
struct ChildPlan {
    child_id: AgentId,
    parent: AgentId,
    name: String,
    role: String,
    permissions: AgentPermissions,
    tool_permissions: AgentToolPermissions,
    allowed_tools: Option<Vec<String>>,
    mode: AgentMode,
    model: String,
    system_prompt: String,
    reasoning_effort: Option<aura_model_reasoner::ThinkingEffort>,
    response_max_tokens: Option<u32>,
    kind: Option<aura_core_types::SubagentKindSpec>,
    iteration_cap: Option<u32>,
    token_cap: Option<u32>,
    timeout: Option<Duration>,
    policy: PolicyConfig,
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
            }),
        }
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
            .deliver_message(from, to, MessageKind::Request, text)
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
        notify_dropped(dropped);
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

    // ------------------------------------------------------------------
    // task
    // ------------------------------------------------------------------

    /// Run a `task`: spawn a child of the requested kind and give it the
    /// prompt. Waits for the result unless `spawn_mode` is detached.
    pub(super) async fn dispatch_task(
        self: &Arc<Self>,
        request: SubagentDispatchRequest,
        events: Option<mpsc::Sender<AgentLoopEvent>>,
        cancel: Option<CancellationToken>,
    ) -> SubagentResult {
        let parent = request.parent_agent_id;
        let Some(kind) = self.kinds.get(&request.subagent_type).cloned() else {
            return SubagentResult::rejected(format!(
                "unknown subagent type '{}'",
                request.subagent_type
            ));
        };
        let Some(parent_entry) = self.parent_snapshot(parent) else {
            return SubagentResult::rejected(format!(
                "agent {parent} is not registered with the agent host"
            ));
        };
        let parent_ctx = parent_context_from_request(&request);
        if !parent_ctx.mode.allows_spawn() {
            return SubagentResult::rejected(format!(
                "spawn rejected: {:?} mode does not allow spawning agents",
                parent_ctx.mode
            ));
        }
        let (spec, manifest) = match self
            .derivation
            .derive(&parent_ctx, overrides_from_request(&request, &kind))
        {
            Ok(derived) => derived,
            Err(err) => return SubagentResult::rejected(format!("spawn rejected: {err}")),
        };
        if spec.response_max_tokens == Some(0)
            || spec.budget.max_tokens == 0
            || spec.budget.max_iterations == 0
        {
            return SubagentResult::rejected("child execution limits must be positive");
        }
        let reasoning_effort = match spec.reasoning_effort.as_deref() {
            None => None,
            Some(effort) => match aura_model_reasoner::ThinkingEffort::from_wire(effort) {
                Some(effort) => Some(effort),
                None => {
                    return SubagentResult::rejected(format!("invalid reasoning effort: {effort}"))
                }
            },
        };
        if self.running_count() >= self.config.max_running {
            return SubagentResult::rejected(format!(
                "spawn rejected: {} agents are already running",
                self.config.max_running
            ));
        }

        let permissions = narrow_permissions(
            &AgentPermissions {
                scope: spec.permissions.scope.clone(),
                capabilities: spec.permissions.capabilities.clone(),
            },
            &kind,
        );
        let user_tool_defaults = spec
            .user_tool_defaults
            .clone()
            .unwrap_or_else(|| parent_entry.env.user_tool_defaults.clone());
        let allowed_tools = spec
            .tool_subset
            .clone()
            .unwrap_or_else(|| kind.allowed_tools.clone());
        let tool_permissions = setup::narrowed_tool_permissions(
            spec.parent_tool_permissions.as_ref(),
            &user_tool_defaults,
            &allowed_tools,
        );
        let model = if spec.model_id.is_empty() {
            kind.default_model
                .clone()
                .unwrap_or_else(|| aura_model_reasoner::ENV_FALLBACK_MODEL.to_string())
        } else {
            spec.model_id.clone()
        };
        let timeout_ms = request
            .override_budget
            .as_ref()
            .map_or(kind.budget.timeout_ms, |b| b.timeout_ms);
        let policy = setup::task_policy(
            permissions.clone(),
            tool_permissions.clone(),
            &user_tool_defaults,
            spec.mode,
        );
        let child_id = AgentId::generate();
        let detached = matches!(request.spawn_mode, Some(SpawnMode::Detached));

        let plan = ChildPlan {
            child_id,
            parent,
            name: kind.name.clone(),
            role: format!("subagent:{}", kind.name),
            permissions,
            tool_permissions,
            allowed_tools: Some(allowed_tools),
            mode: spec.mode,
            model,
            system_prompt: setup::kind_system_prompt(&kind, spec.system_prompt_addendum.as_deref()),
            reasoning_effort,
            response_max_tokens: spec.response_max_tokens,
            iteration_cap: Some(spec.budget.max_iterations),
            token_cap: Some(spec.budget.max_tokens),
            kind: Some(kind),
            timeout: Some(Duration::from_millis(timeout_ms)),
            policy,
        };

        let record = serde_json::json!({
            "parent_agent_id": parent,
            "child_agent_id": child_id,
            "override_manifest": manifest,
            "spawn_mode": request.spawn_mode.unwrap_or_default(),
        });
        if let Err(err) = self
            .create_child(
                plan,
                &parent_entry,
                Some(record),
                request.originating_user_id.as_deref(),
            )
            .await
        {
            return SubagentResult::rejected(format!("spawn failed: {err}"));
        }

        if detached {
            let envelope = self.envelope(
                Some(parent),
                MessageKind::Request,
                request.prompt,
                None,
                events,
                None,
            );
            if let Err(err) = self.enqueue(child_id, envelope) {
                return SubagentResult::rejected(err);
            }
            return SubagentResult::completed(
                child_id,
                format!(
                    "Started agent {child_id} in the background. Its result will be delivered \
                     to you as a message when it finishes."
                ),
            );
        }

        let (tx, rx) = oneshot::channel();
        let envelope = self.envelope(
            Some(parent),
            MessageKind::Request,
            request.prompt,
            Some(tx),
            events,
            cancel,
        );
        if let Err(err) = self.enqueue(child_id, envelope) {
            return SubagentResult::rejected(err);
        }
        rx.await.unwrap_or_else(|_| cancelled(child_id))
    }

    // ------------------------------------------------------------------
    // spawn_agent
    // ------------------------------------------------------------------

    /// Create a long-lived child for `spawn_agent`, optionally giving it a
    /// first message.
    pub(super) async fn spawn_agent(
        self: &Arc<Self>,
        parent: AgentId,
        originating_user_id: Option<&str>,
        child: ChildAgentSpec,
        initial_prompt: Option<String>,
    ) -> Result<SpawnOutcome, String> {
        let Some(parent_entry) = self.parent_snapshot(parent) else {
            return Err(format!(
                "agent {parent} is not registered with the agent host"
            ));
        };
        if !parent_entry.mode.allows_spawn() {
            return Err(format!(
                "{:?} mode does not allow spawning agents",
                parent_entry.mode
            ));
        }
        if !parent_entry.permissions.contains(&child.permissions) {
            return Err("a spawned agent cannot have more permissions than its parent".into());
        }
        let depth = parent_entry.chain.len() + 2;
        if depth > self.config.max_depth as usize {
            return Err(format!(
                "spawn depth {depth} exceeds the limit of {}",
                self.config.max_depth
            ));
        }
        if self.running_count() >= self.config.max_running {
            return Err(format!(
                "{} agents are already running",
                self.config.max_running
            ));
        }

        let child_id = child.preassigned_agent_id.unwrap_or_else(AgentId::generate);
        if self.agents.lock().contains_key(&child_id) {
            return Err(format!("agent {child_id} already exists"));
        }
        let tool_permissions = child
            .tool_permissions
            .clone()
            .or_else(|| parent_entry.tool_permissions.clone());
        let base_prompt = child.system_prompt_override.clone().unwrap_or_else(|| {
            self.scheduler
                .identity_registry()
                .get(parent)
                .map(|identity| identity.system_prompt)
                .unwrap_or_default()
        });
        let policy = setup::agent_policy(
            child.permissions.clone(),
            tool_permissions.clone(),
            &parent_entry.env.user_tool_defaults,
            parent_entry.mode,
        );
        let plan = ChildPlan {
            child_id,
            parent,
            name: child.name.clone(),
            role: child.role.clone(),
            permissions: child.permissions.clone(),
            tool_permissions: tool_permissions.unwrap_or_default(),
            allowed_tools: None,
            mode: parent_entry.mode,
            model: parent_entry.model.clone(),
            system_prompt: setup::spawned_agent_prompt(&base_prompt, &child.name, &child.role),
            reasoning_effort: None,
            response_max_tokens: None,
            kind: None,
            iteration_cap: None,
            token_cap: None,
            timeout: None,
            policy,
        };
        let outcome = self
            .create_child(plan, &parent_entry, None, originating_user_id)
            .await?;

        if let Some(prompt) = initial_prompt.filter(|p| !p.trim().is_empty()) {
            let envelope =
                self.envelope(Some(parent), MessageKind::Request, prompt, None, None, None);
            self.enqueue(child_id, envelope)?;
        }
        Ok(SpawnOutcome {
            child_agent_id: child_id,
            external_agent_id: Some(child_id.to_hex()),
            delegate_tx_hash: outcome.delegate_tx_hash,
        })
    }

    // ------------------------------------------------------------------
    // shared spawn path
    // ------------------------------------------------------------------

    /// Snapshot of what a child inherits from `parent`.
    fn parent_snapshot(&self, parent: AgentId) -> Option<ParentSnapshot> {
        let agents = self.agents.lock();
        let entry = agents.get(&parent)?;
        Some(ParentSnapshot {
            chain: entry.chain.clone(),
            env: entry.env.clone(),
            mode: entry.mode,
            model: entry.model.clone(),
            permissions: entry.permissions.clone(),
            tool_permissions: entry.tool_permissions.clone(),
            stop: entry.stop.clone(),
        })
    }

    /// Write the child's records, build its run setup, and add it to the
    /// table as idle.
    async fn create_child(
        self: &Arc<Self>,
        plan: ChildPlan,
        parent: &ParentSnapshot,
        spawn_record: Option<serde_json::Value>,
        originating_user_id: Option<&str>,
    ) -> Result<SpawnOutcome, String> {
        let originating_user_id = originating_user_id
            .map(ToString::to_string)
            .or_else(|| parent.env.originating_user_id.clone());
        let outcome = {
            let lock = self.parent_lock(plan.parent);
            let _guard = lock.lock().await;
            if let Some(record) = spawn_record {
                let tx = Transaction::new_chained(
                    plan.parent,
                    TransactionType::SubagentSpawn,
                    Bytes::from(record.to_string().into_bytes()),
                    None,
                );
                write_system_record(&self.store, plan.parent, tx)
                    .map_err(|e| format!("spawn record: {e}"))?;
            }
            KernelSpawnHook::new(self.store.clone())
                .spawn_child(
                    &plan.parent,
                    originating_user_id.as_deref(),
                    ChildAgentSpec {
                        name: plan.name.clone(),
                        role: plan.role.clone(),
                        permissions: plan.permissions.clone(),
                        tool_permissions: Some(plan.tool_permissions.clone()),
                        parent_tool_permissions: parent.tool_permissions.clone(),
                        system_prompt_override: Some(plan.system_prompt.clone()),
                        preassigned_agent_id: Some(plan.child_id),
                        initial_prompt: None,
                    },
                )
                .await
                .map_err(|e| format!("spawn child: {e}"))?
        };

        let mut chain = Vec::with_capacity(parent.chain.len() + 1);
        chain.push(plan.parent);
        chain.extend(parent.chain.iter().copied());

        let identity = self
            .scheduler
            .identity_registry()
            .get(plan.parent)
            .map(|mut identity| {
                identity.model.clone_from(&plan.model);
                if let Some(effort) = plan.reasoning_effort {
                    identity.reasoning_effort = Some(effort);
                }
                if let Some(tokens) = plan.response_max_tokens {
                    identity.max_tokens = tokens;
                }
                identity.system_prompt.clone_from(&plan.system_prompt);
                self.scheduler
                    .identity_registry()
                    .register(plan.child_id, identity.clone());
                identity
            });
        let mut loop_config = setup::loop_config_for(&plan.model, &plan.system_prompt, identity);
        if let Some(kind) = plan.kind.as_ref() {
            setup::apply_kind_budget(&mut loop_config, kind);
        }
        if let Some(effort) = plan.reasoning_effort {
            loop_config.user_thinking_effort = Some(effort);
        }
        if let Some(tokens) = plan.response_max_tokens {
            loop_config.max_tokens = tokens;
        }
        if let Some(cap) = plan.iteration_cap {
            loop_config.max_iterations = loop_config.max_iterations.min(cap as usize);
        }
        if let Some(cap) = plan.token_cap {
            loop_config.max_tokens = loop_config.max_tokens.min(cap);
        }

        let request = ChildKernelRequest {
            child_agent_id: plan.child_id,
            permissions: plan.permissions.clone(),
            tool_permissions: plan.tool_permissions.clone(),
            user_tool_defaults: parent.env.user_tool_defaults.clone(),
            parent_chain: chain.clone(),
            originating_user_id: originating_user_id.clone(),
            model_id: plan.model.clone(),
        };
        let kernel = parent.env.factory.build_child(&request);
        let hooks = AgentHooks::new(Arc::downgrade(self), plan.child_id);
        let mut resolver = hooks
            .attach_with_env(kernel.resolver, &parent.env)
            .with_caller_permissions(plan.permissions.clone())
            .with_tool_permission_context(
                parent.env.user_tool_defaults.clone(),
                Some(plan.tool_permissions.clone()),
            )
            .with_parent_chain(chain.clone());
        if let Some(user) = originating_user_id.filter(|u| !u.trim().is_empty()) {
            resolver = resolver.with_originating_user_id(user);
        }
        if !plan.model.trim().is_empty() {
            resolver = resolver.with_caller_model_id(plan.model.clone());
        }
        let mut tools = kernel.tools;
        if let Some(allowed) = &plan.allowed_tools {
            tools.retain(|tool| allowed.contains(&tool.name));
        }

        let one_shot = plan.kind.is_some();
        let setup = RunSetup {
            loop_config,
            policy: plan.policy,
            router: crate::executor::build_executor_router(resolver),
            tools,
            timeout: plan.timeout,
        };
        info!(
            child = %plan.child_id,
            parent = %plan.parent,
            name = %plan.name,
            depth = chain.len(),
            "agent spawned"
        );
        self.agents.lock().insert(
            plan.child_id,
            AgentEntry {
                name: plan.name,
                role: plan.role,
                chain,
                permissions: plan.permissions,
                tool_permissions: Some(plan.tool_permissions),
                mode: plan.mode,
                model: plan.model,
                env: parent.env.clone(),
                stop: parent.stop.child_token(),
                one_shot,
                running: false,
                paused: false,
                turns: 0,
                last_reply: None,
                body: Body::Hosted {
                    setup: Arc::new(setup),
                    history: Vec::new(),
                    queue: VecDeque::new(),
                },
            },
        );
        Ok(outcome)
    }

    // ------------------------------------------------------------------
    // messaging
    // ------------------------------------------------------------------

    fn envelope(
        &self,
        from: Option<AgentId>,
        kind: MessageKind,
        text: String,
        waiter: Option<oneshot::Sender<SubagentResult>>,
        events: Option<mpsc::Sender<AgentLoopEvent>>,
        cancel: Option<CancellationToken>,
    ) -> Envelope {
        let from_name = from
            .and_then(|id| self.agents.lock().get(&id).map(|e| e.name.clone()))
            .unwrap_or_else(|| "user".to_string());
        Envelope {
            from,
            from_name,
            kind,
            text,
            waiter,
            events,
            cancel,
        }
    }

    pub(super) fn deliver_message(
        self: &Arc<Self>,
        from: Option<AgentId>,
        to: AgentId,
        kind: MessageKind,
        text: String,
    ) -> Result<(), String> {
        let envelope = self.envelope(from, kind, text, None, None, None);
        self.enqueue(to, envelope)
    }

    /// Queue `envelope` for a hosted agent and wake it, or hand it to a
    /// root's inbox.
    fn enqueue(self: &Arc<Self>, to: AgentId, envelope: Envelope) -> Result<(), String> {
        let root_inbox = {
            let mut agents = self.agents.lock();
            let entry = agents
                .get_mut(&to)
                .ok_or_else(|| format!("unknown agent {to}"))?;
            match &mut entry.body {
                Body::Root { inbox } => Some((inbox.clone(), envelope)),
                Body::Hosted { queue, .. } => {
                    queue.push_back(envelope);
                    None
                }
            }
        };
        match root_inbox {
            None => {
                self.pump(to);
                Ok(())
            }
            Some((inbox, envelope)) => {
                let Some(from) = envelope.from else {
                    return Err("roots only accept messages from agents".to_string());
                };
                let message = InboxMessage {
                    to,
                    from,
                    from_name: envelope.from_name,
                    kind: envelope.kind,
                    text: envelope.text,
                };
                let store = self.store.clone();
                tokio::spawn(async move {
                    let delivered = match inbox {
                        Some(inbox) => inbox.deliver(message.clone()).await,
                        None => Err("no inbox".to_string()),
                    };
                    if let Err(err) = delivered {
                        info!(root = %to, %err, "root inbox unavailable; recording message");
                        record_message(&store, &message);
                    }
                });
                Ok(())
            }
        }
    }

    /// Start the next queued turn if the agent is free to run one.
    fn pump(self: &Arc<Self>, agent_id: AgentId) {
        let job = {
            let mut agents = self.agents.lock();
            let Some(entry) = agents.get_mut(&agent_id) else {
                return;
            };
            if entry.running || entry.paused {
                return;
            }
            let parent = entry.parent();
            let Body::Hosted {
                setup,
                history,
                queue,
            } = &mut entry.body
            else {
                return;
            };
            let Some(envelope) = queue.pop_front() else {
                return;
            };
            entry.running = true;
            let prompt = render_for_hosted(&envelope, parent);
            Turn {
                envelope,
                prompt,
                history: history.clone(),
                setup: setup.clone(),
                workspace: entry.env.workspace.clone(),
                stop: entry.stop.clone(),
            }
        };
        let host = self.clone();
        tokio::spawn(async move { host.run_turn(agent_id, job).await });
    }

    async fn run_turn(self: Arc<Self>, agent_id: AgentId, turn: Turn) {
        let Turn {
            envelope,
            prompt,
            history,
            setup,
            workspace,
            stop,
        } = turn;
        let Envelope {
            from,
            kind,
            waiter,
            events,
            cancel,
            ..
        } = envelope;
        let (result, messages) = self
            .execute(
                agent_id, &prompt, history, &setup, workspace, events, cancel, &stop,
            )
            .await;

        // Stopped while the turn ran: the entry is gone and nobody but a
        // waiter should hear about it.
        let reply_to = {
            let mut agents = self.agents.lock();
            agents.get_mut(&agent_id).and_then(|entry| {
                entry.running = false;
                entry.turns += 1;
                if !result.final_message.trim().is_empty() {
                    entry.last_reply = Some(result.final_message.clone());
                }
                if let (Some(messages), Body::Hosted { history, .. }) = (messages, &mut entry.body)
                {
                    *history = messages;
                }
                match kind {
                    MessageKind::Request => from,
                    MessageKind::Reply => entry.parent(),
                }
            })
        };

        match waiter {
            Some(waiter) => {
                let _ = waiter.send(result);
            }
            None => {
                if let Some(to) = reply_to {
                    let text = reply_text(&result);
                    if let Err(err) =
                        self.deliver_message(Some(agent_id), to, MessageKind::Reply, text)
                    {
                        warn!(agent = %agent_id, %err, "could not deliver reply");
                    }
                }
            }
        }
        if !self.retire_if_done(agent_id) {
            self.pump(agent_id);
        }
    }

    /// Remove a `task` child, and everything it started, once it has
    /// nothing left to do. Returns whether it was removed.
    fn retire_if_done(&self, agent_id: AgentId) -> bool {
        let dropped = {
            let mut agents = self.agents.lock();
            let done = agents.get(&agent_id).is_some_and(|entry| {
                entry.one_shot
                    && !entry.running
                    && matches!(&entry.body, Body::Hosted { queue, .. } if queue.is_empty())
            });
            if !done {
                return false;
            }
            self.remove_subtree(&mut agents, agent_id, true)
        };
        notify_dropped(dropped);
        true
    }

    /// Cancel and remove `target`'s descendants, and `target` itself when
    /// `include_target`. Returns the queued messages that will never run.
    fn remove_subtree(
        &self,
        agents: &mut HashMap<AgentId, AgentEntry>,
        target: AgentId,
        include_target: bool,
    ) -> Vec<(AgentId, Envelope)> {
        let ids: Vec<AgentId> = agents
            .iter()
            .filter(|(id, e)| (include_target && **id == target) || e.chain.contains(&target))
            .map(|(id, _)| *id)
            .collect();
        let mut dropped = Vec::new();
        for id in ids {
            let Some(entry) = agents.remove(&id) else {
                continue;
            };
            entry.stop.cancel();
            self.parent_locks.remove(&id);
            if let Body::Hosted { queue, .. } = entry.body {
                dropped.extend(queue.into_iter().map(|env| (id, env)));
                self.scheduler.identity_registry().unregister(id);
            }
        }
        dropped
    }

    /// Run one turn of a hosted agent through the scheduler.
    #[allow(clippy::too_many_arguments)]
    async fn execute(
        &self,
        agent_id: AgentId,
        prompt: &str,
        history: Vec<Message>,
        setup: &RunSetup,
        workspace: Option<(PathBuf, bool)>,
        events: Option<mpsc::Sender<AgentLoopEvent>>,
        cancel: Option<CancellationToken>,
        stop: &CancellationToken,
    ) -> (SubagentResult, Option<Vec<Message>>) {
        if setup.timeout == Some(Duration::ZERO) {
            return (ended(agent_id, SubagentExit::Timeout), None);
        }
        let tx = Transaction::user_prompt(agent_id, prompt.to_string());
        if let Err(err) = self.store.enqueue_tx(&tx) {
            return (failed(agent_id, format!("enqueue prompt: {err}")), None);
        }
        let schedule = self.scheduler.schedule_agent_with_options(
            agent_id,
            ScheduleOverrides {
                caller_owns_deadline: true,
                loop_config: Some(setup.loop_config.clone()),
                policy: Some(setup.policy.clone()),
                event_tx: events,
                workspace_override: workspace,
                router_override: Some(setup.router.clone()),
                tools_override: Some(setup.tools.clone()),
                history,
            },
        );
        let bounded = async {
            match setup.timeout {
                Some(limit) => tokio::time::timeout(limit, schedule).await.ok(),
                None => Some(schedule.await),
            }
        };
        let cancel = cancel.unwrap_or_default();
        let processed = tokio::select! {
            biased;
            () = stop.cancelled() => return (cancelled(agent_id), None),
            () = cancel.cancelled() => return (cancelled(agent_id), None),
            outcome = bounded => match outcome {
                None => return (ended(agent_id, SubagentExit::Timeout), None),
                Some(Err(err)) => return (failed(agent_id, format!("schedule: {err}")), None),
                Some(Ok(processed)) => processed,
            },
        };
        let Some(result) = processed.last_result else {
            return (failed(agent_id, "the turn produced no result".into()), None);
        };
        let exit = if result.output_truncated || result.insufficient_credits || result.stalled {
            SubagentExit::Failed {
                reason: "child execution stopped before completion".into(),
            }
        } else if result.timed_out {
            SubagentExit::Timeout
        } else {
            result
                .llm_error
                .as_ref()
                .map_or(SubagentExit::Completed, |reason| SubagentExit::Failed {
                    reason: reason.clone(),
                })
        };
        let outcome = SubagentResult {
            child_agent_id: Some(agent_id),
            final_message: result.total_text,
            total_input_tokens: result.total_input_tokens,
            total_output_tokens: result.total_output_tokens,
            files_changed: result
                .file_changes
                .into_iter()
                .map(|change| change.path)
                .collect(),
            exit,
        };
        (outcome, Some(result.messages))
    }

    // ------------------------------------------------------------------
    // lifecycle
    // ------------------------------------------------------------------

    pub(super) fn lifecycle(
        self: &Arc<Self>,
        target: AgentId,
        action: LifecycleAction,
    ) -> Result<(), String> {
        let dropped = {
            let mut agents = self.agents.lock();
            let entry = agents
                .get(&target)
                .ok_or_else(|| format!("unknown agent {target}"))?;
            if matches!(entry.body, Body::Root { .. }) {
                return Err("root agents are controlled by their surface".to_string());
            }
            let parent_stop = entry
                .parent()
                .and_then(|p| agents.get(&p))
                .map(|p| p.stop.clone());
            match action {
                LifecycleAction::Pause | LifecycleAction::Resume => {
                    let paused = action == LifecycleAction::Pause;
                    for (id, entry) in agents.iter_mut() {
                        if *id == target || entry.chain.contains(&target) {
                            entry.paused = paused;
                        }
                    }
                    Vec::new()
                }
                LifecycleAction::Stop => self.remove_subtree(&mut agents, target, true),
                LifecycleAction::Restart => {
                    let dropped = self.remove_subtree(&mut agents, target, false);
                    if let Some(entry) = agents.get_mut(&target) {
                        entry.paused = false;
                        entry.stop.cancel();
                        entry.stop = parent_stop
                            .as_ref()
                            .map_or_else(CancellationToken::new, CancellationToken::child_token);
                        if let Body::Hosted { history, .. } = &mut entry.body {
                            history.clear();
                        }
                    }
                    dropped
                }
            }
        };
        notify_dropped(dropped);
        info!(agent = %target, ?action, "agent lifecycle");
        if matches!(action, LifecycleAction::Resume | LifecycleAction::Restart) {
            let ids: Vec<AgentId> = self.agents.lock().keys().copied().collect();
            for id in ids {
                self.pump(id);
            }
        }
        Ok(())
    }

    /// True when `ancestor` spawned `agent`, directly or indirectly.
    pub(super) fn is_descendant(&self, agent: AgentId, ancestor: AgentId) -> bool {
        self.agents
            .lock()
            .get(&agent)
            .is_some_and(|e| e.chain.contains(&ancestor))
    }
}

struct ParentSnapshot {
    chain: Vec<AgentId>,
    env: AgentEnv,
    mode: AgentMode,
    model: String,
    permissions: AgentPermissions,
    tool_permissions: Option<AgentToolPermissions>,
    stop: CancellationToken,
}

struct Turn {
    envelope: Envelope,
    prompt: String,
    history: Vec<Message>,
    setup: Arc<RunSetup>,
    workspace: Option<(PathBuf, bool)>,
    stop: CancellationToken,
}

/// The prompt a hosted agent sees for `envelope`. Its parent talks to it
/// directly; anyone else is introduced.
fn render_for_hosted(envelope: &Envelope, parent: Option<AgentId>) -> String {
    let Some(from) = envelope.from else {
        return envelope.text.clone();
    };
    match envelope.kind {
        MessageKind::Request if Some(from) == parent => envelope.text.clone(),
        MessageKind::Request => format!(
            "Message from agent {} ({from}):\n\n{}\n\nYour final answer is sent back to {}.",
            envelope.from_name, envelope.text, envelope.from_name
        ),
        MessageKind::Reply => format!(
            "Agent {} ({from}) reported back:\n\n{}",
            envelope.from_name, envelope.text
        ),
    }
}

fn reply_text(result: &SubagentResult) -> String {
    match &result.exit {
        SubagentExit::Completed if result.final_message.trim().is_empty() => {
            "(finished without a reply)".to_string()
        }
        SubagentExit::Completed => result.final_message.clone(),
        SubagentExit::Failed { reason } => format!("failed: {reason}"),
        SubagentExit::Cancelled => "was cancelled".to_string(),
        SubagentExit::Timeout => "timed out".to_string(),
        SubagentExit::Rejected { reason } => format!("rejected: {reason}"),
    }
}

/// Append a message a root could not receive live to its record.
fn record_message(store: &Arc<dyn Store>, message: &InboxMessage) {
    let payload = serde_json::json!({
        "kind": "agent_message",
        "from_agent_id": message.from,
        "from_name": message.from_name,
        "reply": message.kind == MessageKind::Reply,
        "text": message.text,
    });
    let tx = Transaction::new_chained(
        message.to,
        TransactionType::System,
        Bytes::from(payload.to_string().into_bytes()),
        None,
    );
    if let Err(err) = write_system_record(store, message.to, tx) {
        warn!(root = %message.to, %err, "could not record agent message");
    }
}

fn ended(agent_id: AgentId, exit: SubagentExit) -> SubagentResult {
    SubagentResult {
        child_agent_id: Some(agent_id),
        final_message: String::new(),
        total_input_tokens: 0,
        total_output_tokens: 0,
        files_changed: Vec::new(),
        exit,
    }
}

fn cancelled(agent_id: AgentId) -> SubagentResult {
    ended(agent_id, SubagentExit::Cancelled)
}

/// Tell anyone waiting on a dropped message that it was cancelled.
fn notify_dropped(dropped: Vec<(AgentId, Envelope)>) {
    for (id, envelope) in dropped {
        if let Some(waiter) = envelope.waiter {
            let _ = waiter.send(cancelled(id));
        }
    }
}

fn failed(agent_id: AgentId, reason: String) -> SubagentResult {
    ended(agent_id, SubagentExit::Failed { reason })
}
