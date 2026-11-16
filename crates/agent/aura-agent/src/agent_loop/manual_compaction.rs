//! Summarizing a session's older history on request (`/compact`), between
//! turns, rather than when context pressure forces it inside one.

use aura_context_compaction as compaction;
use aura_exec_tools::todo::{render_todos, TodoItem};
use aura_model::{ContentBlock, Message, ModelProvider};
use tokio_util::sync::CancellationToken;

use super::config::AgentLoopConfig;
use super::iteration::LlmCallError;
use super::AgentLoop;

/// A history with its older part replaced by a summary.
#[derive(Debug, Clone)]
pub struct CompactedHistory {
    /// The new history
    pub messages: Vec<Message>,
    /// Estimated tokens of the history before
    pub before_tokens: u64,
    /// Estimated tokens of the history after
    pub after_tokens: u64,
}

/// Why [`AgentLoop::compact_history`] left the history as it was.
#[derive(Debug, thiserror::Error)]
pub enum CompactHistoryError {
    /// Not enough older history to be worth a summary
    #[error("the history is too short to summarize")]
    TooShort,
    /// The summary call failed
    #[error("summary failed: {0}")]
    Model(String),
    /// The model returned no summary text
    #[error("the model returned an empty summary")]
    EmptySummary,
    /// The summary could not replace the history it covers
    #[error("the summary could not be applied")]
    NotApplied,
    /// Cancelled before the summary arrived
    #[error("cancelled")]
    Cancelled,
}

impl AgentLoop {
    /// The loop's configuration.
    #[must_use]
    pub const fn config(&self) -> &AgentLoopConfig {
        &self.config
    }

    /// Replace the older part of `messages` with a model-written summary,
    /// keeping the first message and the recent tail verbatim. `focus`
    /// tells the summary what to keep in particular. `todos` are appended
    /// to the summary, since it replaces the `todo_write` calls that
    /// carried them.
    ///
    /// # Errors
    ///
    /// [`CompactHistoryError`] when the history is left unchanged.
    pub async fn compact_history(
        &self,
        provider: &dyn ModelProvider,
        messages: &[Message],
        todos: &[TodoItem],
        focus: Option<&str>,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<CompactedHistory, CompactHistoryError> {
        let input =
            compaction::forced_summary_input(messages).ok_or(CompactHistoryError::TooShort)?;
        let request = self
            .build_summary_request(&input, focus)
            .map_err(|e| CompactHistoryError::Model(e.to_string()))?;
        let response = self
            .call_model(provider, request, None, cancellation_token)
            .await
            .map_err(|e| match e {
                _ if cancellation_token.is_some_and(CancellationToken::is_cancelled) => {
                    CompactHistoryError::Cancelled
                }
                LlmCallError::InsufficientCredits(m)
                | LlmCallError::PromptTooLong(m)
                | LlmCallError::RateLimited(m)
                | LlmCallError::Fatal(m) => CompactHistoryError::Model(m),
            })?;

        let mut text = response
            .message
            .content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        if text.trim().is_empty() {
            return Err(CompactHistoryError::EmptySummary);
        }
        if !todos.is_empty() {
            text.push_str("\n\nCurrent todo list (keep it updated with todo_write):\n");
            text.push_str(&render_todos(todos));
        }

        let mut compacted = messages.to_vec();
        let report = compaction::apply_message_summary(
            &mut compacted,
            compaction::SummaryOutput::Messages {
                text,
                compact_start: input.compact_start,
                compact_end: input.compact_end,
            },
        );
        if matches!(report.action, compaction::CompactionAction::None) {
            return Err(CompactHistoryError::NotApplied);
        }
        Ok(CompactedHistory {
            messages: compacted,
            before_tokens: compaction::message_chars_to_tokens(report.before_chars),
            after_tokens: compaction::message_chars_to_tokens(report.after_chars),
        })
    }
}
