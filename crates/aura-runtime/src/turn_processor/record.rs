//! Record entry conversion for turn results.

use super::TurnProcessor;
use aura_core::{
    Action, AuraError, Decision, Effect, EffectKind, EffectStatus, ProposalSet, RecordEntry,
    ToolCall, Transaction,
};
use aura_reasoner::{ModelProvider, ToolResultContent};
use aura_store::Store;
use aura_tools::ToolRegistry;
use bytes::Bytes;

use super::TurnResult;

impl<P, S, R> TurnProcessor<P, S, R>
where
    P: ModelProvider,
    S: Store,
    R: ToolRegistry,
{
    /// Convert turn results to a `RecordEntry` for storage.
    ///
    /// This properly records all tool calls with their full information
    /// (tool name, args, results).
    ///
    /// # Errors
    ///
    /// Returns `AuraError::Serialization` if tool call delegation payloads
    /// cannot be serialized.
    pub fn to_record_entry(
        &self,
        seq: u64,
        tx: Transaction,
        turn_result: &TurnResult,
        context_hash: [u8; 32],
    ) -> Result<RecordEntry, AuraError> {
        let proposals = ProposalSet::new();
        let mut decision = Decision::new();
        let mut actions = Vec::new();
        let mut effects = Vec::new();

        for entry in &turn_result.entries {
            for executed_tool in &entry.executed_tools {
                let tool_call = ToolCall::new(
                    executed_tool.tool_name.clone(),
                    executed_tool.tool_args.clone(),
                );

                let action = Action::delegate_tool(&tool_call)?;
                let action_id = action.action_id;
                actions.push(action);

                decision.accept(action_id);

                let effect_status = if executed_tool.is_error {
                    EffectStatus::Failed
                } else {
                    EffectStatus::Committed
                };

                let payload = match &executed_tool.result {
                    ToolResultContent::Text(s) => Bytes::from(s.clone()),
                    ToolResultContent::Json(v) => Bytes::from(serde_json::to_vec(v)?),
                };

                let effect = Effect::new(action_id, EffectKind::Agreement, effect_status, payload);
                effects.push(effect);
            }
        }

        Ok(RecordEntry::builder(seq, tx)
            .context_hash(context_hash)
            .proposals(proposals)
            .decision(decision)
            .actions(actions)
            .effects(effects)
            .build())
    }
}
