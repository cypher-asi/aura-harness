use super::*;

impl AgentHub {
    /// Re-read the permanent agents, returning files that could not be used.
    pub fn reload_definitions(&self) -> Vec<String> {
        lock(&self.skills).reload();
        lock(&self.definitions).reload()
    }

    /// Rows for the roster (the agents panel's Agents tab), each with its
    /// abbreviation. Definitions without one yet reserve it now, in roster
    /// order (see [`AgentRegistry::reserve_prefixes`]); when that cannot
    /// be saved the roster still shows them, and the next save keeps them.
    pub fn definition_summaries(&self) -> Vec<AgentDefinitionSummary> {
        // Only channels create coordinators, so the roster never offers one.
        let mut summaries: Vec<AgentDefinitionSummary> = lock(&self.definitions)
            .all()
            .iter()
            .filter(|d| !d.is_coordinator())
            .map(|d| AgentDefinitionSummary {
                name: d.name.clone(),
                description: d.description.clone(),
                abbreviation: None,
            })
            .collect();
        let names: Vec<&str> = summaries.iter().map(|s| s.name.as_str()).collect();
        let abbreviations = {
            let mut registry = lock(&self.registry);
            registry.reserve_prefixes(&names).or_else(|error| {
                tracing::warn!(
                    error = %format!("{error:#}"),
                    "could not save the permanent agents' abbreviations"
                );
                // Reserved in memory all the same, so asking again
                // reserves nothing new and saves nothing.
                registry.reserve_prefixes(&names)
            })
        }
        .unwrap_or_default();
        for (summary, abbreviation) in summaries.iter_mut().zip(abbreviations) {
            summary.abbreviation = Some(abbreviation);
        }
        summaries
    }

    /// Rows for the Agents panel, without the members of channels that
    /// have no project.
    pub fn agent_summaries(&self) -> Vec<AgentSummary> {
        let registry = lock(&self.registry);
        let running: HashSet<AgentId> = lock(&self.running).keys().copied().collect();
        let mut rows = agent_summaries(
            &registry,
            self.factory.store.as_ref(),
            registry.current().agent_id(),
            &running,
        );
        drop(registry);
        let hidden = lock(&self.hidden_members);
        rows.retain(|row| AgentId::from_hex(&row.id).map_or(true, |id| !hidden.contains(&id)));
        drop(hidden);
        for row in &mut rows {
            if let Ok(agent_id) = AgentId::from_hex(&row.id) {
                self.mark_plan_mode(agent_id, row);
            }
        }
        rows
    }

    /// Archive spawns from an earlier run of the app that never got to
    /// record anything: the app closed or was killed while a `task` call
    /// was still being dispatched, or while a writer waited for the
    /// workspace (see [`Self::write_slot`]). Nothing was recorded for
    /// them, so there is no prompt or turn to resume; left alone they sit
    /// in the Agents panel with a chat that stays empty forever.
    ///
    /// Only agents a `task` call spawned (`parent` is set) are candidates:
    /// a user-created project or agent legitimately starts with an empty
    /// record before its first message, and is left untouched.
    ///
    /// See [`crate::record_loader::archive_unstarted_spawns`], which this
    /// runs on the hub's registry.
    pub fn archive_unstarted_spawns(&self) -> Vec<AgentId> {
        crate::record_loader::archive_unstarted_spawns(
            &mut lock(&self.registry),
            self.factory.store.as_ref(),
        )
    }

    /// Give `agent` the chat title `title` and show it in the Agents panel
    /// right away, unless the agent got a name or title in the meantime.
    pub(in crate::event_loop) async fn apply_title(&self, agent: AgentId, title: &str) {
        let applied = {
            let mut registry = lock(&self.registry);
            let wanted = registry
                .find(agent)
                .is_some_and(crate::agent_registry::RegisteredAgent::needs_title);
            wanted
                && match registry.set_title(agent, title) {
                    Ok(()) => true,
                    Err(e) => {
                        warn!(%agent, error = %e, "Could not save the chat title");
                        false
                    }
                }
        };
        if applied {
            self.send_agent_list().await;
        }
    }

    /// Name `agent` after its subtask, `subtitle`, and show it in the
    /// Agents panel right away, unless it was named meanwhile or is no
    /// spawned instance.
    pub(in crate::event_loop) async fn apply_subtitle(&self, agent: AgentId, subtitle: &str) {
        if self.set_subtitle_if_needed(agent, subtitle) {
            self.send_agent_list().await;
        }
    }

    /// Save `subtitle` for `agent` when it still needs one. Returns whether
    /// it was saved.
    pub(super) fn set_subtitle_if_needed(&self, agent: AgentId, subtitle: &str) -> bool {
        let mut registry = lock(&self.registry);
        let wanted = registry
            .find(agent)
            .is_some_and(crate::agent_registry::RegisteredAgent::needs_subtitle);
        wanted
            && match registry.set_subtitle(agent, subtitle) {
                Ok(()) => true,
                Err(e) => {
                    warn!(%agent, error = %e, "Could not save the subtask's subtitle");
                    false
                }
            }
    }

