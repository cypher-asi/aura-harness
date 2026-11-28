use super::*;

impl AgentHub {
    /// Let `agent`, about to run a turn on `config` in `folder`, spawn
    /// background agents in its own mode, and list them in the Processes
    /// tab.
    pub(crate) fn register_root(
        &self,
        agent: AgentId,
        name: &str,
        folder: PathBuf,
        config: &AgentLoopConfig,
    ) {
        if let Some(agents) = &self.agents {
            let mode = self.mode_of(agent).agent_mode();
            let cache_salt = self.factory.cache_salt_of(agent);
            let run_scratch = self.factory.run_scratch_of(agent);
            let turn = crate::event_loop::host::RootTurn {
                config,
                mode,
                cache_salt: cache_salt.as_deref(),
                run_scratch: run_scratch.as_ref(),
            };
            agents.register(agent, name, folder, &turn, self.notices.clone());
            self.processes.add_root(agent);
        }
    }

    /// The receiving end of [`Self::notices`], for the event loop. Later
    /// calls get a receiver that never yields.
    pub(crate) fn take_notices(&self) -> mpsc::UnboundedReceiver<Notice> {
        lock(&self.notice_rx)
            .take()
            .unwrap_or_else(|| mpsc::unbounded_channel().1)
    }

    /// Start collecting mail between agents for the INBOX tab: the host's
    /// messages and the `task` calls this hub dispatches. Returns where it
    /// arrives, for the event loop. A later call replaces the earlier
    /// channel. Surfaces that never call it (`aura -p`) collect none, so
    /// nothing piles up unread.
    pub(crate) fn open_mail(&self) -> mpsc::UnboundedReceiver<MailItem> {
        let (tx, rx) = mpsc::unbounded_channel();
        if let Some(agents) = &self.agents {
            agents
                .host()
                .set_mail_observer(Arc::new(Mailbox::new(tx.clone())));
        }
        *lock(&self.mail) = Some(tx);
        rx
    }

    /// Show `view` in the INBOX tab, if mail is collected. A `task`
    /// thread is titled when its subtask is named ([`Self::name_subtask`]),
    /// so its mail asks for no subject.
    pub(crate) fn post_mail(&self, view: MailView) {
        if let Some(mail) = lock(&self.mail).as_ref() {
            // A closed channel means the event loop ended; nobody is left
            // to show it.
            let _ = mail.send(MailItem {
                view,
                title_from: None,
            });
        }
    }

    /// Whether mail is collected for the INBOX tab ([`Self::open_mail`]).
    pub(super) fn mail_open(&self) -> bool {
        lock(&self.mail).is_some()
    }

    /// Have the title model write a subject for INBOX thread `thread_id`,
    /// opened by `sender` with `text`, through the sender's kernel with
    /// the terminal's credentials. Skipped under a benchmark's run, whose
    /// agents make no model calls but the ones it measures, and when the
    /// sender has no work in flight: its kernel is only reachable then,
    /// and the thread keeps the subject made from `text`.
    pub(in crate::event_loop) fn title_thread(
        &self,
        sender: AgentId,
        thread_id: String,
        text: String,
    ) {
        if self.in_bench(sender) {
            return;
        }
        let Some(kernel) = self.running_kernel(sender) else {
            debug!(%sender, %thread_id, "Thread sender has no running kernel; not titled");
            return;
        };
        // Only the credentials are used; the turn's prompt and tools are
        // not, so the base config does without loading instructions.
        let (config, classifier) = {
            let base = lock(&self.base);
            (base.config.clone(), base.gate.classifier.clone())
        };
        crate::event_loop::titles::spawn_subject(
            self.commands.clone(),
            thread_id,
            aura_agent::KernelModelGateway::new(kernel),
            config,
            crate::event_loop::titles::title_model(classifier),
            text,
        );
    }

    /// Tell the Processes tab what every agent is called.
    pub(crate) fn name_agents(&self) {
        let registry = lock(&self.registry);
        self.processes.name_agents(
            registry
                .agents()
                .iter()
                .map(|agent| (agent.agent_id(), agent.display_name())),
        );
    }

    /// Send the roster rows to the UI.
    pub async fn send_definitions(&self) {
        let summaries = self.definition_summaries();
        let _ = self
            .commands
            .send(UiCommand::SetAgentDefinitions(summaries))
            .await;
    }

    /// Sessions of spawns that finished since the last call.
    pub fn take_finished(&self) -> Vec<AgentSession> {
        std::mem::take(&mut *lock(&self.finished))
    }

    /// Spawns whose work was dropped mid-run since the last call. They no
    /// longer run and have no session anywhere.
    pub(crate) fn take_abandoned(&self) -> Vec<AgentId> {
        std::mem::take(&mut *lock(&self.abandoned))
    }

    /// Note that `agent_id` started work on `kernel` that `cancel` stops.
    /// `steering` takes messages for it while it runs. Returns which start
    /// this is, for [`Self::stop_running_if`].
    pub(crate) fn start_running(
        &self,
        agent_id: AgentId,
        kernel: Arc<Kernel>,
        cancel: CancellationToken,
        steering: Option<Steering>,
    ) -> RunGeneration {
        // New work: whatever failed before is behind it.
        self.status.clear(agent_id);
        let generation = RunGeneration(self.next_generation.fetch_add(1, Ordering::Relaxed));
        lock(&self.running).insert(
            agent_id,
            Running {
                kernel,
                cancel,
                steering,
                subtasks: Vec::new(),
                generation,
            },
        );
        generation
    }

