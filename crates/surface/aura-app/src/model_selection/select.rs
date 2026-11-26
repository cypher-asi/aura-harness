//! Selector entries, switching models and the thinking level, and persisting the choice.

use super::*;

impl ModelSelection {
    /// Selector entries, always including the current model.
    pub fn options(&self) -> Vec<ModelOptionView> {
        let mut options = model_catalog::catalog_with_plans(
            &self.default_local,
            self.installed_local(),
            &self.plan_models,
        );
        if !options.iter().any(|o| o.id == self.current) {
            let backend = backend_for(&self.current);
            options.push(ModelOption {
                id: self.current.clone(),
                label: local_model_name(&self.current)
                    .unwrap_or(&self.current)
                    .to_string(),
                backend,
                installed: backend != ModelBackend::Local || self.local_installed(&self.current),
                tier: infer_tier(&self.current),
            });
        }
        let gate = ModelOptionView {
            id: GATE_MODEL_ID.to_string(),
            label: GATE_LABEL.to_string(),
            local: false,
            installed: true,
            group: GATE_LABEL.to_string(),
            provider: None,
            available: true,
            context_window: None,
        };
        std::iter::once(gate)
            .chain(options.into_iter().map(|o| ModelOptionView {
                local: o.backend == ModelBackend::Local,
                group: group_heading(o.backend, &o.id),
                provider: o.required_provider().map(|p| p.id().to_string()),
                available: self.usable(&o.id),
                context_window: Some(aura_engine::model_context::context_window_for_model(&o.id)),
                id: o.id,
                label: o.label,
                installed: o.installed,
            }))
            .collect()
    }

    /// `UiCommand::SetModels` payload for the current state.
    pub fn ui_command(&self) -> aura_ui::UiCommand {
        aura_ui::UiCommand::SetModels {
            options: self.options(),
            current: self.current.clone(),
            route: self
                .is_gate()
                .then(|| model_catalog::route_label(&self.turn_model())),
            user: self.auth.zos_user.clone(),
            plans: self.auth.plan_names.clone(),
            thinking: self
                .thinking
                .map(|effort| thinking_name(effort).to_string()),
        }
    }

    /// Pin the thinking level of every turn to the one named `level`
    /// (`high`, `off`, ...), or let each model pick its own for `None`.
    /// Returns a user-facing problem for a name that is not a level.
    pub fn set_thinking(
        &mut self,
        config: &mut AgentLoopConfig,
        level: Option<&str>,
    ) -> Result<(), String> {
        let effort = match level {
            None => None,
            Some(name) => Some(
                ThinkingEffort::from_wire(name)
                    .ok_or_else(|| format!("Unknown thinking level: {name}"))?,
            ),
        };
        self.thinking = effort;
        config.user_thinking_effort = effort;
        self.persist();
        Ok(())
    }

