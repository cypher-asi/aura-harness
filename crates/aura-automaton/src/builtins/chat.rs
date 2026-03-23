//! Chat automaton – handles a single chat round-trip.
//!
//! Replaces `ChatService::send_message_streaming()` from `aura-app`.
//! This is an on-demand automaton: each tick runs one chat interaction
//! (system prompt + messages → agent loop → result) then returns `Done`.

use std::sync::Arc;

use tracing::{error, info};

use aura_agent::agent_runner::{AgentRunner, AgentRunnerConfig};
use aura_agent::prompts::ProjectInfo;
use aura_reasoner::{Message, ModelProvider};
use aura_tools::definitions::agent_tool_definitions;
use aura_tools::domain_tools::{DomainApi, MessageDescriptor, SaveMessageParams};

use crate::context::TickContext;
use crate::error::AutomatonError;
use crate::events::AutomatonEvent;
use crate::runtime::{Automaton, TickOutcome};
use crate::schedule::Schedule;

pub struct ChatAutomaton {
    domain: Arc<dyn DomainApi>,
    provider: Arc<dyn ModelProvider>,
    runner: AgentRunner,
}

impl ChatAutomaton {
    pub fn new(
        domain: Arc<dyn DomainApi>,
        provider: Arc<dyn ModelProvider>,
        config: AgentRunnerConfig,
    ) -> Self {
        Self {
            domain,
            provider,
            runner: AgentRunner::new(config),
        }
    }
}

struct ChatConfig {
    project_id: String,
    instance_id: String,
    custom_system_prompt: String,
}

impl ChatConfig {
    fn from_json(config: &serde_json::Value) -> Result<Self, AutomatonError> {
        let project_id = config
            .get("project_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AutomatonError::InvalidConfig("missing project_id".into()))?
            .to_string();
        let instance_id = config
            .get("agent_instance_id")
            .and_then(|v| v.as_str())
            .unwrap_or("default")
            .to_string();
        let custom_system_prompt = config
            .get("system_prompt")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        Ok(Self {
            project_id,
            instance_id,
            custom_system_prompt,
        })
    }
}

#[async_trait::async_trait]
impl Automaton for ChatAutomaton {
    fn kind(&self) -> &str {
        "chat"
    }

    fn default_schedule(&self) -> Schedule {
        Schedule::OnDemand
    }

    async fn tick(&self, ctx: &mut TickContext) -> Result<TickOutcome, AutomatonError> {
        let cfg = ChatConfig::from_json(&ctx.config)?;

        ctx.emit(AutomatonEvent::Progress {
            message: "Loading conversation...".into(),
        });

        // ------------------------------------------------------------------
        // 1. Fetch messages
        // ------------------------------------------------------------------
        let stored = self
            .domain
            .list_messages(&cfg.project_id, &cfg.instance_id)
            .await
            .map_err(|e| AutomatonError::DomainApi(format!("list_messages: {e}")))?;

        if stored.is_empty() {
            ctx.emit(AutomatonEvent::Error {
                automaton_id: ctx.automaton_id.to_string(),
                message: "No messages to process".into(),
            });
            return Ok(TickOutcome::Done);
        }

        // ------------------------------------------------------------------
        // 2. Build context
        // ------------------------------------------------------------------
        ctx.emit(AutomatonEvent::Progress {
            message: "Building context...".into(),
        });

        let project = self
            .domain
            .get_project(&cfg.project_id)
            .await
            .map_err(|e| AutomatonError::DomainApi(e.to_string()))?;

        let project_info = ProjectInfo {
            name: &project.name,
            description: project.description.as_deref().unwrap_or(""),
            folder_path: &project.path,
            build_command: project.build_command.as_deref(),
            test_command: project.test_command.as_deref(),
        };

        let api_messages = convert_descriptors_to_messages(&stored);
        let tools = agent_tool_definitions().to_vec();

        // ------------------------------------------------------------------
        // 3. Run agent loop
        // ------------------------------------------------------------------
        ctx.emit(AutomatonEvent::Progress {
            message: "Waiting for response...".into(),
        });

        let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel();
        let automaton_tx = ctx.event_tx.clone();
        tokio::spawn(async move {
            while let Some(evt) = event_rx.recv().await {
                super::dev_loop::forward_agent_event(&automaton_tx, evt);
            }
        });

        let cancel = ctx.cancellation_token().clone();

        let executor = NoOpChatExecutor;

        let result = self
            .runner
            .execute_chat(
                self.provider.as_ref(),
                &executor,
                &project_info,
                &cfg.custom_system_prompt,
                api_messages,
                tools,
                Some(event_tx),
                Some(cancel),
            )
            .await
            .map_err(|e| AutomatonError::AgentExecution(e.to_string()))?;

        info!(
            input_tokens = result.total_input_tokens,
            output_tokens = result.total_output_tokens,
            iterations = result.iterations,
            "chat loop finished"
        );

        // ------------------------------------------------------------------
        // 4. Save assistant message
        // ------------------------------------------------------------------
        if !result.total_text.is_empty() {
            let session = self
                .domain
                .get_active_session(&cfg.instance_id)
                .await
                .ok()
                .flatten();
            let session_id = session.map(|s| s.id).unwrap_or_default();

            if let Err(e) = self
                .domain
                .save_message(SaveMessageParams {
                    project_id: cfg.project_id.clone(),
                    instance_id: cfg.instance_id.clone(),
                    session_id,
                    role: "assistant".into(),
                    content: result.total_text.clone(),
                })
                .await
            {
                error!(error = %e, "failed to save assistant message");
            }

            ctx.emit(AutomatonEvent::MessageSaved {
                message_id: String::new(),
            });
        }

        ctx.emit(AutomatonEvent::TokenUsage {
            input_tokens: result.total_input_tokens,
            output_tokens: result.total_output_tokens,
        });

        Ok(TickOutcome::Done)
    }
}

/// Convert `MessageDescriptor`s from `DomainApi` into `aura_reasoner::Message`s.
fn convert_descriptors_to_messages(descriptors: &[MessageDescriptor]) -> Vec<Message> {
    descriptors
        .iter()
        .filter_map(|d| {
            let msg = match d.role.as_str() {
                "user" => Message::user(&d.content),
                "assistant" => Message::assistant(&d.content),
                _ => return None,
            };
            Some(msg)
        })
        .collect()
}

struct NoOpChatExecutor;

#[async_trait::async_trait]
impl aura_agent::types::AgentToolExecutor for NoOpChatExecutor {
    async fn execute(
        &self,
        tool_calls: &[aura_agent::types::ToolCallInfo],
    ) -> Vec<aura_agent::types::ToolCallResult> {
        tool_calls
            .iter()
            .map(|tc| {
                aura_agent::types::ToolCallResult::error(
                    &tc.id,
                    "tool execution not configured for this chat session",
                )
            })
            .collect()
    }
}
