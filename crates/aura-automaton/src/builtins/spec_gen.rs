//! Spec-generation automaton.
//!
//! Replaces `SpecGenerationService::generate_specs_streaming()` from
//! `aura-app`. On-demand: a single tick generates specs for a project's
//! requirements document, saves them via `DomainApi`, and returns `Done`.

use std::sync::Arc;

use tracing::{error, info};

use aura_reasoner::ModelProvider;
use aura_tools::domain_tools::DomainApi;

use crate::context::TickContext;
use crate::error::AutomatonError;
use crate::events::AutomatonEvent;
use crate::runtime::{Automaton, TickOutcome};
use crate::schedule::Schedule;

pub struct SpecGenAutomaton {
    domain: Arc<dyn DomainApi>,
    provider: Arc<dyn ModelProvider>,
}

impl SpecGenAutomaton {
    pub fn new(domain: Arc<dyn DomainApi>, provider: Arc<dyn ModelProvider>) -> Self {
        Self { domain, provider }
    }
}

struct SpecGenConfig {
    project_id: String,
}

impl SpecGenConfig {
    fn from_json(config: &serde_json::Value) -> Result<Self, AutomatonError> {
        let project_id = config
            .get("project_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AutomatonError::InvalidConfig("missing project_id".into()))?
            .to_string();
        Ok(Self { project_id })
    }
}

const SPEC_GENERATION_SYSTEM_PROMPT: &str = r#"You are a software specification writer. Given a requirements document, break it down into clear, actionable technical specifications.

Each specification should:
1. Have a clear title
2. Contain detailed implementation instructions in markdown
3. Be self-contained enough for a developer to implement independently
4. Be ordered logically (foundational specs first)

Output your response as a JSON array where each element has:
- "title": string
- "markdown_contents": string (detailed spec in markdown)

Output ONLY the JSON array, no other text."#;

const MAX_TOKENS: u32 = 32_768;

#[async_trait::async_trait]
impl Automaton for SpecGenAutomaton {
    fn kind(&self) -> &str {
        "spec-gen"
    }

    fn default_schedule(&self) -> Schedule {
        Schedule::OnDemand
    }

