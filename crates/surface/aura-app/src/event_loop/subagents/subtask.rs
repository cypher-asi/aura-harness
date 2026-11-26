use super::*;

impl AgentHub {
    /// Run `subtask` to its end: wait for its workspace when it writes
    /// there, run its first attempt and, when Gate routed it and the
    /// attempt failed, put back the files it changed and retry it once on
    /// a stronger model.
    ///
    /// The files its file tools changed are always put back. Changes by
    /// its shell commands are too when the workspace is in a git
    /// repository and the attempt had the workspace to itself: it took
    /// the write lock (so it is no read-only or nested subtask) and its
    /// caller waits for it rather than working on beside it.
    pub(super) async fn run_subtask(self: &Arc<Self>, subtask: Subtask) -> TaskReport {
        let Subtask {
            request,
            definition,
            first,
        } = subtask;
        let workspace = first.session.kernel.agent_workspace();
        let mut slot = self.first_slot(&request, &definition, &first).await;
        let candidates = first.gate.candidates.clone();
        // The view Gate routed the first attempt on: its `/compare` arm's
        // own under a run, so the retry escalates on the same one.
        let live = Arc::clone(&first.gate.live);
        let alone = slot.as_ref().is_some_and(WriteSlot::exclusive)
            && request.spawn_mode != Some(SpawnMode::Detached);
        let snapshot = if alone && candidates.is_some() {
            snapshot_workspace(workspace).await
        } else {
            None
        };
        let deadline = first.deadline;
        let first = self.finish_task(first).await;
        let Some((why, candidates, category)) = retry_reason(&first, candidates, deadline) else {
            return TaskReport { first, retry: None };
        };
        let Some(decision) = escalate_subtask(&live, &first, category, &candidates) else {
            return TaskReport { first, retry: None };
        };
        undo_attempt(&first, snapshot).await;
        let prompt = followup::retry_prompt(&request.prompt, &first.model, &why);
        let retry = match self
            .open_task(&request, &definition, prompt, Some((decision, category)))
            .await
        {
            Ok(retry) => TaskRun { deadline, ..retry },
            Err(problem) => {
                warn!(%problem, "Could not retry the subtask");
                return TaskReport { first, retry: None };
            }
        };
        if let Some(slot) = &mut slot {
            slot.hold(retry.session.agent_id());
        }
        let retry = self.finish_task(retry).await;
        TaskReport {
            first,
            retry: Some((retry, why)),
        }
    }

    /// The write slot `first`, the first attempt at `request`, holds, or
    /// `None` when `definition` only reads.
    async fn first_slot(
        &self,
        request: &SubagentDispatchRequest,
        definition: &AgentDefinition,
        first: &TaskRun,
    ) -> Option<WriteSlot<'_>> {
        let mut slot = if reads_only(definition) {
            None
        } else {
            Some(
                self.write_slot(
                    request,
                    first.session.kernel.agent_workspace(),
                    &first.cancel,
                    &AgentUi::new(self.commands.clone(), first.session.agent_id()),
                )
                .await,
            )
        };
        if let Some(slot) = &mut slot {
            slot.hold(first.session.agent_id());
        }
        slot
    }

    /// The turn of a write subtask for `request` in `workspace`: write
    /// subtasks in one workspace run one at a time, except those
    /// dispatched by one that holds it. Cancelling stops the wait.
    pub(super) async fn write_slot(
        &self,
        request: &SubagentDispatchRequest,
        workspace: PathBuf,
        cancel: &CancellationToken,
        ui: &AgentUi,
    ) -> WriteSlot<'_> {
        let nested = {
            let writing = lock(&self.writing);
            std::iter::once(&request.parent_agent_id)
                .chain(&request.parent_chain)
                .any(|agent| writing.contains(agent))
        };
        let guard = if nested {
            None
        } else {
            let mutex = Arc::clone(lock(&self.write_locks).entry(workspace).or_default());
            if let Ok(guard) = Arc::clone(&mutex).try_lock_owned() {
                Some(guard)
            } else {
                // The agent is listed as working from here on, but its turn
                // (and so its chat and record) starts only once it has the
                // workspace. Say so rather than show an empty chat.
                ui.send(system_message(
                    "Queued: another agent is editing this workspace. \
                     This agent starts when it finishes."
                        .to_string(),
                ))
                .await;
                tokio::select! {
                    guard = mutex.lock_owned() => Some(guard),
                    () = cancel.cancelled() => None,
                }
            }
        };
        WriteSlot {
            hub: self,
            guard,
            holders: Vec::new(),
        }
    }

    /// How Gate handles the subtask `request` gives the permanent agent
    /// `definition`, set up with `config`. A model the call names is
    /// applied to `config` here and pins the subtask, as do a council
    /// seat, a definition naming its own model, and a pinned main model.
    /// Under a `/compare` run, the run's arm decides instead of the
    /// user's selection.
    pub(super) fn subtask_gate(
        &self,
        request: &SubagentDispatchRequest,
        definition: &AgentDefinition,
        config: &mut AgentLoopConfig,
    ) -> SubtaskGate {
        let arm = self.compare_arm(request.parent_agent_id);
        let live = arm
            .as_ref()
            .map_or_else(|| Arc::clone(&self.gate), |arm| Arc::clone(&arm.gate));
        let base = lock(&self.base);
        let explicit = explicit_model(request);
        if let Some(model) = explicit {
            base.limits.configure(config, model);
        }
        let mut pinned =
            explicit.is_some() || request.council_index.is_some() || definition.model.is_some();
        let mut candidates = base.gate.candidates.clone();
        let mut explore = true;
        let mut table = None;
        match arm.map(|arm| arm.routing) {
            Some(ArmRouting::Pinned(model)) => {
                if !pinned {
                    base.limits.configure(config, &model);
                }
                pinned = true;
            }
            Some(ArmRouting::Gate {
                candidates: arm_candidates,
                ..
            }) => {
                candidates = Some(arm_candidates);
                explore = false;
            }
            Some(ArmRouting::ByDifficulty(routes)) => {
                // The arm's table routes the subtask, so Gate neither
                // picks among candidates nor retries a failed attempt.
                candidates = None;
                explore = false;
                table = (!pinned).then_some(routes);
            }
            None => {}
        }
        SubtaskGate {
            kind: definition.name.clone(),
            classifier: base.gate.classifier.clone(),
            candidates: if pinned { None } else { candidates },
            lead_model: live.model_of(request.parent_agent_id),
            limits: base.limits,
            live,
            explore,
            escalation: None,
            table,
        }
    }
}

