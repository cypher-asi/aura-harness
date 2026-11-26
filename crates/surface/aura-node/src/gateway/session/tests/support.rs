//! Shared fixtures for the session-state unit tests.

use super::super::Session;
use aura_core_protocol::{
    AgentCapabilities, AgentIdentity, AgentPermissionsWire, ModelSelection, RuntimeRequest,
    RuntimeRequestType, WorkspaceLocation,
};
use std::path::PathBuf;

pub(super) fn absolute_path(parts: &[&str]) -> PathBuf {
    #[cfg(windows)]
    let mut path = PathBuf::from(r"C:\");
    #[cfg(not(windows))]
    let mut path = PathBuf::from("/");

    for part in parts {
        path.push(part);
    }

    path
}

pub(super) fn test_session(project_base: Option<PathBuf>) -> Session {
    let tmp = std::env::temp_dir().join("aura-test-session");
    let _ = std::fs::create_dir_all(&tmp);
    let mut s = Session::new(tmp);
    s.project_base = project_base;
    s
}

pub(super) fn blank_chat_request() -> RuntimeRequest {
    RuntimeRequest {
        r#type: RuntimeRequestType::Chat {
            conversation_messages: Vec::new(),
        },
        agent_identity: AgentIdentity::default(),
        model: ModelSelection::default(),
        workspace: WorkspaceLocation::default(),
        project: None,
        agent_permissions: AgentPermissionsWire::default(),
        tool_permissions: None,
        agent_capabilities: AgentCapabilities::default(),
        auth_jwt: None,
        user_id: "user-test".to_string(),
        mode: None,
    }
}

pub(super) fn chat_request_with_project_path(path: &std::path::Path) -> RuntimeRequest {
    let mut req = blank_chat_request();
    req.workspace.project_path = Some(path.display().to_string());
    req
}
