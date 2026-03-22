//! Model call delegation trait for external orchestrators.
//!
//! [`ModelCallDelegate`] allows an external loop (e.g., `AgentLoop`) to
//! delegate the model call to a processor while retaining control over tool
//! execution, blocking detection, context compaction, and budget management.
//!
//! # Streaming
//!
//! Streaming events are handled by the delegate's internal callback mechanism.
//! Callers that need streaming should configure the implementing processor's
//! streaming callback before passing it as a delegate.

use async_trait::async_trait;
use aura_reasoner::{ModelProvider, ModelRequest, ModelResponse};
use aura_store::Store;
use aura_tools::ToolRegistry;

use super::TurnProcessor;

/// Trait for delegating model calls to a processor.
///
/// Implementations handle streaming, cancellation, replay, and error
/// classification internally. External orchestrators use this to delegate
/// the model call while managing their own tool execution and intelligence.
#[async_trait]
pub trait ModelCallDelegate: Send + Sync {
    /// Make a model call, returning the model's response.
    ///
    /// The implementation may use streaming or non-streaming completions,
    /// handle cancellation, and emit streaming events through its own
    /// callback mechanism.
    async fn call_model(&self, request: ModelRequest) -> anyhow::Result<ModelResponse>;
}

#[async_trait]
impl<P, S, R> ModelCallDelegate for TurnProcessor<P, S, R>
where
    P: ModelProvider + Send + Sync + 'static,
    S: Store + Send + Sync + 'static,
    R: ToolRegistry + Send + Sync + 'static,
{
    async fn call_model(&self, request: ModelRequest) -> anyhow::Result<ModelResponse> {
        self.resolve_model_call(request).await
    }
}
