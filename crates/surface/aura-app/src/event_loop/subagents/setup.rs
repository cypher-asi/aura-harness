use super::*;
use crate::cloud::hub::TurnAsk;

/// Said after a cloud member's `<channel>` block: its machine finds the
/// agents of its channel by id alone.
const MACHINE_PEERS: &str = "\n\nYou run on a cloud machine: to reach the agents of \
your channel, give `send_to_agent` the `agent_id` that `list_agents` shows for each, not \
its code.";

impl AgentHub {
    /// Build the hub. `factory` receives the dispatcher that kernels must
    /// install so their `task` calls spawn permanent agents.
    pub fn new(
        parts: AgentHubParts,
        factory: impl FnOnce(Arc<dyn SubagentDispatchHook>) -> KernelFactory,
    ) -> Arc<Self> {
        let (notices, notice_rx) = mpsc::unbounded_channel();
        let mut parts = parts;
        if let Some(approvals) = &parts.approvals {
            approvals.set_default_level(parts.approval);
        }
        let tasks = Arc::new(TaskBoard::new(
            parts.commands.clone(),
            parts.record_panel.clone(),
        ));
        let channel_sets = crate::event_loop::channels::load_channels(
            parts.channels.as_ref(),
            &mut parts.registry,
        );
        parts
            .registry
            .mark_channel_members(channel_sets.members.iter().copied());
        Arc::new_cyclic(|hub: &Weak<Self>| {
            let dispatcher: Arc<dyn SubagentDispatchHook> =
                Arc::new(TerminalSubagentDispatcher { hub: hub.clone() });
            let mut factory = factory(dispatcher);
            factory.tasks = tasks;
            Self::assemble(parts, factory, channel_sets, notices, notice_rx)
        })
    }

    /// The hub `parts` describe, spawning through `factory`, with the
    /// channel members `channel_sets` lists and its notice channel.
    fn assemble(
        parts: AgentHubParts,
        factory: KernelFactory,
        channel_sets: crate::event_loop::channels::ChannelSets,
        notices: NoticeSender,
        notice_rx: mpsc::UnboundedReceiver<Notice>,
    ) -> Self {
        Self {
            registry: Mutex::new(parts.registry),
            definitions: Mutex::new(parts.definitions),
            skills: Mutex::new(parts.skills),
            base: Mutex::new(parts.base),
            running: Mutex::new(HashMap::new()),
            next_generation: AtomicU64::new(0),
            abandoned: Mutex::new(Vec::new()),
            finished: Mutex::new(Vec::new()),
            spawn_finished: Notify::new(),
            factory,
            commands: parts.commands,
            long_run: parts.long_run,
            permissions: Mutex::new(HashMap::new()),
            before_plan: Mutex::new(HashMap::new()),
            default_permissions: Permissions::new(parts.mode, parts.approval),
            approvals: parts.approvals,
            instructions: parts.instructions,
            processes: parts.processes,
            notices,
            notice_rx: Mutex::new(Some(notice_rx)),
            mail: Mutex::new(None),
            agents: parts.agents,
            record_panel: parts.record_panel,
            traces: parts.traces,
            gate: parts.gate,
            compare_arms: Mutex::new(HashMap::new()),
            write_locks: Mutex::new(HashMap::new()),
            writing: Mutex::new(HashSet::new()),
            last_active: Mutex::default(),
            channel_members: Mutex::new(channel_sets.members),
            hidden_members: Mutex::new(channel_sets.hidden),
            cloud_starts: Mutex::default(),
            cloud_turns: Mutex::default(),
            machine_working: Mutex::default(),
            channels: parts.channels,
            channel_chat: crate::event_loop::channels::ChannelChat::default(),
            inbox: crate::event_loop::inbox::Inbox::default(),
            shown_chats: Mutex::default(),
            shown_records: Mutex::default(),
        }
    }

    /// Route the subtasks of the `/compare` run `agent` (and theirs) as
    /// `routing` says, on `gate`, its arm's own view of Gate, until
    /// [`Self::end_compare_run`].
    pub(crate) fn begin_compare_run(
        &self,
        agent: AgentId,
        routing: ArmRouting,
        gate: Arc<GateLive>,
    ) {
        lock(&self.compare_arms).insert(agent, CompareArmRun { routing, gate });
    }

