//! Wire→core translators and prompt / path helpers used while
//! applying a chat [`aura_core_protocol::RuntimeRequest`] to a [`super::Session`].

use aura_context_prompts::{
    AgentIdentity as PromptAgentIdentity, ProjectInfo, SystemPromptBuilder,
};
use aura_core_protocol::{
    AgentPersona, ChatProjectInfoWire, IntentClassifierSpec, ReasoningEffort,
};
use aura_exec_tools::IntentClassifier;
use aura_model::ThinkingEffort;
use std::path::{Path, PathBuf};

/// Chat-WS migration helper: assemble the chat-path system prompt
/// from the typed [`aura_core_protocol::RuntimeRequest`] identity / project info fields
/// via [`SystemPromptBuilder`].
///
/// Returns `Some(prompt)` when at least one typed field is populated,
/// and `None` when every typed field is absent / blank.
pub(super) fn build_typed_chat_system_prompt(
    persona: Option<&AgentPersona>,
    skills: &[String],
    agent_system_prompt: Option<&str>,
    project_wire: Option<&ChatProjectInfoWire>,
) -> Option<String> {
    let persona_populated = persona.is_some_and(|w| !w.is_empty());
    let skills_populated = skills.iter().any(|s| !s.trim().is_empty());
    let agent_prompt_populated = agent_system_prompt.is_some_and(|s| !s.trim().is_empty());
    let project_populated = project_wire.is_some_and(|w| !w.is_empty());

    if !persona_populated && !skills_populated && !agent_prompt_populated && !project_populated {
        return None;
    }

    let identity = persona
        .filter(|w| !w.is_empty())
        .map(|w| PromptAgentIdentity {
            name: w.name.as_str(),
            role: w.role.as_str(),
            personality: w.personality.as_str(),
        });
    let mut builder = SystemPromptBuilder::new()
        .chat_capabilities()
        .agent_identity(identity)
        .agent_skills(skills)
        .agent_system_prompt(agent_system_prompt);

    if let Some(project) = project_wire.filter(|w| !w.is_empty()) {
        let project_info = ProjectInfo {
            project_id: Some(project.id.as_str()).filter(|s| !s.trim().is_empty()),
            name: project.name.as_str(),
            description: project.description.as_str(),
            folder_path: project.workspace_root.as_str(),
            build_command: Some(project.build_command.as_str()).filter(|s| !s.trim().is_empty()),
            test_command: Some(project.test_command.as_str()).filter(|s| !s.trim().is_empty()),
        };
        builder = builder
            .project_context(&project_info)
            .agents_md_from_workspace(project_info.folder_path);
    }

    Some(builder.build())
}

/// Translate an [`IntentClassifierSpec`] from the wire protocol into
/// the in-process [`IntentClassifier`] plus a `(tool_name, domain)`
/// manifest the agent loop can consume.
pub(super) fn build_intent_classifier(
    spec: IntentClassifierSpec,
) -> (IntentClassifier, Vec<(String, String)>) {
    let IntentClassifierSpec {
        tier1_domains,
        classifier_rules,
        tool_domains,
    } = spec;
    let rules: Vec<(String, Vec<String>)> = classifier_rules
        .into_iter()
        .map(|r| (r.domain, r.keywords))
        .collect();
    let mut manifest: Vec<(String, String)> = tool_domains.into_iter().collect();
    manifest.sort_by(|a, b| a.0.cmp(&b.0));
    (IntentClassifier::from_rules(tier1_domains, rules), manifest)
}

/// Map the wire [`ReasoningEffort`] tier onto the reasoner's internal
/// [`ThinkingEffort`]. `Minimal`/`Max` are preserved 1:1; the Anthropic
/// budget mapping happens later in the reasoner's request conversion.
pub(super) fn thinking_effort_from_wire(effort: ReasoningEffort) -> ThinkingEffort {
    match effort {
        ReasoningEffort::Minimal => ThinkingEffort::Minimal,
        ReasoningEffort::Low => ThinkingEffort::Low,
        ReasoningEffort::Medium => ThinkingEffort::Medium,
        ReasoningEffort::High => ThinkingEffort::High,
        ReasoningEffort::XHigh => ThinkingEffort::XHigh,
        ReasoningEffort::Max => ThinkingEffort::Max,
    }
}

pub(crate) use aura_core_protocol::agent_permissions_from_wire;

/// Return whether a project path points into the harness-owned Safe Workspace
/// tree beside `workspaces/` under the node data directory.
///
/// Hosted deployments may also configure `project_base` (for example
/// `/home/aura`). Safe worktrees deliberately live under the persistent node
/// data directory, so they need this narrow second accepted root. Arbitrary
/// paths outside both roots remain rejected.
pub(crate) fn is_managed_safe_workspace_path(workspace_base: &Path, path: &Path) -> bool {
    let Some(data_dir) = workspace_base.parent() else {
        return false;
    };
    let safe_root = lexical_normalize(&data_dir.join("safe-workspaces"));
    let normalized = lexical_normalize(path);
    let Ok(relative) = normalized.strip_prefix(safe_root) else {
        return false;
    };
    let mut components = relative.components();
    matches!(
        (components.next(), components.next(), components.next()),
        (
            Some(std::path::Component::Normal(_)),
            Some(std::path::Component::Normal(_)),
            Some(std::path::Component::Normal(repo)),
        ) if repo == std::ffi::OsStr::new("repo")
    )
}

pub(super) fn lexical_normalize(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}