    async fn tick(&self, ctx: &mut TickContext) -> Result<TickOutcome, AutomatonError> {
        let cfg = SpecGenConfig::from_json(&ctx.config)?;

        ctx.emit(AutomatonEvent::Progress {
            message: "Loading project...".into(),
        });

        let _project = self
            .domain
            .get_project(&cfg.project_id, None)
            .await
            .map_err(|e| AutomatonError::DomainApi(e.to_string()))?;

        // ------------------------------------------------------------------
        // 1. Load requirements
        // ------------------------------------------------------------------
        ctx.emit(AutomatonEvent::Progress {
            message: "Reading requirements document...".into(),
        });

        let requirements_path = ctx
            .config
            .get("requirements_path")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let requirements = if !requirements_path.is_empty() {
            let workspace_root = ctx
                .workspace_root
                .as_deref()
                .ok_or_else(|| AutomatonError::InvalidConfig("no workspace_root set".into()))?;
            let resolved = workspace_root.join(requirements_path);
            let canonical = resolved.canonicalize().map_err(|e| {
                AutomatonError::InvalidConfig(format!(
                    "failed to resolve requirements path {requirements_path}: {e}"
                ))
            })?;
            let canonical_base = workspace_root.canonicalize().map_err(|e| {
                AutomatonError::InvalidConfig(format!(
                    "failed to canonicalize workspace root: {e}"
                ))
            })?;
            if !canonical.starts_with(&canonical_base) {
                return Err(AutomatonError::InvalidConfig(format!(
                    "requirements_path escapes workspace root: {requirements_path}"
                )));
            }
            tokio::fs::read_to_string(&canonical)
                .await
                .map_err(|e| {
                    AutomatonError::InvalidConfig(format!(
                        "failed to read requirements file {requirements_path}: {e}"
                    ))
                })?
        } else {
            return Err(AutomatonError::InvalidConfig(
                "no requirements_path configured".into(),
            ));
        };

        info!(
            project_id = %cfg.project_id,
            bytes = requirements.len(),
            "requirements loaded"
        );

        // ------------------------------------------------------------------
        // 2. Generate specs via LLM
        // ------------------------------------------------------------------
        ctx.emit(AutomatonEvent::Progress {
            message: "Generating specifications...".into(),
        });

        let model = ctx
            .config
            .get("model")
            .and_then(|v| v.as_str())
            .unwrap_or(aura_core::DEFAULT_MODEL)
            .to_string();

        let request = aura_reasoner::ModelRequest::builder(&model, SPEC_GENERATION_SYSTEM_PROMPT)
            .messages(vec![aura_reasoner::Message::user(&requirements)])
            .max_tokens(MAX_TOKENS)
            .build();

        let response = self
            .provider
            .complete(request)
            .await
            .map_err(|e| AutomatonError::AgentExecution(format!("LLM call failed: {e}")))?;

        ctx.emit(AutomatonEvent::TokenUsage {
            input_tokens: response.usage.input_tokens,
            output_tokens: response.usage.output_tokens,
        });

        // ------------------------------------------------------------------
        // 3. Parse response
        // ------------------------------------------------------------------
        ctx.emit(AutomatonEvent::Progress {
            message: "Parsing AI response...".into(),
        });

        let response_text = response.message.text_content();
        let specs = parse_spec_response(&response_text)?;
        info!(
            project_id = %cfg.project_id,
            count = specs.len(),
            "parsed specs from LLM response"
        );

        // ------------------------------------------------------------------
        // 4. Save specs
        // ------------------------------------------------------------------
        ctx.emit(AutomatonEvent::Progress {
            message: format!("Saving {} specs...", specs.len()),
        });

        // Clear existing specs
        let existing = self
            .domain
            .list_specs(&cfg.project_id, None)
            .await
            .unwrap_or_default();
        for s in &existing {
            if let Err(e) = self.domain.delete_spec(&s.id, None).await {
                error!(spec_id = %s.id, error = %e, "failed to delete old spec");
            }
        }

        for (idx, spec) in specs.iter().enumerate() {
            let saved = self
                .domain
                .create_spec(&cfg.project_id, &spec.title, &spec.content, idx as u32, None)
                .await
                .map_err(|e| AutomatonError::DomainApi(format!("save spec: {e}")))?;

            ctx.emit(AutomatonEvent::SpecSaved {
                spec_id: saved.id,
                title: saved.title,
            });
        }

        ctx.emit(AutomatonEvent::Progress {
            message: format!("{} specs generated and saved", specs.len()),
        });

        Ok(TickOutcome::Done)
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
struct RawSpec {
    title: String,
    markdown_contents: String,
}

struct ParsedSpec {
    title: String,
    content: String,
}

fn parse_spec_response(text: &str) -> Result<Vec<ParsedSpec>, AutomatonError> {
    let trimmed = text.trim();

    let json_str = if let Some(s) = extract_fenced_json(trimmed) {
        s
    } else {
        trimmed.to_string()
    };

    let raw: Vec<RawSpec> = serde_json::from_str(&json_str)
        .map_err(|e| AutomatonError::AgentExecution(format!("failed to parse spec JSON: {e}")))?;

    if raw.is_empty() {
        return Err(AutomatonError::AgentExecution(
            "LLM returned empty spec array".into(),
        ));
    }

    Ok(raw
        .into_iter()
        .map(|r| ParsedSpec {
            title: r.title,
            content: r.markdown_contents,
        })
        .collect())
}

fn extract_fenced_json(text: &str) -> Option<String> {
    let start = text.find("```")?;
    let after_fence = &text[start + 3..];
    let content_start = after_fence.find('\n')? + 1;
    let rest = &after_fence[content_start..];
    let end = rest.find("```")?;
    Some(rest[..end].trim().to_string())
}
