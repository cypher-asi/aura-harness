//! Main agentic turn loop and context management.

use super::{StepConfig, ToolCache, TurnEntry, TurnProcessor, TurnResult};
use aura_core::{AgentId, Transaction};
use aura_reasoner::{Message, ModelProvider, StopReason, ToolResultContent};
use aura_store::Store;
use aura_tools::ToolRegistry;
use std::collections::HashMap;
use tracing::{debug, info, warn};

/// Approximate tokens-per-character ratio for English text. Claude
/// tokenisers average ~3.5–4 chars/token; we use 4 for a conservative
/// (over-)estimate that biases toward earlier truncation.
const CHARS_PER_TOKEN: usize = 4;

impl<P, S, R> TurnProcessor<P, S, R>
where
    P: ModelProvider,
    S: Store,
    R: ToolRegistry,
{
    /// Build initial messages including conversation history from the store.
    pub(super) fn build_initial_messages(
        &self,
        agent_id: AgentId,
        tx: &Transaction,
        current_seq: u64,
    ) -> Vec<Message> {
        let mut messages = Vec::new();

        if current_seq > 1 && self.config.context_window > 0 {
            self.load_history(agent_id, current_seq, &mut messages);
        }

        let prompt = String::from_utf8_lossy(&tx.payload);
        debug!(
            prompt_len = prompt.len(),
            history_count = messages.len(),
            "Building messages for model"
        );
        messages.push(Message::user(prompt.to_string()));

        log_message_context(&messages);
        messages
    }

    fn load_history(&self, agent_id: AgentId, current_seq: u64, messages: &mut Vec<Message>) {
        let start_seq = current_seq
            .saturating_sub(self.config.context_window as u64)
            .max(1);
        let limit = self.config.context_window;

        debug!(
            agent_id = %agent_id,
            start_seq = start_seq,
            limit = limit,
            "Loading conversation history"
        );

        if let Ok(entries) = self.store.scan_record(agent_id, start_seq, limit) {
            let session_start_idx = entries
                .iter()
                .rposition(|e| e.tx.tx_type == aura_core::TransactionType::SessionStart);

            let relevant_entries = session_start_idx.map_or_else(
                || &entries[..],
                |idx| {
                    debug!(
                        session_start_seq = entries[idx].seq,
                        "Found session boundary"
                    );
                    &entries[idx + 1..]
                },
            );

            for entry in relevant_entries {
                match entry.tx.tx_type {
                    aura_core::TransactionType::UserPrompt => {
                        let content = String::from_utf8_lossy(&entry.tx.payload);
                        if !content.is_empty() {
                            messages.push(Message::user(content.to_string()));
                        }
                    }
                    aura_core::TransactionType::AgentMsg => {
                        let content = String::from_utf8_lossy(&entry.tx.payload);
                        if !content.is_empty() {
                            messages.push(Message::assistant(content.to_string()));
                        }
                    }
                    _ => {}
                }
            }
            debug!(
                loaded_messages = messages.len(),
                "Loaded conversation history"
            );
        }
    }

    /// Estimate the total character count of messages.
    fn estimate_message_chars(messages: &[Message]) -> usize {
        messages.iter().map(|m| m.text_content().len()).sum()
    }

    /// Truncate the message list to fit within the context-window budget.
    fn truncate_messages_if_needed(messages: &mut Vec<Message>, config: &super::TurnConfig) {
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::cast_precision_loss
        )]
        let budget_chars = (config.context_window_tokens as f64
            * f64::from(config.context_target_ratio)
            * CHARS_PER_TOKEN as f64) as usize;

        let total_chars = Self::estimate_message_chars(messages);

        if total_chars <= budget_chars || messages.len() <= 3 {
            return;
        }

        debug!(
            total_chars,
            budget_chars,
            message_count = messages.len(),
            "Context approaching limit — truncating older messages"
        );

        let keep_tail = 4usize;
        let first = messages[0].clone();
        let tail_start = messages.len().saturating_sub(keep_tail);
        let tail: Vec<Message> = messages[tail_start..].to_vec();

        let mut new_messages = Vec::with_capacity(2 + keep_tail);
        new_messages.push(first);
        new_messages.push(Message::user(
            "[Earlier tool results were truncated to fit the context window. \
             If you need to re-read a file, request it again.]"
                .to_string(),
        ));
        new_messages.extend(tail);

        let new_chars = Self::estimate_message_chars(&new_messages);
        info!(
            old_count = messages.len(),
            new_count = new_messages.len(),
            chars_saved = total_chars.saturating_sub(new_chars),
            "Truncated context"
        );
        *messages = new_messages;
    }

    /// Core agentic turn loop shared by both `process_turn` and
    /// `process_turn_with_messages`.
    pub(super) async fn run_turn_loop(
        &self,
        mut messages: Vec<Message>,
        agent_id: AgentId,
    ) -> anyhow::Result<TurnResult> {
        let mut entries = Vec::new();
        let mut total_input_tokens = 0u64;
        let mut total_output_tokens = 0u64;
        let mut had_failures = false;
        let mut cancelled = false;
        let mut final_message = None;
        let provider_name = self.provider.name().to_string();
        let model_name = self.config.model.clone();
        let mut tool_cache: ToolCache = HashMap::new();

        for step in 0..self.config.max_steps {
            if self.is_cancelled() {
                info!(step = step, "Turn cancelled before step");
                cancelled = true;
                break;
            }

            debug!(step = step, messages = messages.len(), "Processing step");
            Self::truncate_messages_if_needed(&mut messages, &self.config);

            let step_result = self
                .process_step(&messages, agent_id, &mut tool_cache, &StepConfig::default())
                .await?;

            total_input_tokens += step_result.response.usage.input_tokens;
            total_output_tokens += step_result.response.usage.output_tokens;
            messages.push(step_result.response.message.clone());
            final_message = Some(step_result.response.message.clone());

            if step_result.had_failures {
                had_failures = true;
            }

            let should_break = process_stop_reason(&step_result, step, &mut entries, &mut messages);
            if should_break {
                break;
            }
        }

        #[allow(clippy::cast_possible_truncation)]
        let steps = entries.len() as u32;
        info!(
            steps = steps,
            input_tokens = total_input_tokens,
            output_tokens = total_output_tokens,
            "Turn processing complete"
        );

        Ok(TurnResult {
            entries,
            final_message,
            total_input_tokens,
            total_output_tokens,
            steps,
            had_failures,
            cancelled,
            model: model_name,
            provider: provider_name,
        })
    }
}

