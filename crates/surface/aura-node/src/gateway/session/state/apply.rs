//! Applying a chat [`RuntimeRequest`] to a [`Session`].
//!
//! The steps run in the same order as the original single method so a
//! validation error leaves the session in the same partially-applied
//! state as before.

use super::wire::{
    agent_permissions_from_wire, build_intent_classifier, build_typed_chat_system_prompt,
    is_managed_safe_workspace_path, lexical_normalize, thinking_effort_from_wire,
};
use super::{context_window_for_model, Session};
use aura_core_protocol::runtime_request::{
    AgentCapabilities, AgentIdentity as WireAgentIdentity, ModelSelection, ProjectContext,
    WorkspaceLocation,
};
use aura_core_protocol::{
    ConversationMessage, IntentClassifierSpec, RuntimeRequest, RuntimeRequestType,
};
use aura_core_types::AgentId;
use aura_model::Message;
use std::path::{Path, PathBuf};
use std::sync::Arc;

impl Session {
    /// Apply a [`RuntimeRequest`] carrying a [`RuntimeRequestType::Chat`]
    /// variant to configure this chat session.
    ///
    /// Returns `Err(...)` when the request is not a Chat variant
    /// (DevLoop / TaskRun never construct a chat Session) or when
    /// workspace / project_path validation fails.
    pub(crate) fn apply_chat_runtime_request(
        &mut self,
        request: RuntimeRequest,
    ) -> Result<(), String> {
        let RuntimeRequest {
            r#type,
            agent_identity,
            mut model,
            workspace,
            project,
            agent_permissions,
            tool_permissions,
            mut agent_capabilities,
            auth_jwt,
            user_id,
            mode,
        } = request;

        let conversation_messages = chat_conversation_messages(r#type)?;

        if user_id.trim().is_empty() {
            return Err("user_id is required".into());
        }
        self.mode = aura_config::session_mode(mode);

        self.apply_typed_prompt(&agent_identity, project.as_ref());
        let provider_overrides = model.provider_overrides.take();
        self.apply_model_selection(model);
        let intent_classifier = agent_capabilities.intent_classifier.take();
        self.apply_capabilities(agent_capabilities);
        self.apply_workspace(workspace)?;
        if let Some(token) = auth_jwt {
            self.auth_token = Some(token);
        }
        self.apply_agent_identity(agent_identity);
        self.user_id = user_id;
        self.tool_permissions =
            tool_permissions.map(aura_core_protocol::agent_tool_permissions_from_wire);

        self.apply_project_context(project);
        if let Some(provider_overrides) = provider_overrides {
            self.prompt_cache_key = provider_overrides.prompt_cache_key.clone();
            self.prompt_cache_retention = provider_overrides.prompt_cache_retention.clone();
            self.provider_overrides = Some(provider_overrides);
        }
        self.apply_intent_classifier(intent_classifier);

        self.agent_permissions = agent_permissions_from_wire(agent_permissions);
        self.synthesize_computer_use_capability();
        self.push_conversation(conversation_messages);
        self.initialized = true;
        Ok(())
    }

    fn push_conversation(&mut self, conversation_messages: Vec<ConversationMessage>) {
        for msg in conversation_messages {
            match msg.role.as_str() {
                "user" => self.messages.push(Message::user(&msg.content)),
                "assistant" => self.messages.push(Message::assistant(&msg.content)),
                _ => {}
            }
        }
    }

    fn apply_typed_prompt(
        &mut self,
        agent_identity: &WireAgentIdentity,
        project: Option<&ProjectContext>,
    ) {
        let project_info_ref = project.and_then(|p| p.project_info.as_ref());
        let typed_prompt = build_typed_chat_system_prompt(
            agent_identity.persona.as_ref(),
            &agent_identity.skills,
            agent_identity.system_prompt.as_deref(),
            project_info_ref,
        );
        match typed_prompt {
            Some(prompt) => {
                self.system_prompt = prompt;
                self.typed_chat_prompt = true;
            }
            None => {
                self.typed_chat_prompt = false;
            }
        }
    }

    fn apply_model_selection(&mut self, model: ModelSelection) {
        if let Some(model_id) = model.id {
            self.context_window_tokens = context_window_for_model(&model_id);
            self.model = model_id;
        }
        if let Some(temperature) = model.temperature {
            self.temperature = Some(temperature);
        }
        if let Some(effort) = model.reasoning_effort {
            // Map the typed wire tier onto the reasoner's internal
            // effort enum. `None` (field absent) leaves the agent loop
            // on its internal effort heuristic.
            self.user_thinking_effort = Some(thinking_effort_from_wire(effort));
        }
        if let Some(max_turns) = model.max_turns {
            self.max_turns = max_turns;
        }
    }

    fn apply_capabilities(&mut self, agent_capabilities: AgentCapabilities) {
        self.installed_tools = agent_capabilities
            .installed_tools
            .into_iter()
            .map(aura_core_protocol::installed_tool_to_core)
            .collect();
        self.installed_integrations = agent_capabilities
            .installed_integrations
            .into_iter()
            .map(aura_core_protocol::installed_integration_to_core)
            .collect();
        self.computer_use = agent_capabilities.computer_use;
        self.computer_executor_url = agent_capabilities
            .computer_executor_url
            .filter(|u| !u.trim().is_empty());
    }

    fn apply_workspace(&mut self, workspace: WorkspaceLocation) -> Result<(), String> {
        if let Some(ws) = workspace.workspace {
            let candidate = PathBuf::from(&ws);
            if has_parent_dir(&candidate) {
                return Err("workspace path must not contain '..' components".into());
            }
            let normalized = lexical_normalize(&candidate);
            let normalized_base = lexical_normalize(&self.workspace_base);
            if !normalized.starts_with(&normalized_base) {
                return Err(format!(
                    "workspace path must be under {}",
                    self.workspace_base.display()
                ));
            }
            self.workspace = candidate;
        }
        if let Some(ref pp) = workspace.project_path {
            self.apply_project_path(PathBuf::from(pp))?;
        }
        Ok(())
    }

    fn apply_project_path(&mut self, candidate: PathBuf) -> Result<(), String> {
        if !candidate.is_absolute() {
            return Err("project_path must be an absolute path".into());
        }
        if has_parent_dir(&candidate) {
            return Err("project_path must not contain '..' components".into());
        }
        if let Some(ref base) = self.project_base {
            let normalized = lexical_normalize(&candidate);
            let normalized_base = lexical_normalize(base);
            if !normalized.starts_with(&normalized_base)
                && !is_managed_safe_workspace_path(&self.workspace_base, &normalized)
            {
                return Err(format!("project_path must be under {}", base.display()));
            }
        }
        self.project_path = Some(candidate);
        Ok(())
    }

    fn apply_agent_identity(&mut self, agent_identity: WireAgentIdentity) {
        if let Some(partition_id) = agent_identity.partition_id {
            self.skill_agent_id = Some(
                agent_identity
                    .template_id
                    .as_ref()
                    .filter(|id| !id.trim().is_empty())
                    .cloned()
                    .unwrap_or_else(|| partition_id.clone()),
            );
            self.agent_id = AgentId::from_hex(&partition_id).unwrap_or_else(|_| {
                let hash = blake3::hash(partition_id.as_bytes());
                AgentId::new(*hash.as_bytes())
            });
        } else if let Some(template_id) = agent_identity.template_id {
            if !template_id.trim().is_empty() {
                self.skill_agent_id = Some(template_id);
            }
        }
    }

    /// Project context (project_id + billing IDs + intent classifier carrier).
    fn apply_project_context(&mut self, project: Option<ProjectContext>) {
        if let Some(project_ctx) = project {
            self.project_id = Some(project_ctx.project_id);
            if let Some(id) = project_ctx.aura_agent_id {
                self.aura_agent_id = Some(id);
            }
            if let Some(id) = project_ctx.aura_session_id {
                self.aura_session_id = Some(id);
            }
            if let Some(id) = project_ctx.aura_org_id {
                self.aura_org_id = Some(id);
            }
        }
    }

    fn apply_intent_classifier(&mut self, spec: Option<IntentClassifierSpec>) {
        if let Some(spec) = spec {
            let (classifier, manifest) = build_intent_classifier(spec);
            self.intent_classifier = Some(Arc::new(classifier));
            self.intent_classifier_manifest = manifest;
        }
    }

    /// Computer-use arrives as a separate boolean (not a wire
    /// capability), so synthesize the `ComputerUse` capability into
    /// the bundle when the run opted in. This makes the catalog's
    /// capability-gated `computer` tool visible + lets the kernel
    /// policy gate accept it.
    fn synthesize_computer_use_capability(&mut self) {
        if self.computer_use
            && !self
                .agent_permissions
                .capabilities
                .iter()
                .any(|c| matches!(c, aura_core_types::Capability::ComputerUse))
        {
            let insert_at = self
                .agent_permissions
                .capabilities
                .iter()
                .position(|c| {
                    matches!(
                        c,
                        aura_core_types::Capability::ReadAllProjects
                            | aura_core_types::Capability::WriteAllProjects
                    )
                })
                .unwrap_or(self.agent_permissions.capabilities.len());
            self.agent_permissions
                .capabilities
                .insert(insert_at, aura_core_types::Capability::ComputerUse);
        }
    }
}

/// Unwrap the Chat variant's conversation history; every other run
/// kind is rejected because it never opens a chat session.
fn chat_conversation_messages(
    request_type: RuntimeRequestType,
) -> Result<Vec<ConversationMessage>, String> {
    match request_type {
        RuntimeRequestType::Chat {
            conversation_messages,
        } => Ok(conversation_messages),
        RuntimeRequestType::DevLoop {}
        | RuntimeRequestType::TaskRun { .. }
        | RuntimeRequestType::Council { .. } => Err(
            "session apply only accepts RuntimeRequestType::Chat — DevLoop / TaskRun / \
             Council runs do not open a chat session"
                .to_string(),
        ),
    }
}

fn has_parent_dir(path: &Path) -> bool {
    path.components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
}
