//! WebSocket session protocol message types.
//!
//! Re-exports canonical types from `aura-protocol` and provides
//! harness-specific conversions between wire types and internal types.

pub use aura_protocol::*;

use aura_core::{InstalledIntegrationDefinition, InstalledToolDefinition};
use aura_reasoner::ToolDefinition;

/// Convert a reasoner [`ToolDefinition`] into a protocol [`ToolInfo`].
pub fn tool_info_from_definition(td: &ToolDefinition) -> ToolInfo {
    ToolInfo {
        name: td.name.clone(),
        description: td.description.clone(),
    }
}

/// Convert a protocol [`InstalledTool`] into a core [`InstalledToolDefinition`].
pub fn installed_tool_to_core(t: InstalledTool) -> InstalledToolDefinition {
    InstalledToolDefinition {
        name: t.name,
        description: t.description,
        input_schema: t.input_schema,
        endpoint: t.endpoint,
        auth: match t.auth {
            ToolAuth::None => aura_core::ToolAuth::None,
            ToolAuth::Bearer { token } => aura_core::ToolAuth::Bearer { token },
            ToolAuth::ApiKey { header, key } => aura_core::ToolAuth::ApiKey { header, key },
            ToolAuth::Headers { headers } => aura_core::ToolAuth::Headers { headers },
        },
        timeout_ms: t.timeout_ms,
        namespace: t.namespace,
        required_integration: t.required_integration.map(|requirement| {
            aura_core::InstalledToolIntegrationRequirement {
                integration_id: requirement.integration_id,
                provider: requirement.provider,
                kind: requirement.kind,
            }
        }),
        runtime_execution: t.runtime_execution.map(|execution| match execution {
            InstalledToolRuntimeExecution::AppProvider(provider) => {
                aura_core::InstalledToolRuntimeExecution::AppProvider(
                    aura_core::InstalledToolRuntimeProviderExecution {
                        provider: provider.provider,
                        base_url: provider.base_url,
                        static_headers: provider.static_headers,
                        integrations: provider
                            .integrations
                            .into_iter()
                            .map(|integration| aura_core::InstalledToolRuntimeIntegration {
                                integration_id: integration.integration_id,
                                base_url: integration.base_url,
                                auth: match integration.auth {
                                    InstalledToolRuntimeAuth::None => {
                                        aura_core::InstalledToolRuntimeAuth::None
                                    }
                                    InstalledToolRuntimeAuth::AuthorizationBearer { token } => {
                                        aura_core::InstalledToolRuntimeAuth::AuthorizationBearer {
                                            token,
                                        }
                                    }
                                    InstalledToolRuntimeAuth::AuthorizationRaw { value } => {
                                        aura_core::InstalledToolRuntimeAuth::AuthorizationRaw {
                                            value,
                                        }
                                    }
                                    InstalledToolRuntimeAuth::Header { name, value } => {
                                        aura_core::InstalledToolRuntimeAuth::Header { name, value }
                                    }
                                    InstalledToolRuntimeAuth::QueryParam { name, value } => {
                                        aura_core::InstalledToolRuntimeAuth::QueryParam {
                                            name,
                                            value,
                                        }
                                    }
                                    InstalledToolRuntimeAuth::Basic { username, password } => {
                                        aura_core::InstalledToolRuntimeAuth::Basic {
                                            username,
                                            password,
                                        }
                                    }
                                },
                                provider_config: integration.provider_config,
                            })
                            .collect(),
                    },
                )
            }
        }),
        metadata: t.metadata,
    }
}