fn log_message_context(messages: &[Message]) {
    for (i, msg) in messages.iter().enumerate() {
        let content_preview: String = msg.text_content().chars().take(50).collect();
        debug!(
            idx = i,
            role = ?msg.role,
            content_preview = %content_preview,
            "Message in context"
        );
    }
}

/// Returns `true` if the loop should break after handling this stop reason.
fn process_stop_reason(
    step_result: &super::StepResult,
    step: u32,
    entries: &mut Vec<TurnEntry>,
    messages: &mut Vec<Message>,
) -> bool {
    let step_u32 = step;
    match step_result.stop_reason {
        StopReason::EndTurn => {
            info!(step = step, "Turn completed (end_turn)");
            entries.push(TurnEntry {
                turn_step: step_u32,
                model_response: step_result.response.clone(),
                tool_results: vec![],
                executed_tools: vec![],
                stop_reason: StopReason::EndTurn,
            });
            true
        }
        StopReason::ToolUse => {
            let tool_results: Vec<(String, ToolResultContent, bool)> = step_result
                .executed_tools
                .iter()
                .map(|t| (t.tool_use_id.clone(), t.result.clone(), t.is_error))
                .collect();

            entries.push(TurnEntry {
                turn_step: step_u32,
                model_response: step_result.response.clone(),
                tool_results: tool_results.clone(),
                executed_tools: step_result.executed_tools.clone(),
                stop_reason: StopReason::ToolUse,
            });

            if !tool_results.is_empty() {
                messages.push(Message::tool_results(tool_results));
            }
            false
        }
        StopReason::MaxTokens => {
            warn!(step = step, "Turn stopped due to max_tokens");
            entries.push(TurnEntry {
                turn_step: step_u32,
                model_response: step_result.response.clone(),
                tool_results: vec![],
                executed_tools: vec![],
                stop_reason: StopReason::MaxTokens,
            });
            true
        }
        StopReason::StopSequence => {
            debug!(step = step, "Turn stopped at stop sequence");
            entries.push(TurnEntry {
                turn_step: step_u32,
                model_response: step_result.response.clone(),
                tool_results: vec![],
                executed_tools: vec![],
                stop_reason: StopReason::StopSequence,
            });
            true
        }
    }
}
