use crate::gateway::session::helpers::kernel::{
    resolve_session_workspace, session_scoped_tool_config,
};
use crate::gateway::session::Session;
use aura_core_types::{AgentToolPermissions, ToolState, UserToolDefaults};
use aura_exec_tools::ToolConfig;
use std::path::PathBuf;

#[test]
fn resolve_session_workspace_uses_project_path_directly() {
    let mut session = Session::new(PathBuf::from("/tmp/aura"));
    session.project_path = Some(PathBuf::from("/tmp/project"));

    let (workspace, use_workspace_base_as_root) = resolve_session_workspace(&session);

    assert_eq!(workspace, PathBuf::from("/tmp/project"));
    assert!(use_workspace_base_as_root);
}

#[test]
fn resolve_session_workspace_uses_explicit_workspace_directly() {
    let mut session = Session::new(PathBuf::from("/tmp/aura"));
    session.workspace = PathBuf::from("/tmp/aura/session-123");

    let (workspace, use_workspace_base_as_root) = resolve_session_workspace(&session);

    assert_eq!(workspace, PathBuf::from("/tmp/aura/session-123"));
    assert!(use_workspace_base_as_root);
}

#[test]
fn resolve_session_workspace_keeps_base_for_default_workspace() {
    let session = Session::new(PathBuf::from("/tmp/aura"));

    let (workspace, use_workspace_base_as_root) = resolve_session_workspace(&session);

    assert_eq!(workspace, PathBuf::from("/tmp/aura"));
    assert!(!use_workspace_base_as_root);
}

#[test]
fn session_config_bypasses_allowlists_only_when_both_gates_open() {
    let mut base = ToolConfig::for_autonomous_dev_loop();
    base.command.allow_unrestricted_full_access = true;

    let full_access = UserToolDefaults::full_access();
    let config = session_scoped_tool_config(&base, &full_access, None);
    assert!(config.command.bypass_allowlists);

    let mut operator_off = base.clone();
    operator_off.command.allow_unrestricted_full_access = false;
    let config = session_scoped_tool_config(&operator_off, &full_access, None);
    assert!(!config.command.bypass_allowlists);

    let config = session_scoped_tool_config(&base, &UserToolDefaults::auto_review(), None);
    assert!(!config.command.bypass_allowlists);

    let narrowing_override = AgentToolPermissions::new().with("run_command", ToolState::Ask);
    let config = session_scoped_tool_config(&base, &full_access, Some(&narrowing_override));
    assert!(!config.command.bypass_allowlists);
}
