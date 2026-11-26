use super::*;

use crate::sandbox::Sandbox;

use crate::ToolConfig;

use async_trait::async_trait;

use aura_core_types::{AgentId, AgentPermissions, AgentScope};

use std::sync::Arc;

fn ctx(caller: AgentPermissions) -> ToolContext {
    let dir = std::env::temp_dir();
    let mut ctx = ToolContext::new(Sandbox::new(&dir).unwrap(), ToolConfig::default());
    ctx.caller_permissions = Some(caller);
    ctx.caller_agent_id = Some(AgentId::generate());
    ctx.originating_user_id = Some("user-root".into());
    ctx
}

/// In-memory `AgentControlHook` used to drive the `execute` path
/// without standing up a real aura-os-server. `deliver_message`
/// returns `Ok(())` so the success branch is exercised.
struct OkHook;

#[async_trait]
impl crate::tool::AgentControlHook for OkHook {
    async fn deliver_message(
        &self,
        _target_agent_id: &str,
        _parent_agent_id: Option<&str>,
        _originating_user_id: Option<&str>,
        _project_id: Option<&str>,
        _content: &str,
        _attachments: Option<serde_json::Value>,
        _model: Option<&str>,
    ) -> Result<(), String> {
        Ok(())
    }

    async fn lifecycle(
        &self,
        _target_agent_id: &str,
        _parent_agent_id: Option<&str>,
        _originating_user_id: Option<&str>,
        _action: &str,
    ) -> Result<(), String> {
        Ok(())
    }

    async fn delegate_task(
        &self,
        _target_agent_id: &str,
        _parent_agent_id: Option<&str>,
        _originating_user_id: Option<&str>,
        _task: &str,
        _context: Option<&serde_json::Value>,
        _model: Option<&str>,
    ) -> Result<(), String> {
        Ok(())
    }
}

/// Capturing hook that records the `parent_agent_id` value
/// `send_to_agent` actually shipped to the runtime side-effect
/// layer. Used by the cross-repo regression tests below to pin the
/// "ship the upstream OS UUID, not the truncated harness hash"
/// contract that lets the server's
/// `spawn_cross_agent_reply_callback` resolve the originating
/// agent's REST URL.
struct CapturingHook {
    captured_parent: std::sync::Mutex<Option<String>>,
    captured_project: std::sync::Mutex<Option<String>>,
    captured_model: std::sync::Mutex<Option<String>>,
}

impl CapturingHook {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            captured_parent: std::sync::Mutex::new(None),
            captured_project: std::sync::Mutex::new(None),
            captured_model: std::sync::Mutex::new(None),
        })
    }
    fn captured(&self) -> Option<String> {
        self.captured_parent.lock().unwrap().clone()
    }
    fn captured_model(&self) -> Option<String> {
        self.captured_model.lock().unwrap().clone()
    }
    fn captured_project(&self) -> Option<String> {
        self.captured_project.lock().unwrap().clone()
    }
}

#[async_trait]
impl crate::tool::AgentControlHook for CapturingHook {
    async fn deliver_message(
        &self,
        _target_agent_id: &str,
        parent_agent_id: Option<&str>,
        _originating_user_id: Option<&str>,
        project_id: Option<&str>,
        _content: &str,
        _attachments: Option<serde_json::Value>,
        model: Option<&str>,
    ) -> Result<(), String> {
        *self.captured_parent.lock().unwrap() = parent_agent_id.map(str::to_string);
        *self.captured_project.lock().unwrap() = project_id.map(str::to_string);
        *self.captured_model.lock().unwrap() = model.map(str::to_string);
        Ok(())
    }
    async fn lifecycle(
        &self,
        _: &str,
        _: Option<&str>,
        _: Option<&str>,
        _: &str,
    ) -> Result<(), String> {
        Ok(())
    }
    async fn delegate_task(
        &self,
        _: &str,
        _: Option<&str>,
        _: Option<&str>,
        _: &str,
        _: Option<&serde_json::Value>,
        _model: Option<&str>,
    ) -> Result<(), String> {
        Ok(())
    }
}

/// Hook with threads: records the subject `send_to_agent` passed.
#[derive(Default)]
struct ThreadedHook {
    captured: std::sync::Mutex<Option<(String, Option<String>)>>,
}

#[async_trait]
impl crate::tool::AgentControlHook for ThreadedHook {
    async fn deliver_message(
        &self,
        _: &str,
        _: Option<&str>,
        _: Option<&str>,
        _: Option<&str>,
        _: &str,
        _: Option<serde_json::Value>,
        _: Option<&str>,
    ) -> Result<(), String> {
        Err("send_to_agent must call deliver_message_in_thread".into())
    }

    async fn deliver_message_in_thread(
        &self,
        _target_agent_id: &str,
        _parent_agent_id: Option<&str>,
        _originating_user_id: Option<&str>,
        _project_id: Option<&str>,
        content: &str,
        _attachments: Option<serde_json::Value>,
        _model: Option<&str>,
        subject: Option<&str>,
    ) -> Result<(), String> {
        *self.captured.lock().unwrap() = Some((content.to_string(), subject.map(str::to_string)));
        Ok(())
    }

    async fn lifecycle(
        &self,
        _: &str,
        _: Option<&str>,
        _: Option<&str>,
        _: &str,
    ) -> Result<(), String> {
        Ok(())
    }

    async fn delegate_task(
        &self,
        _: &str,
        _: Option<&str>,
        _: Option<&str>,
        _: &str,
        _: Option<&serde_json::Value>,
        _: Option<&str>,
    ) -> Result<(), String> {
        Ok(())
    }
}

fn control_ctx() -> ToolContext {
    ctx(AgentPermissions {
        scope: AgentScope::default(),
        capabilities: vec![Capability::ControlAgent],
    })
}

mod forwarding;
mod scope;
mod subject;