/// Convert a protocol [`InstalledIntegration`] into a core [`InstalledIntegrationDefinition`].
pub fn installed_integration_to_core(
    integration: InstalledIntegration,
) -> InstalledIntegrationDefinition {
    InstalledIntegrationDefinition {
        integration_id: integration.integration_id,
        name: integration.name,
        provider: integration.provider,
        kind: integration.kind,
        metadata: integration.metadata,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json;

    // ========================================================================
    // Inbound message deserialization
    // ========================================================================

    #[test]
    fn test_inbound_session_init_full() {
        let json = serde_json::json!({
            "type": "session_init",
            "system_prompt": "You are helpful",
            "model": (aura_agent::DEFAULT_MODEL),
            "max_tokens": 4096,
            "temperature": 0.7,
            "max_turns": 10,
            "workspace": "/tmp/ws",
            "token": "jwt-abc",
            "project_id": "proj-123"
        });
        let msg: InboundMessage = serde_json::from_value(json).unwrap();
        match msg {
            InboundMessage::SessionInit(init) => {
                assert_eq!(init.system_prompt.as_deref(), Some("You are helpful"));
                assert_eq!(init.model.as_deref(), Some(aura_agent::DEFAULT_MODEL));
                assert_eq!(init.max_tokens, Some(4096));
                assert!((init.temperature.unwrap() - 0.7).abs() < f32::EPSILON);
                assert_eq!(init.max_turns, Some(10));
                assert_eq!(init.workspace.as_deref(), Some("/tmp/ws"));
                assert_eq!(init.token.as_deref(), Some("jwt-abc"));
                assert_eq!(init.project_id.as_deref(), Some("proj-123"));
            }
            _ => panic!("Expected SessionInit"),
        }
    }

    #[test]
    fn test_inbound_session_init_minimal() {
        let json = serde_json::json!({"type": "session_init"});
        let msg: InboundMessage = serde_json::from_value(json).unwrap();
        match msg {
            InboundMessage::SessionInit(init) => {
                assert!(init.system_prompt.is_none());
                assert!(init.model.is_none());
                assert!(init.max_tokens.is_none());
                assert!(init.temperature.is_none());
                assert!(init.max_turns.is_none());
                assert!(init.installed_tools.is_none());
                assert!(init.installed_integrations.is_none());
                assert!(init.workspace.is_none());
                assert!(init.token.is_none());
            }
            _ => panic!("Expected SessionInit"),
        }
    }

    #[test]
    fn test_inbound_user_message() {
        let json = serde_json::json!({"type": "user_message", "content": "hello world"});
        let msg: InboundMessage = serde_json::from_value(json).unwrap();
        match msg {
            InboundMessage::UserMessage(um) => assert_eq!(um.content, "hello world"),
            _ => panic!("Expected UserMessage"),
        }
    }

    #[test]
    fn test_inbound_cancel() {
        let json = serde_json::json!({"type": "cancel"});
        let msg: InboundMessage = serde_json::from_value(json).unwrap();
        assert!(matches!(msg, InboundMessage::Cancel));
    }

    #[test]
    fn test_inbound_approval_response_approved() {
        let json = serde_json::json!({
            "type": "approval_response",
            "tool_use_id": "tu_123",
            "approved": true
        });
        let msg: InboundMessage = serde_json::from_value(json).unwrap();
        match msg {
            InboundMessage::ApprovalResponse(ar) => {
                assert_eq!(ar.tool_use_id, "tu_123");
                assert!(ar.approved);
            }
            _ => panic!("Expected ApprovalResponse"),
        }
    }

    #[test]
    fn test_inbound_approval_response_denied() {
        let json = serde_json::json!({
            "type": "approval_response",
            "tool_use_id": "tu_456",
            "approved": false
        });
        let msg: InboundMessage = serde_json::from_value(json).unwrap();
        match msg {
            InboundMessage::ApprovalResponse(ar) => {
                assert_eq!(ar.tool_use_id, "tu_456");
                assert!(!ar.approved);
            }
            _ => panic!("Expected ApprovalResponse"),
        }
    }

    #[test]
    fn test_inbound_unknown_type_fails() {
        let json = serde_json::json!({"type": "nonexistent"});
        assert!(serde_json::from_value::<InboundMessage>(json).is_err());
    }

    #[test]
    fn test_inbound_missing_type_fails() {
        let json = serde_json::json!({"content": "hello"});
        assert!(serde_json::from_value::<InboundMessage>(json).is_err());
    }

    // ========================================================================
    // Outbound message serialization
    // ========================================================================

    #[test]
    fn test_outbound_session_ready_roundtrip() {
        let msg = OutboundMessage::SessionReady(SessionReady {
            session_id: "sess_1".to_string(),
            tools: vec![
                ToolInfo {
                    name: "read_file".to_string(),
                    description: "Read a file".to_string(),
                },
                ToolInfo {
                    name: "write_file".to_string(),
                    description: "Write a file".to_string(),
                },
            ],
            skills: vec![],
        });
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["type"], "session_ready");
        assert_eq!(json["session_id"], "sess_1");
        assert_eq!(json["tools"].as_array().unwrap().len(), 2);
        assert_eq!(json["tools"][0]["name"], "read_file");
    }

    #[test]
    fn test_outbound_assistant_message_start() {
        let msg = OutboundMessage::AssistantMessageStart(AssistantMessageStart {
            message_id: "msg_1".to_string(),
        });
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["type"], "assistant_message_start");
        assert_eq!(json["message_id"], "msg_1");
    }

    #[test]
    fn test_outbound_text_delta() {
        let msg = OutboundMessage::TextDelta(TextDelta {
            text: "Hello, ".to_string(),
        });
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["type"], "text_delta");
        assert_eq!(json["text"], "Hello, ");
    }

    #[test]
    fn test_outbound_thinking_delta() {
        let msg = OutboundMessage::ThinkingDelta(ThinkingDelta {
            thinking: "Let me consider...".to_string(),
        });
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["type"], "thinking_delta");
        assert_eq!(json["thinking"], "Let me consider...");
    }

    #[test]
    fn test_outbound_tool_use_start() {
        let msg = OutboundMessage::ToolUseStart(ToolUseStart {
            id: "tu_1".to_string(),
            name: "read_file".to_string(),
        });
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["type"], "tool_use_start");
        assert_eq!(json["id"], "tu_1");
        assert_eq!(json["name"], "read_file");
    }

    #[test]
    fn test_outbound_tool_result() {
        let msg = OutboundMessage::ToolResult(ToolResultMsg {
            name: "read_file".to_string(),
            result: "file contents here".to_string(),
            is_error: false,
            tool_use_id: Some("tu_1".to_string()),
        });
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["type"], "tool_result");
        assert_eq!(json["name"], "read_file");
        assert!(!json["is_error"].as_bool().unwrap());
        assert_eq!(json["tool_use_id"], "tu_1");
    }

    #[test]
    fn test_outbound_tool_result_error() {
        let msg = OutboundMessage::ToolResult(ToolResultMsg {
            name: "write_file".to_string(),
            result: "permission denied".to_string(),
            is_error: true,
            tool_use_id: None,
        });
        let json = serde_json::to_value(&msg).unwrap();
        assert!(json["is_error"].as_bool().unwrap());
        assert_eq!(json["result"], "permission denied");
        assert!(json.get("tool_use_id").is_none());
    }

    #[test]
    fn test_outbound_assistant_message_end() {
        let msg = OutboundMessage::AssistantMessageEnd(AssistantMessageEnd {
            message_id: "msg_1".to_string(),
            stop_reason: "end_turn".to_string(),
            usage: SessionUsage {
                input_tokens: 100,
                output_tokens: 50,
                estimated_context_tokens: 150,
                cache_creation_input_tokens: 25,
                cache_read_input_tokens: 10,
                cumulative_input_tokens: 200,
                cumulative_output_tokens: 100,
                cumulative_cache_creation_input_tokens: 50,
                cumulative_cache_read_input_tokens: 20,
                context_utilization: 0.5,
                model: aura_agent::DEFAULT_MODEL.to_string(),
                provider: "anthropic".to_string(),
            },
            files_changed: FilesChanged {
                created: vec!["new.txt".to_string()],
                modified: vec!["old.txt".to_string()],
                deleted: vec![],
            },
        });
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["type"], "assistant_message_end");
        assert_eq!(json["message_id"], "msg_1");
        assert_eq!(json["stop_reason"], "end_turn");
        assert_eq!(json["usage"]["input_tokens"], 100);
        assert_eq!(json["usage"]["output_tokens"], 50);
        assert_eq!(json["usage"]["estimated_context_tokens"], 150);
        assert_eq!(json["usage"]["cache_creation_input_tokens"], 25);
        assert_eq!(json["usage"]["cache_read_input_tokens"], 10);
        assert_eq!(json["usage"]["cumulative_cache_creation_input_tokens"], 50);
        assert_eq!(json["usage"]["cumulative_cache_read_input_tokens"], 20);
        assert_eq!(json["usage"]["model"], aura_agent::DEFAULT_MODEL);
        assert_eq!(json["files_changed"]["created"][0], "new.txt");
        assert_eq!(json["files_changed"]["modified"][0], "old.txt");
        assert!(json["files_changed"]["deleted"]
            .as_array()
            .unwrap()
            .is_empty());
    }

    #[test]
    fn test_outbound_error_msg() {
        let msg = OutboundMessage::Error(ErrorMsg {
            code: "rate_limit".to_string(),
            message: "Too many requests".to_string(),
            recoverable: true,
        });
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["type"], "error");
        assert_eq!(json["code"], "rate_limit");
        assert!(json["recoverable"].as_bool().unwrap());
    }

    #[test]
    fn test_outbound_error_non_recoverable() {
        let msg = OutboundMessage::Error(ErrorMsg {
            code: "auth_failed".to_string(),
            message: "Invalid token".to_string(),
            recoverable: false,
        });
        let json = serde_json::to_value(&msg).unwrap();
        assert!(!json["recoverable"].as_bool().unwrap());
    }

    // ========================================================================
    // Structural / utility tests
    // ========================================================================

    #[test]
    fn test_session_usage_default() {
        let usage = SessionUsage::default();
        assert_eq!(usage.input_tokens, 0);
        assert_eq!(usage.output_tokens, 0);
        assert_eq!(usage.estimated_context_tokens, 0);
        assert_eq!(usage.cumulative_input_tokens, 0);
        assert_eq!(usage.cumulative_output_tokens, 0);
        assert!((usage.context_utilization - 0.0).abs() < f32::EPSILON);
        assert!(usage.model.is_empty());
        assert!(usage.provider.is_empty());
    }

    #[test]
    fn test_files_changed_is_empty() {
        let fc = FilesChanged::default();
        assert!(fc.is_empty());

        let fc2 = FilesChanged {
            created: vec!["a.txt".to_string()],
            ..Default::default()
        };
        assert!(!fc2.is_empty());

        let fc3 = FilesChanged {
            modified: vec!["b.txt".to_string()],
            ..Default::default()
        };
        assert!(!fc3.is_empty());

        let fc4 = FilesChanged {
            deleted: vec!["c.txt".to_string()],
            ..Default::default()
        };
        assert!(!fc4.is_empty());
    }

    #[test]
    fn test_tool_info_from_tool_definition() {
        let td = ToolDefinition::new(
            "test_tool",
            "A test tool",
            serde_json::json!({"type": "object"}),
        );
        let info = tool_info_from_definition(&td);
        assert_eq!(info.name, "test_tool");
        assert_eq!(info.description, "A test tool");
    }

    #[test]
    fn test_inbound_user_message_empty_content() {
        let json = serde_json::json!({"type": "user_message", "content": ""});
        let msg: InboundMessage = serde_json::from_value(json).unwrap();
        match msg {
            InboundMessage::UserMessage(um) => assert!(um.content.is_empty()),
            _ => panic!("Expected UserMessage"),
        }
    }

    #[test]
    fn test_inbound_user_message_unicode() {
        let json = serde_json::json!({"type": "user_message", "content": "こんにちは🌍"});
        let msg: InboundMessage = serde_json::from_value(json).unwrap();
        match msg {
            InboundMessage::UserMessage(um) => assert_eq!(um.content, "こんにちは🌍"),
            _ => panic!("Expected UserMessage"),
        }
    }

    #[test]
    fn test_outbound_all_variants_serialize() {
        let variants: Vec<OutboundMessage> = vec![
            OutboundMessage::SessionReady(SessionReady {
                session_id: "s".into(),
                tools: vec![],
                skills: vec![],
            }),
            OutboundMessage::AssistantMessageStart(AssistantMessageStart {
                message_id: "m".into(),
            }),
            OutboundMessage::TextDelta(TextDelta { text: "t".into() }),
            OutboundMessage::ThinkingDelta(ThinkingDelta {
                thinking: "th".into(),
            }),
            OutboundMessage::ToolUseStart(ToolUseStart {
                id: "i".into(),
                name: "n".into(),
            }),
            OutboundMessage::ToolResult(ToolResultMsg {
                name: "n".into(),
                result: "r".into(),
                is_error: false,
                tool_use_id: None,
            }),
            OutboundMessage::AssistantMessageEnd(AssistantMessageEnd {
                message_id: "m".into(),
                stop_reason: "s".into(),
                usage: SessionUsage::default(),
                files_changed: FilesChanged::default(),
            }),
            OutboundMessage::Error(ErrorMsg {
                code: "c".into(),
                message: "m".into(),
                recoverable: false,
            }),
        ];

        let expected_types = [
            "session_ready",
            "assistant_message_start",
            "text_delta",
            "thinking_delta",
            "tool_use_start",
            "tool_result",
            "assistant_message_end",
            "error",
        ];

        for (variant, expected) in variants.iter().zip(expected_types.iter()) {
            let json = serde_json::to_value(variant).unwrap();
            assert_eq!(
                json["type"].as_str().unwrap(),
                *expected,
                "variant type mismatch"
            );
        }
    }
}