    /// [`Self::start_running`] for a spawn's task, returning the guard
    /// that ends the running if the task's future is dropped first.
    pub(super) fn start_guarded(
        self: &Arc<Self>,
        agent_id: AgentId,
        kernel: Arc<Kernel>,
        cancel: CancellationToken,
    ) -> RunningGuard {
        // Messages for a spawn wait for its task to end: steering it
        // mid-task would change the work its parent is waiting on.
        let generation = self.start_running(agent_id, kernel, cancel, None);
        RunningGuard {
            hub: Arc::downgrade(self),
            agent: agent_id,
            generation,
            armed: true,
        }
    }

    /// Note that the work `parent` is doing dispatched `attempt`. Returns
    /// the subtasks of the same kind and prompt it dispatched before that
    /// have ended and not been redone yet; those still running are marked
    /// to be redone when they end. Nothing is kept for an agent with no
    /// work in flight.
    pub(super) fn begin_attempt(&self, parent: AgentId, attempt: Attempt) -> Vec<Redo> {
        let mut running = lock(&self.running);
        let Some(work) = running.get_mut(&parent) else {
            return Vec::new();
        };
        let mut redos = Vec::new();
        for earlier in &mut work.subtasks {
            if earlier.redone
                || earlier.kind != attempt.kind
                || !aura_gate::evidence::outcome::is_repeat(&earlier.prompt, &attempt.prompt)
            {
                continue;
            }
            earlier.redone = true;
            if let Some(ended) = &earlier.ended {
                redos.push(Redo {
                    kernel: Arc::clone(&earlier.kernel),
                    child: earlier.child,
                    ended: ended.clone(),
                });
            }
        }
        work.subtasks.push(attempt);
        redos
    }

    /// Note that `parent`'s subtask on `child` ended with `ended`. Returns
    /// it as a [`Redo`] when the parent already dispatched it again.
    pub(super) fn end_attempt(
        &self,
        parent: AgentId,
        child: AgentId,
        ended: Option<OutcomePayload>,
    ) -> Option<Redo> {
        let mut running = lock(&self.running);
        let attempt = running
            .get_mut(&parent)?
            .subtasks
            .iter_mut()
            .find(|attempt| attempt.child == child)?;
        attempt.ended.clone_from(&ended);
        let ended = ended.filter(|_| attempt.redone)?;
        Some(Redo {
            kernel: Arc::clone(&attempt.kernel),
            child,
            ended,
        })
    }

    /// Input for the turn `agent_id` is running, if it is running one.
    pub(in crate::event_loop) fn steering(&self, agent_id: AgentId) -> Option<Steering> {
        lock(&self.running)
            .get(&agent_id)
            .and_then(|running| running.steering.clone())
    }

    /// Note that `agent_id` has no work in flight any more. Returns
    /// whether that work was cancelled.
    pub(crate) fn stop_running(&self, agent_id: AgentId) -> bool {
        self.gate.forget_model(agent_id);
        lock(&self.running)
            .remove(&agent_id)
            .is_some_and(|running| running.cancel.is_cancelled())
    }

    /// Note that the work [`Self::start_running`] started as `generation`
    /// on `agent_id` is no longer in flight. Work started on it since is
    /// left running. Returns whether an entry was removed.
    pub(super) fn stop_running_if(&self, agent_id: AgentId, generation: RunGeneration) -> bool {
        let removed = match lock(&self.running).entry(agent_id) {
            Entry::Occupied(entry) if entry.get().generation == generation => {
                entry.remove();
                true
            }
            _ => false,
        };
        if removed {
            self.gate.forget_model(agent_id);
        }
        removed
    }

    /// The top-level agents with work in flight here, not the spawns
    /// running for them: what a restart cuts short.
    pub(crate) fn running_agents(&self) -> Vec<AgentId> {
        let running: Vec<AgentId> = lock(&self.running).keys().copied().collect();
        let registry = lock(&self.registry);
        running
            .into_iter()
            .filter(|agent| registry.find(*agent).is_some_and(|a| a.parent.is_none()))
            .collect()
    }

    /// Whether `agent_id` has work in flight.
    pub(crate) fn is_running(&self, agent_id: AgentId) -> bool {
        lock(&self.running).contains_key(&agent_id)
    }

    /// Whether `agent_id` is working, here ([`Self::is_running`]) or on
    /// a cloud machine: what its peers and the agent lists are told.
    pub(crate) fn is_working(&self, agent_id: AgentId) -> bool {
        self.is_running(agent_id)
            || lock(&self.cloud_turns).contains(&agent_id)
            || lock(&self.machine_working).contains(&agent_id)
    }

    /// Every agent [`Self::is_working`].
    pub(super) fn working(&self) -> HashSet<AgentId> {
        let mut working: HashSet<AgentId> = lock(&self.running).keys().copied().collect();
        working.extend(lock(&self.cloud_turns).iter().copied());
        working.extend(lock(&self.machine_working).iter().copied());
        working
    }

    /// Kernel of `agent_id` while it has work in flight.
    pub(in crate::event_loop) fn running_kernel(&self, agent_id: AgentId) -> Option<Arc<Kernel>> {
        lock(&self.running)
            .get(&agent_id)
            .map(|running| Arc::clone(&running.kernel))
    }

    /// Stop the work of `agent_id`. Returns whether it had any.
    pub(in crate::event_loop) fn cancel(&self, agent_id: AgentId) -> bool {
        lock(&self.running)
            .get(&agent_id)
            .map(|running| running.cancel.cancel())
            .is_some()
    }

    /// Stop the work of every agent.
    pub(in crate::event_loop) fn cancel_all(&self) {
        for running in lock(&self.running).values() {
            running.cancel.cancel();
        }
    }
}