    /// The pinned thinking level, `auto` when each model picks its own.
    pub fn thinking_display(&self) -> &'static str {
        self.thinking.map_or("auto", thinking_name)
    }

    pub(super) fn local_installed(&self, id: &str) -> bool {
        local_model_name(id)
            .is_some_and(|name| model_catalog::is_installed(self.installed_local(), name))
    }

    /// Switch to `id`. Returns a user-facing problem when the switch is refused.
    pub fn select(&mut self, config: &mut AgentLoopConfig, id: &str) -> Result<(), String> {
        self.use_for_run(config, id)?;
        self.persist();
        Ok(())
    }

    /// Switch to `id` for this process only, leaving the saved choice
    /// alone. `id` may be a short alias (`opus`, `deepseek`), which picks
    /// the model [`Self::resolve`] names; a full model name (see
    /// [`may_be_alias`]) is used as given unless it is a listed label.
    /// Returns a user-facing problem when the switch is refused.
    pub fn use_for_run(&mut self, config: &mut AgentLoopConfig, id: &str) -> Result<(), String> {
        let typed = id.trim();
        if typed.is_empty() {
            return Err("Model id is empty".to_string());
        }
        let id = self.resolve(typed).unwrap_or_else(|| typed.to_string());
        if !self.usable(&id) {
            let reason = unusable_reason(&id);
            return Err(if id.eq_ignore_ascii_case(typed) {
                reason
            } else {
                format!("\"{typed}\" matches {id}: {reason}")
            });
        }
        self.apply(config, &id);
        Ok(())
    }

    /// The model id `query` names among Gate and the listed models: an
    /// exact id, else the best-ranked model with that exact label, else,
    /// when [`may_be_alias`] holds, the best match for a short alias
    /// (`opus`, `sonnet 4.5`, `openrouter opus`), preferring models the
    /// current sign-ins can run. `None` when nothing matches that way
    /// (an unlisted explicit id, a full model name such as
    /// `claude-opus-4-6`, a name that matches nothing); callers then use
    /// `query` as the id.
    pub fn resolve(&self, query: &str) -> Option<String> {
        let catalog = model_catalog::catalog_with_plans(
            &self.default_local,
            self.installed_local(),
            &self.plan_models,
        );
        // Gate first, so its id and label match exactly; the resolver
        // never picks it for a fuzzy query, so its tier does not matter.
        let gate = AliasCandidate {
            id: GATE_MODEL_ID,
            label: GATE_LABEL,
            tier: ModelTier::Frontier,
            available: true,
        };
        let candidates: Vec<AliasCandidate<'_>> = std::iter::once(gate)
            .chain(catalog.iter().map(|option| AliasCandidate {
                id: &option.id,
                label: &option.label,
                tier: option.tier,
                available: self.usable(&option.id),
            }))
            .collect();
        let resolved = if may_be_alias(query) {
            resolve_alias(query, &candidates, &self.current)
        } else {
            resolve_exact(query, &candidates, &self.current)
        };
        resolved.map(str::to_string)
    }

    /// Switch to Gate when the current model needs a sign-in the user no
    /// longer has, and point `config` at Gate's pick for the sign-ins
    /// left. Returns `true` when the selected model changed.
    pub fn ensure_usable(&mut self, config: &mut AgentLoopConfig) -> bool {
        if !self.usable(&self.current) {
            self.apply(config, GATE_MODEL_ID);
            return true;
        }
        self.refresh_route(config);
        false
    }

    /// Hint shown when the next turn's local model can't run, or `None`
    /// when it's ready.
    pub fn local_hint(&self) -> Option<String> {
        let model = self.turn_model();
        let name = local_model_name(&model)?;
        let setup = format!(
            "Start it with `OLLAMA_CONTEXT_LENGTH={} ollama serve`, then `ollama pull {name}`.",
            model_catalog::local_context_tokens()
        );
        match &self.local_status {
            LocalServerStatus::Ready { .. } if self.local_installed(&model) => None,
            LocalServerStatus::Ready { .. } => Some(format!(
                "Local model {name} is not pulled yet. Run `ollama pull {name}`, \
                 or use /login to use cloud models."
            )),
            LocalServerStatus::Outdated { version } => Some(format!(
                "Ollama {version} at {} is too old (Aura needs 0.14+ for the Messages API). \
                 Update Ollama, or use /login to use cloud models.",
                self.local_url
            )),
            LocalServerStatus::Unreachable => Some(format!(
                "No local model server at {}. Install Ollama (https://ollama.com). {setup} \
                 Or use /login to use cloud models.",
                self.local_url
            )),
        }
    }

    /// Display label for `id`.
    pub fn label(id: &str) -> String {
        if id == GATE_MODEL_ID {
            return GATE_LABEL.to_string();
        }
        let backend = backend_for(id).label();
        format!("{} ({backend})", local_model_name(id).unwrap_or(id))
    }

    /// Display label for the selected model, with Gate's pick:
    /// `Gate → claude-sonnet-4-6 (claude plan)`.
    pub fn display(&self) -> String {
        if self.is_gate() {
            format!(
                "{GATE_LABEL} → {}",
                model_catalog::route_label(&self.turn_model())
            )
        } else {
            Self::label(&self.current)
        }
    }

    pub(super) fn apply(&mut self, config: &mut AgentLoopConfig, id: &str) {
        id.clone_into(&mut self.current);
        let model = self.turn_model();
        self.limits().configure(config, &model);
    }

    /// Limits for configuring other agent loops to run a given model.
    pub const fn limits(&self) -> ModelLimits {
        ModelLimits(self.router_limits)
    }

    pub(super) fn persist(&self) {
        let Some(path) = &self.prefs_path else {
            return;
        };
        let prefs = TuiPrefs {
            model: Some(self.current.clone()),
            thinking: self
                .thinking
                .map(|effort| thinking_name(effort).to_string()),
        };
        let result = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| {
                let json = serde_json::to_vec_pretty(&prefs).map_err(std::io::Error::other)?;
                std::fs::write(path, json)
            });
        if let Err(e) = result {
            warn!(error = %e, path = %path.display(), "Failed to persist model selection");
        }
    }
}
