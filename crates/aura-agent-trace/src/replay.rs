//! Replaying a recorded run prefix.
//!
//! [`ReplayProvider`] serves recorded model responses in order, then
//! hands off to a live provider. Running the agent loop against it in a
//! fresh copy of the workspace re-executes the recorded tool calls, so
//! the state at the branch point matches the original run; from there
//! a different model or different [`aura_config::HarnessParams`]
//! continue live. That yields counterfactual pairs that share an exact
//! prefix.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use aura_core_types::trace::{ModelAction, Step, StepAction};
use aura_model_reasoner::{
    ContentBlock, Message, ModelProvider, ModelRequest, ModelResponse, ProviderTrace,
    ReasonerError, Role, StopReason, Usage,
};
use aura_store_snapshot::{FsSnapshotStore, SnapshotStore};

use crate::bundle::blobs_dir;
use crate::TraceError;

/// Serves recorded responses, then delegates to `live`.
pub struct ReplayProvider {
    recorded: Mutex<VecDeque<ModelResponse>>,
    live: Option<Arc<dyn ModelProvider>>,
}

impl std::fmt::Debug for ReplayProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReplayProvider")
            .field("remaining", &self.remaining())
            .field("live", &self.live.is_some())
            .finish()
    }
}

impl ReplayProvider {
    /// Provider over explicit responses.
    #[must_use]
    pub fn new(recorded: Vec<ModelResponse>, live: Option<Arc<dyn ModelProvider>>) -> Self {
        Self {
            recorded: Mutex::new(recorded.into()),
            live,
        }
    }

    /// Provider over the model steps of `steps` with `seq <= upto`.
    /// Exact messages are read from `home`'s blob store when the run
    /// was recorded at the `full` level; otherwise the response is
    /// rebuilt from the recorded text and tool calls.
    ///
    /// # Errors
    ///
    /// Returns [`TraceError::Invalid`] when the prefix has no model
    /// step, or a blob store error.
    pub fn from_steps(
        home: &Path,
        steps: &[Step],
        upto: u64,
        live: Option<Arc<dyn ModelProvider>>,
    ) -> Result<Self, TraceError> {
        let blobs = FsSnapshotStore::open(blobs_dir(home))?;
        let responses: Vec<ModelResponse> = steps
            .iter()
            .filter(|s| s.seq <= upto)
            .filter_map(|s| match &s.action {
                StepAction::Model(action) => Some(response_for(s, action, &blobs)),
                _ => None,
            })
            .collect();
        if responses.is_empty() {
            return Err(TraceError::Invalid(format!(
                "no model step at or before step {upto}"
            )));
        }
        Ok(Self::new(responses, live))
    }

    /// Recorded responses not yet served.
    #[must_use]
    pub fn remaining(&self) -> usize {
        self.recorded
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }
}

fn response_for(step: &Step, action: &ModelAction, blobs: &FsSnapshotStore) -> ModelResponse {
    let message = action
        .message
        .as_ref()
        .and_then(|id| blobs.get(id).ok().flatten())
        .and_then(|bytes| serde_json::from_slice::<Message>(&bytes).ok())
        .unwrap_or_else(|| rebuild_message(action));
    let stop_reason: StopReason =
        serde_json::from_value(serde_json::Value::String(action.stop_reason.clone()))
            .unwrap_or_default();
    let usage = step.usage.unwrap_or_default();
    ModelResponse::new(
        stop_reason,
        message,
        Usage {
            input_tokens: usage.input,
            output_tokens: usage.output,
            cache_creation_input_tokens: Some(usage.cache_write),
            cache_read_input_tokens: Some(usage.cache_read),
        },
        ProviderTrace::new(step.policy.id.clone(), 0),
    )
}

/// Assistant message from the recorded summary. Thinking text is not
/// replayed; it does not affect tool execution.
#[must_use]
pub fn rebuild_message(action: &ModelAction) -> Message {
    let mut content = Vec::new();
    if !action.text.is_empty() {
        content.push(ContentBlock::Text {
            text: action.text.clone(),
        });
    }
    for call in &action.tool_calls {
        content.push(ContentBlock::ToolUse {
            id: call.tool_use_id.clone(),
            name: call.name.clone(),
            input: call.input.clone(),
        });
    }
    Message {
        role: Role::Assistant,
        content,
    }
}

#[async_trait]
impl ModelProvider for ReplayProvider {
    fn name(&self) -> &'static str {
        "replay"
    }

    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, ReasonerError> {
        let next = self
            .recorded
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop_front();
        if let Some(response) = next {
            return Ok(response);
        }
        match &self.live {
            Some(live) => live.complete(request).await,
            None => Err(ReasonerError::Internal(
                "replay exhausted and no live provider configured".into(),
            )),
        }
    }

    async fn health_check(&self) -> bool {
        match &self.live {
            Some(live) => live.health_check().await,
            None => self.remaining() > 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rewards::tests::model;
    use serde_json::json;

    #[tokio::test]
    async fn serves_prefix_then_errors_without_live() {
        let home = tempfile::tempdir().unwrap();
        let steps = vec![
            model(1, &[("a", "read_file", json!({"path": "x"}))]),
            model(2, &[]),
            model(3, &[]),
        ];
        let provider = ReplayProvider::from_steps(home.path(), &steps, 2, None).unwrap();
        assert_eq!(provider.remaining(), 2);
        let request = ModelRequest::builder("m", "sys").try_build().unwrap();
        let first = provider.complete(request.clone()).await.unwrap();
        assert!(matches!(
            first.message.content.first(),
            Some(ContentBlock::ToolUse { name, .. }) if name == "read_file"
        ));
        provider.complete(request.clone()).await.unwrap();
        assert!(provider.complete(request).await.is_err());
    }

    #[test]
    fn empty_prefix_is_rejected() {
        let home = tempfile::tempdir().unwrap();
        assert!(ReplayProvider::from_steps(home.path(), &[model(5, &[])], 2, None).is_err());
    }
}