    /// The `/compare` run `agent` ended.
    pub(crate) fn end_compare_run(&self, agent: AgentId) {
        lock(&self.compare_arms).remove(&agent);
    }

    /// How the subtasks of `agent` are routed, and on which view of Gate,
    /// when it works for a `/compare` run, directly or through the agents
    /// between.
    pub(super) fn compare_arm(&self, agent: AgentId) -> Option<CompareArmRun> {
        let ancestry = lock(&self.registry).ancestry(agent);
        let arms = lock(&self.compare_arms);
        ancestry.iter().find_map(|id| arms.get(id).cloned())
    }

    /// How the subtasks of `agent` are routed when it works for a
    /// `/compare` run (see [`Self::compare_arm`]).
    pub(super) fn compare_routing(&self, agent: AgentId) -> Option<ArmRouting> {
        self.compare_arm(agent).map(|arm| arm.routing)
    }

    /// Hand `session`, whose work ended, to the event loop to keep, and
    /// note that it stopped running.
    ///
    /// Parked before it stops running, so a spawn that ends normally is
    /// never seen with neither a session nor work in flight. A spawn whose
    /// work is dropped before it gets here loses its session with the
    /// dropped future: its [`RunningGuard`] ends the running and lists it
    /// in [`Self::take_abandoned`], and the event loop reopens its session
    /// from the record (see `LoopState::park_finished_spawns`).
    pub(crate) fn park(&self, session: AgentSession) {
        let agent_id = session.agent_id();
        lock(&self.finished).push(session);
        self.stop_running(agent_id);
        self.spawn_finished.notify_one();
    }

    /// Work mode of `agent_id`.
    pub fn mode_of(&self, agent_id: AgentId) -> PermissionMode {
        self.permissions_of(agent_id).mode
    }

    /// Work mode and approval level of `agent_id`.
    pub(crate) fn permissions_of(&self, agent_id: AgentId) -> Permissions {
        lock(&self.permissions)
            .get(&agent_id)
            .copied()
            .unwrap_or(self.default_permissions)
    }

    /// Note that `agent_id` works under `permissions` from now on.
    pub(in crate::event_loop) fn set_permissions(
        &self,
        agent_id: AgentId,
        permissions: Permissions,
    ) {
        let from = lock(&self.permissions).insert(agent_id, permissions);
        let from = from.unwrap_or(self.default_permissions).mode;
        if permissions.mode == PermissionMode::Plan && from != PermissionMode::Plan {
            lock(&self.before_plan).insert(agent_id, from);
        }
        if let Some(approvals) = &self.approvals {
            approvals.set_level(agent_id, permissions.approval);
        }
    }

    /// The mode `/plan` returns `agent_id` to: the one it had before plan
    /// mode, or `agent` when the session started there.
    pub(in crate::event_loop) fn exit_plan_mode(&self, agent_id: AgentId) -> PermissionMode {
        lock(&self.before_plan)
            .get(&agent_id)
            .copied()
            .filter(|mode| *mode != PermissionMode::Plan)
            .unwrap_or(PermissionMode::Agent)
    }

    /// The mode `/build` puts `agent_id` in: its current one when that
    /// lets it edit, else the one it had before plan mode, else
    /// `agent`.
    pub(in crate::event_loop) fn build_mode(&self, agent_id: AgentId) -> PermissionMode {
        let mode = self.mode_of(agent_id);
        if mode.agent_mode() == aura_core_types::AgentMode::Agent {
            return mode;
        }
        lock(&self.before_plan)
            .get(&agent_id)
            .copied()
            .filter(|before| before.agent_mode() == aura_core_types::AgentMode::Agent)
            .unwrap_or(PermissionMode::Agent)
    }

    /// Names of every tool a turn may offer, for the mode tables.
    pub(crate) fn tool_names(&self) -> Vec<String> {
        let mut names: Vec<String> = lock(&self.base)
            .tools
            .iter()
            .map(|tool| tool.name.clone())
            .collect();
        names.push(TASK_TOOL_NAME.to_string());
        names.push(ASSIGN_TOOL_NAME.to_string());
        names.extend(inbox_tool_definitions().into_iter().map(|tool| tool.name));
        if self.agents.is_some() {
            names.extend(host_tool_definitions().into_iter().map(|tool| tool.name));
        }
        names
    }

