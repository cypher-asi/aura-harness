//! Reloading the live model lists and probing the local server.

use super::*;

impl ModelSelection {
    /// Reload the model lists of the linked Claude plan and OpenRouter
    /// account, concurrently. Returns a user-facing note (one line per
    /// provider) when a linked provider's list could not be loaded; the
    /// list saved when it last loaded is used then, else the built-in one.
    pub async fn refresh_plans(&mut self, resolver: &dyn CredentialResolver) -> Option<String> {
        let claude = self.auth.can_use(ProviderKind::Claude);
        let openrouter = self.auth.can_use(ProviderKind::OpenRouter);
        let (claude_listed, openrouter_listed) = tokio::join!(
            async {
                if claude {
                    let listed = aura_model::plan_models::fetch_claude_models(resolver).await;
                    Some(listed.map_err(|e| e.to_string()))
                } else {
                    None
                }
            },
            async {
                if openrouter {
                    let listed = aura_model::plan_models::fetch_openrouter_models(resolver).await;
                    Some(listed.map_err(|e| e.to_string()))
                } else {
                    None
                }
            },
        );
        let claude_note = match claude_listed {
            Some(listed) => self.plans_listed(listed),
            None => {
                self.set_backend_models(ModelBackend::Claude, Vec::new());
                None
            }
        };
        let openrouter_note = match openrouter_listed {
            Some(listed) => self.openrouter_listed(listed),
            None => {
                self.set_backend_models(ModelBackend::OpenRouter, Vec::new());
                None
            }
        };
        let notes: Vec<String> = [claude_note, openrouter_note]
            .into_iter()
            .flatten()
            .collect();
        (!notes.is_empty()).then(|| notes.join("\n"))
    }

    /// Replace the live models of `backend` with `models`; an empty list
    /// means the built-in one.
    pub(super) fn set_backend_models(&mut self, backend: ModelBackend, models: Vec<ModelOption>) {
        self.plan_models.retain(|m| m.backend != backend);
        self.plan_models.extend(models);
    }

    /// Path beside `tui.json` named `file`, `None` without a prefs path.
    pub(super) fn saved_path(&self, file: &str) -> Option<PathBuf> {
        self.prefs_path
            .as_ref()
            .map(|prefs| prefs.with_file_name(file))
    }

    /// Take in the Claude plan's model list as `listed` just now, or why
    /// it could not be. Returns the Claude note of [`Self::refresh_plans`].
    pub(super) fn plans_listed(
        &mut self,
        listed: Result<Vec<ModelOption>, String>,
    ) -> Option<String> {
        let saved_at = self.saved_path(PLAN_MODELS_FILE);
        match listed {
            Ok(models) => {
                if let Some(path) = &saved_at {
                    save_plan_models(path, &models);
                }
                self.set_backend_models(ModelBackend::Claude, models);
                None
            }
            Err(problem) => {
                warn!(error = %problem, "Failed to list Claude plan models");
                let saved = saved_at
                    .as_deref()
                    .map(load_plan_models)
                    .unwrap_or_default();
                let shown = shown_list(saved.is_empty());
                self.set_backend_models(ModelBackend::Claude, saved);
                Some(format!(
                    "Couldn't load your Claude plan's model list ({problem}); {shown}. Any \
                     model works with /model claude/<model-id>."
                ))
            }
        }
    }

    /// Take in the OpenRouter account's model list as `listed` just now,
    /// or why it could not be, and publish the limits and prices it gives.
    /// Returns the OpenRouter note of [`Self::refresh_plans`].
    pub(super) fn openrouter_listed(
        &mut self,
        listed: Result<Vec<(ModelOption, LiveModelInfo)>, String>,
    ) -> Option<String> {
        let saved_at = self.saved_path(OPENROUTER_MODELS_FILE);
        let (entries, note) = match listed {
            Ok(entries) => {
                if let Some(path) = &saved_at {
                    save_openrouter_models(path, &entries);
                }
                (entries, None)
            }
            Err(problem) => {
                warn!(error = %problem, "Failed to list OpenRouter models");
                let saved = saved_at
                    .as_deref()
                    .map(load_openrouter_models)
                    .unwrap_or_default();
                let shown = shown_list(saved.is_empty());
                let note = format!(
                    "Couldn't load your OpenRouter account's model list ({problem}); {shown}. \
                     Any model works with /model openrouter/<vendor>/<model>."
                );
                (saved, Some(note))
            }
        };
        if note.is_none() || !entries.is_empty() {
            publish_openrouter_info(&entries);
        }
        let models = entries.into_iter().map(|(option, _)| option).collect();
        self.set_backend_models(ModelBackend::OpenRouter, models);
        note
    }

    /// Re-probe the local model server.
    pub async fn refresh_local(&mut self) {
        self.local_status = model_catalog::probe_local_server(&self.local_url).await;
    }

    pub(super) fn installed_local(&self) -> &[String] {
        match &self.local_status {
            LocalServerStatus::Ready { models, .. } => models,
            _ => &[],
        }
    }
}