/// Why `first` is retried, with the `candidates` and prompt category to
/// route the retry by; `None` when it is not: it went well, was
/// cancelled, the judge excused it, Gate did not route it or too little
/// time is left before `deadline`.
fn retry_reason(
    first: &TaskDone,
    candidates: Option<Vec<Candidate>>,
    deadline: Option<Instant>,
) -> Option<(String, Vec<Candidate>, PromptCategory)> {
    let failure = first
        .attempted()
        .failure()
        .or_else(|| first.unfinished.clone());
    let mut why = failure.filter(|_| !first.cancelled)?;
    match &first.judged {
        // Not the model's doing: a stronger model would fail the
        // same way.
        Some(verdict) if !verdict.at_fault => {
            info!(child = %first.name, "The judge excused the subtask's failure; not retried");
            return None;
        }
        Some(verdict) if !verdict.reasons.trim().is_empty() => {
            why.push_str("\nA reviewer of that attempt found: ");
            why.push_str(verdict.reasons.trim());
        }
        _ => {}
    }
    let candidates = candidates?;
    let category = first.category?;
    if deadline.is_some_and(|at| at.saturating_duration_since(Instant::now()) < WRAP_UP_BEFORE * 2)
    {
        info!(child = %first.name, "Too little time left to retry the subtask");
        return None;
    }
    Some((why, candidates, category))
}

/// The stronger model `live` retries `first` on, a subtask of
/// `category`, among `candidates`.
fn escalate_subtask(
    live: &GateLive,
    first: &TaskDone,
    category: PromptCategory,
    candidates: &[Candidate],
) -> Option<Decision> {
    // Only work the judge found unfinished is done again on the
    // strongest model; a failed check or an error there is reported.
    let signal = if first.unfinished.is_some() {
        OutcomeSignal::JudgedFail
    } else if first.check.is_some() {
        OutcomeSignal::ChecksFailed
    } else {
        OutcomeSignal::Error
    };
    let escalated = live.escalate(
        RouteLevel::Subtask,
        category,
        (&first.model, first.effort),
        signal,
        candidates,
    );
    if escalated.is_none() {
        info!(child = %first.name, "No stronger model to retry the subtask on");
    }
    escalated
}

/// Put back what `first` changed: the files its file tools changed and,
/// with `snapshot`, the rest of the workspace.
async fn undo_attempt(first: &TaskDone, snapshot: Option<changes::Snapshot>) {
    if let Some(files) = &first.files_before {
        let report = files.restore();
        for (path, problem) in &report.failed {
            warn!(path = %path.display(), %problem, "Could not undo a failed subtask's change");
        }
    }
    if let Some(snapshot) = snapshot {
        restore_workspace(snapshot).await;
    }
}