    /// Name the subtask `child` was spawned for with `prompt`: by the
    /// title model in the background, through the child's `kernel` with
    /// `config`'s credentials, or at once from the prompt under a
    /// benchmark's run, whose agents are named without model calls. The
    /// same model writes a subject for the INBOX `thread` the task opens,
    /// when it opens one and mail is collected.
    pub(super) async fn name_subtask(
        self: &Arc<Self>,
        request: &SubagentDispatchRequest,
        child: AgentId,
        kernel: &Arc<Kernel>,
        config: &AgentLoopConfig,
        prompt: &str,
        thread: Option<String>,
    ) {
        if self.in_bench(request.parent_agent_id) {
            self.apply_subtitle(child, &crate::event_loop::titles::bench_subtitle(prompt))
                .await;
            return;
        }
        let classifier = lock(&self.base).gate.classifier.clone();
        crate::event_loop::titles::spawn_subtitle(
            Arc::clone(self),
            child,
            aura_agent::KernelModelGateway::new(Arc::clone(kernel)),
            config.clone(),
            crate::event_loop::titles::title_model(classifier.clone()),
            prompt.to_string(),
        );
        if let Some(thread) = thread.filter(|_| self.mail_open()) {
            crate::event_loop::titles::spawn_subject(
                self.commands.clone(),
                thread,
                aura_agent::KernelModelGateway::new(Arc::clone(kernel)),
                config.clone(),
                crate::event_loop::titles::title_model(classifier),
                prompt.to_string(),
            );
        }
    }

    /// The projects, the Agents panel rows and the Archive, in the order
    /// the UI takes them: the projects first, so one with every agent
    /// archived is known when the rows arrive. Also returns the agent the
    /// rows mark active.
    ///
    /// The registry is held only while the rows are laid out; the store is
    /// read after it is released, so the event loop and the spawns don't
    /// wait on the reads.
    pub(super) fn agent_lists_with_active(&self) -> AgentLists {
        self.agent_lists_marking(None)
    }

    /// [`Self::agent_lists_with_active`] with `active` marked as the open
    /// agent, or the registry's when `None`.
    pub(super) fn agent_lists_marking(&self, active: Option<AgentId>) -> AgentLists {
        let running: HashSet<AgentId> = lock(&self.running).keys().copied().collect();
        let mut rows = {
            let registry = lock(&self.registry);
            self.processes.name_agents(
                registry
                    .agents()
                    .iter()
                    .map(|agent| (agent.agent_id(), agent.display_name())),
            );
            let active = active.unwrap_or_else(|| registry.current().agent_id());
            crate::record_loader::ListRows::build(&registry, &running, active)
        };
        rows.read_activity(self.factory.store.as_ref(), &mut lock(&self.last_active));
        for (agent_id, row) in rows.listed.iter_mut().chain(rows.archived.iter_mut()) {
            self.mark_plan_mode(*agent_id, row);
        }
        // After the rows are complete, so the members' copies carry the
        // same activity and mode. The member set is copied out so the two
        // sets are never locked together.
        let members = lock(&self.channel_members).clone();
        rows.split_members(&members, &lock(&self.hidden_members));
        let active = rows.active;
        let members = std::mem::take(&mut rows.members).into_iter().collect();
        AgentLists {
            commands: rows.into_commands(),
            active,
            members,
        }
    }

    /// The rows of every channel member that is not archived, built as the
    /// Agents panel's rows are, for the channels list.
    pub(crate) fn member_rows(&self) -> HashMap<AgentId, AgentSummary> {
        self.agent_lists_with_active().members
    }

    /// Where the UI's commands go.
    pub(crate) fn commands(&self) -> &mpsc::Sender<UiCommand> {
        &self.commands
    }

    /// Mark `row`, the Agents panel or Archive row of `agent_id`, when the
    /// agent is in plan mode. Archived agents can be opened and run turns
    /// too, so their rows are marked as well.
    pub(super) fn mark_plan_mode(&self, agent_id: AgentId, row: &mut AgentSummary) {
        row.plan_mode = self.mode_of(agent_id) == PermissionMode::Plan;
    }

    /// The projects, the Agents panel rows and the Archive, in the order
    /// the UI takes them: the projects first, so one with every agent
    /// archived is known when the rows arrive.
    pub fn agent_lists(&self) -> [UiCommand; 3] {
        self.agent_lists_with_active().commands
    }

    /// Send the projects, the Agents panel rows, the Archive and the
    /// active agent to the UI, then the channels, whose member rows show
    /// the same names and activity.
    pub async fn send_agent_list(&self) {
        self.send_lists(self.agent_lists_with_active(), true).await;
    }

    /// The lists for a switch to `active`, which they mark open, built on
    /// a blocking thread: with hundreds of agents they take a while, and
    /// the switch reads the agent's record meanwhile. `None` if building
    /// them failed.
    pub(in crate::event_loop) async fn lists_for_switch(
        self: &Arc<Self>,
        active: AgentId,
    ) -> Option<AgentLists> {
        let hub = Arc::clone(self);
        tokio::task::spawn_blocking(move || hub.agent_lists_marking(Some(active)))
            .await
            .inspect_err(|e| warn!(error = %e, "Could not build the agent lists"))
            .ok()
    }

    /// Send `lists`, built for a switch, and the channels. Not the active
    /// agent: the switch sends it, and sent again, the UI would put it on
    /// screen a second time, asking again for the LOG, GRAPH and BENCH it
    /// shows.
    pub(in crate::event_loop) async fn send_switch_lists(&self, lists: AgentLists) {
        self.send_lists(lists, false).await;
    }

    pub(super) async fn send_lists(&self, lists: AgentLists, with_active: bool) {
        for list in lists.commands {
            let _ = self.commands.send(list).await;
        }
        if with_active {
            let _ = self
                .commands
                .send(UiCommand::SetActiveAgent(lists.active.to_hex()))
                .await;
        }
        crate::event_loop::channels::send_channels_with(self, lists.members, false).await;
    }
}
