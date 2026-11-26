//! Serialization tests.

use super::*;
use aura_core_types::{
    ActionKind, AgentId, Decision, Proposal, ToolCall, ToolResult, Transaction, TransactionType,
};
use bytes::Bytes;

proptest! {
    /// Transaction roundtrips through JSON.
    #[test]
    fn transaction_json_roundtrip(
        payload: Vec<u8>,
        tx_type in prop_oneof![
            Just(TransactionType::UserPrompt),
            Just(TransactionType::AgentMsg),
            Just(TransactionType::Trigger),
            Just(TransactionType::System),
        ]
    ) {
        let agent_id = AgentId::generate();
        let tx = Transaction::new_chained(
            agent_id,
            tx_type,
            Bytes::from(payload),
            None,
        );

        let json = serde_json::to_string(&tx).unwrap();
        let parsed: Transaction = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(tx, parsed);
    }

    /// ToolCall roundtrips through JSON.
    #[test]
    fn tool_call_json_roundtrip(tool: String, path: String) {
        prop_assume!(!tool.is_empty());
        let tool_call = ToolCall::new(&tool, serde_json::json!({ "path": path }));

        let json = serde_json::to_string(&tool_call).unwrap();
        let parsed: ToolCall = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(tool_call, parsed);
    }

    /// ToolResult roundtrips through JSON.
    #[test]
    fn tool_result_json_roundtrip(tool: String, output: String, ok: bool) {
        prop_assume!(!tool.is_empty());
        let result = if ok {
            ToolResult::success(&tool, output.clone())
        } else {
            ToolResult::failure(&tool, output.clone())
        };

        let json = serde_json::to_string(&result).unwrap();
        let parsed: ToolResult = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(result, parsed);
    }

    /// Proposal roundtrips through JSON.
    #[test]
    fn proposal_json_roundtrip(
        payload: Vec<u8>,
        kind in prop_oneof![
            Just(ActionKind::Reason),
            Just(ActionKind::Memorize),
            Just(ActionKind::Decide),
            Just(ActionKind::Delegate),
        ],
        rationale: Option<String>
    ) {
        let mut proposal = Proposal::new(kind, Bytes::from(payload));
        if let Some(r) = rationale {
            proposal = proposal.with_rationale(r);
        }

        let json = serde_json::to_string(&proposal).unwrap();
        let parsed: Proposal = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(proposal, parsed);
    }

    /// Decision roundtrips through JSON.
    #[test]
    fn decision_json_roundtrip(
        num_accepted in 0usize..5,
        num_rejected in 0u32..5
    ) {
        let mut decision = Decision::new();

        for _ in 0..num_accepted {
            decision.accept(aura_core_types::ActionId::generate());
        }

        for i in 0..num_rejected {
            decision.reject(i, format!("Reason {i}"));
        }

        let json = serde_json::to_string(&decision).unwrap();
        let parsed: Decision = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(decision, parsed);
    }
}
