//! `load_skill`: an agent's skill, loaded when the agent decides it
//! needs it. The system prompt lists only the agent's skills
//! (`aura_context_skills::on_demand`); this tool returns one skill's full
//! instructions.
//!
//! Where the skills come from is the host's: the app looks them up
//! through the caller's permanent agent, the node through the session's
//! installed skills. Both are behind [`SkillSource`], so this crate needs
//! no skill store.

use std::sync::Arc;

use async_trait::async_trait;
use aura_core_types::{AgentId, ToolDefinition, ToolResult};

use crate::error::ToolError;
use crate::tool::{Tool, ToolContext};

/// Name of the tool. The same as `aura_context_skills::on_demand`'s, which
/// the skill index tells the model to call.
pub const LOAD_SKILL_TOOL_NAME: &str = "load_skill";

/// Where `load_skill` finds an agent's skills.
pub trait SkillSource: Send + Sync {
    /// The full instructions of skill `name` for `caller`.
    ///
    /// # Errors
    /// Why it can't be loaded: for an unknown name, which skills the
    /// caller does have.
    fn load(&self, caller: Option<AgentId>, name: &str) -> Result<String, String>;
}

/// `load_skill`: return one of the caller's skills in full.
pub struct LoadSkillTool {
    source: Arc<dyn SkillSource>,
}

impl LoadSkillTool {
    #[must_use]
    pub fn new(source: Arc<dyn SkillSource>) -> Self {
        Self { source }
    }
}

/// The tool's definition, for hosts that offer it without the tool.
#[must_use]
pub fn load_skill_definition() -> ToolDefinition {
    ToolDefinition::new(
        LOAD_SKILL_TOOL_NAME,
        "Load the full instructions of one of your skills, by the name the skills list in \
         your instructions gives it. Call it before a task the skill covers, then follow \
         what it returns.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "name": {"type": "string", "description": "The skill's name"}
            },
            "required": ["name"]
        }),
    )
}

#[async_trait]
impl Tool for LoadSkillTool {
    fn name(&self) -> &str {
        LOAD_SKILL_TOOL_NAME
    }

    fn definition(&self) -> ToolDefinition {
        load_skill_definition()
    }

    async fn execute(
        &self,
        ctx: &ToolContext,
        args: serde_json::Value,
    ) -> Result<ToolResult, ToolError> {
        let name = args
            .get("name")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .ok_or_else(|| ToolError::InvalidArguments("give the skill's `name`".into()))?;
        let text = self
            .source
            .load(ctx.caller_agent_id, name)
            .map_err(ToolError::InvalidArguments)?;
        Ok(ToolResult::success(LOAD_SKILL_TOOL_NAME, text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Sandbox, ToolConfig};

    struct Fixed;

    impl SkillSource for Fixed {
        fn load(&self, _caller: Option<AgentId>, name: &str) -> Result<String, String> {
            match name {
                "deploy" => Ok("Tag, build, then publish.".into()),
                other => Err(format!("no skill `{other}`; your skills: deploy")),
            }
        }
    }

    async fn call(args: serde_json::Value) -> Result<ToolResult, ToolError> {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(Sandbox::new(dir.path()).unwrap(), ToolConfig::default());
        LoadSkillTool::new(Arc::new(Fixed))
            .execute(&ctx, args)
            .await
    }

    #[tokio::test]
    async fn a_known_skill_comes_back_in_full() {
        let result = call(serde_json::json!({"name": "deploy"})).await.unwrap();
        assert!(result.ok, "{result:?}");
        assert!(String::from_utf8_lossy(&result.stdout).contains("Tag, build, then publish."));
    }

    #[tokio::test]
    async fn an_unknown_skill_or_no_name_says_what_to_do() {
        let unknown = call(serde_json::json!({"name": "launch"}))
            .await
            .unwrap_err();
        assert!(
            unknown.to_string().contains("your skills: deploy"),
            "{unknown}"
        );
        let missing = call(serde_json::json!({})).await.unwrap_err();
        assert!(missing.to_string().contains("name"), "{missing}");
    }
}