    /// Config and tools for a turn of an instance of the permanent agent
    /// `definition` that another agent spawned, or of a user's own agent
    /// when `None`, working in `workspace`. [`Self::agent_turn`] is the
    /// turn of a given agent.
    pub fn turn(
        &self,
        definition: Option<&str>,
        workspace: &Path,
    ) -> (AgentLoopConfig, Vec<ToolDefinition>) {
        self.ranked_turn(definition, workspace, definition.is_none())
    }

    /// [`Self::turn`] of a top-level agent, which nobody spawned, or of a
    /// subagent.
    fn ranked_turn(
        &self,
        definition: Option<&str>,
        workspace: &Path,
        top_level: bool,
    ) -> (AgentLoopConfig, Vec<ToolDefinition>) {
        let instructions = self.instructions.for_workspace(workspace);
        let (mut config, tools) = self.definition_turn(definition, &instructions.text, top_level);
        self.long_run.apply(&mut config);
        if crate::event_loop::channels::is_coordinator(definition) {
            // Its todos are its members' work, which it can't carry on with.
            config.continue_open_todos = false;
        }
        (config, tools)
    }

    /// Whether the agent `agent_id`, an instance of the permanent agent
    /// `definition` if any, leads under Gate the turns that call for a
    /// lead ([`aura_gate::lead::choose_way`]): `Some(true)` with
    /// delegation enforced, `Some(false)` with it only recommended
    /// (`AURA_GATE_DELEGATE=off`, or the `gate:nodelegate` arm of
    /// `/compare`), `None` when it never leads. Only the user's own
    /// agents lead, and only when Gate routes their work: never an
    /// instance of a permanent agent, a pinned main model or a pinned
    /// `/compare` arm.
    pub(crate) fn leads(&self, agent_id: AgentId, definition: Option<&str>) -> Option<bool> {
        if definition.is_some() {
            return None;
        }
        match self.compare_routing(agent_id) {
            Some(ArmRouting::Gate { delegate, .. }) => Some(delegate),
            Some(ArmRouting::Pinned(_) | ArmRouting::ByDifficulty(_)) => None,
            None => lock(&self.base)
                .gate
                .candidates
                .is_some()
                .then(|| self.gate.settings().delegate),
        }
    }

    /// [`Self::agent_turn`] for a turn `session` runs for the user (or a
    /// `/compare` run), and how the turn leads when its prompt turns out
    /// to ask for work a lead takes ([`LeadProfile::apply`]); `None` when
    /// it never leads (see [`Self::leads`]). `session` starts the turn
    /// without the lead's limits.
    pub(crate) fn user_turn(
        &self,
        session: &mut AgentSession,
    ) -> (AgentLoopConfig, Vec<ToolDefinition>, Option<LeadProfile>) {
        let workspace = session.kernel.agent_workspace();
        let agent_id = session.agent_id();
        let definition = session.definition.clone();
        let (mut config, tools) = self.agent_turn(agent_id, definition.as_deref(), &workspace);
        session.lead = false;
        let lead = self
            .leads(agent_id, definition.as_deref())
            .map(|enforced| LeadProfile { enforced });
        if coordinates(
            definition.as_deref(),
            self.mode_of(agent_id),
            lead.is_some(),
            &tools,
        ) {
            config.system_prompt.push_str("\n\n");
            config
                .system_prompt
                .push_str(&aura_context_prompts::system::sections::coordinator::render());
        }
        if let Some(channel) = crate::event_loop::channels::channel_prompt(self, agent_id) {
            config.system_prompt.push_str("\n\n");
            config.system_prompt.push_str(&channel);
        }
        (config, tools, lead)
    }

    /// [`Self::turn`] for `agent_id`, top level unless another agent
    /// spawned it, with the plan-mode instructions when the agent is in
    /// plan mode.
    pub(crate) fn agent_turn(
        &self,
        agent_id: AgentId,
        definition: Option<&str>,
        workspace: &Path,
    ) -> (AgentLoopConfig, Vec<ToolDefinition>) {
        let top_level = lock(&self.registry).is_top_level_agent(agent_id);
        let (mut config, tools) = self.ranked_turn(definition, workspace, top_level);
        if self.mode_of(agent_id) == PermissionMode::Plan {
            config.system_prompt.push_str("\n\n");
            config
                .system_prompt
                .push_str(&aura_context_prompts::system::sections::plan_mode::render());
        }
        (config, tools)
    }

    /// What a turn of `agent_id` on its cloud machine adds to the node's
    /// own prompt and tools, so it works as a turn here would: its
    /// project's instructions, its permanent agent's prompt and skills,
    /// plan mode's instructions and its channel's block, and the tools
    /// its permanent agent leaves out, turned off. Everything else the
    /// turn asks for is left at its default.
    pub(crate) fn cloud_turn(&self, agent_id: AgentId) -> TurnAsk {
        let (definition, folder, top_level) = {
            let registry = lock(&self.registry);
            (
                registry.find(agent_id).and_then(|a| a.definition.clone()),
                registry.project_folder(agent_id),
                registry.is_top_level_agent(agent_id),
            )
        };
        let mut prompt = folder
            .map(|folder| self.instructions.for_workspace(&folder).text.clone())
            .unwrap_or_default();
        if let Some(name) = definition.as_deref() {
            if let Some(found) = lock(&self.definitions).get(name) {
                found.extend_system_prompt(&mut prompt, &lock(&self.skills));
            }
        }
        if self.mode_of(agent_id) == PermissionMode::Plan {
            prompt.push_str("\n\n");
            prompt.push_str(&aura_context_prompts::system::sections::plan_mode::render());
        }
        if let Some(channel) = crate::event_loop::channels::channel_prompt(self, agent_id) {
            prompt.push_str("\n\n");
            prompt.push_str(&channel);
            prompt.push_str(MACHINE_PEERS);
        }
        let prompt = prompt.trim();
        let (_, offered) = self.definition_turn(None, "", top_level);
        let (_, kept) = self.definition_turn(definition.as_deref(), "", top_level);
        TurnAsk {
            system_prompt: (!prompt.is_empty()).then(|| prompt.to_string()),
            tools_off: offered
                .into_iter()
                .map(|tool| tool.name)
                .filter(|name| kept.iter().all(|tool| &tool.name != name))
                .collect(),
            ..TurnAsk::default()
        }
    }

    /// Show `agent_id` the instruction files its turns load from
    /// `workspace`.
    pub(in crate::event_loop) async fn send_instructions(
        &self,
        agent_id: AgentId,
        workspace: &Path,
    ) {
        let files = self.instructions.for_workspace(workspace).files.clone();
        AgentUi::new(self.commands.clone(), agent_id)
            .send(UiCommand::SetInstructions(files))
            .await;
    }

    /// The config and tools of a turn of an instance of `definition`, or
    /// of a user's own agent when `None`. A coordinator's tools are
    /// [`COORDINATOR_TOOLS`] whatever its file says, even when the file is
    /// gone.
    pub(super) fn definition_turn(
        &self,
        definition: Option<&str>,
        instructions: &str,
        top_level: bool,
    ) -> (AgentLoopConfig, Vec<ToolDefinition>) {
        let base = lock(&self.base);
        let definitions = lock(&self.definitions);
        let mut config = base.config.clone();
        // Right after the base prompt, ahead of anything that varies
        // between agents or turns, so the cached prompt prefix covers it.
        if !instructions.is_empty() {
            config.system_prompt.push_str("\n\n");
            config.system_prompt.push_str(instructions);
        }
        let mut tools = base.tools.clone();
        let coordinator = crate::event_loop::channels::is_coordinator(definition);
        if coordinator {
            tools.extend(assign_tool_definition(&definitions));
            tools.extend(inbox_tool_definitions());
            tools.retain(|tool| COORDINATOR_TOOLS.contains(&tool.name.as_str()));
        } else {
            tools.extend(task_tool_definition(&definitions));
            if top_level || self.agents.is_some() {
                tools.extend(inbox_tool_definitions());
            }
            if self.agents.is_some() {
                tools.extend(host_tool_definitions());
            }
        }
        let Some(name) = definition else {
            return (config, tools);
        };
        let Some(definition) = definitions.get(name) else {
            warn!(
                definition = name,
                "Permanent agent no longer exists; using defaults"
            );
            return (config, tools);
        };
        if !coordinator {
            tools = definition_tools(definition, &tools, top_level);
        }
        definition.extend_system_prompt(&mut config.system_prompt, &lock(&self.skills));
        if let Some(model) = &definition.model {
            base.limits.configure(&mut config, model);
        }
        (config, tools)
    }
}
