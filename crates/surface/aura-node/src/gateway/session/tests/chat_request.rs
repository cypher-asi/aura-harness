//! `Session::apply_chat_runtime_request` behaviour: intent classifier,
//! permissions, skill lookup, partitioning and prompt assembly.

use super::support::{blank_chat_request, test_session};
use aura_core_protocol::{ChatProjectInfoWire, ProjectContext};
use aura_core_types::{AgentPermissions, Capability};

#[test]
fn apply_chat_runtime_request_builds_intent_classifier_when_spec_present() {
    use aura_core_protocol::{IntentClassifierRule, IntentClassifierSpec};
    use std::collections::HashMap;

    let mut session = test_session(None);
    let mut tool_domains = HashMap::new();
    tool_domains.insert("list_credits".to_string(), "billing".to_string());
    tool_domains.insert("create_project".to_string(), "project".to_string());

    let spec = IntentClassifierSpec {
        tier1_domains: vec!["project".to_string()],
        classifier_rules: vec![IntentClassifierRule {
            domain: "billing".to_string(),
            keywords: vec!["credit".to_string()],
        }],
        tool_domains,
    };
    let mut req = blank_chat_request();
    req.agent_capabilities.intent_classifier = Some(spec);

    session.apply_chat_runtime_request(req).unwrap();

    let classifier = session
        .intent_classifier
        .as_ref()
        .expect("classifier populated");
    let visible = classifier.visible_domains("please check my credit balance");
    assert!(visible.contains(&"project".to_string()));
    assert!(visible.contains(&"billing".to_string()));

    let manifest = &session.intent_classifier_manifest;
    assert_eq!(manifest.len(), 2);
    assert_eq!(manifest[0].0, "create_project");
    assert_eq!(manifest[1].0, "list_credits");

    let cfg = session.agent_loop_config();
    assert!(cfg.intent_classifier.is_some());
    assert_eq!(cfg.intent_classifier_manifest.len(), 2);
}

#[test]
fn apply_chat_runtime_request_leaves_intent_classifier_none_when_spec_absent() {
    let mut session = test_session(None);
    let req = blank_chat_request();
    session.apply_chat_runtime_request(req).unwrap();
    assert!(session.intent_classifier.is_none());
    assert!(session.intent_classifier_manifest.is_empty());

    let cfg = session.agent_loop_config();
    assert!(cfg.intent_classifier.is_none());
    assert!(cfg.intent_classifier_manifest.is_empty());
}

#[test]
fn apply_chat_runtime_request_applies_full_access_permissions_by_default() {
    let mut session = test_session(None);
    session
        .apply_chat_runtime_request(blank_chat_request())
        .unwrap();
    assert_eq!(session.agent_permissions, AgentPermissions::full_access());
}

#[test]
fn apply_chat_runtime_request_uses_template_id_for_skill_lookup() {
    let mut session = test_session(None);
    let mut req = blank_chat_request();
    req.agent_identity.partition_id = Some("spec-gen-project-123".to_string());
    req.agent_identity.template_id = Some("f74bc868-0a34-4195-9718-bf5ce7f67a55".to_string());

    session.apply_chat_runtime_request(req).unwrap();

    assert_eq!(
        session.skill_agent_id.as_deref(),
        Some("f74bc868-0a34-4195-9718-bf5ce7f67a55")
    );
}

#[test]
fn apply_chat_runtime_request_falls_back_to_partition_id_for_skill_lookup() {
    let mut session = test_session(None);
    let mut req = blank_chat_request();
    req.agent_identity.partition_id = Some("legacy-agent-id".to_string());

    session.apply_chat_runtime_request(req).unwrap();

    assert_eq!(session.skill_agent_id.as_deref(), Some("legacy-agent-id"));
}

/// Phase 2 of the cross-repo `parallel-session-chats` plan: two
/// runtime requests that differ only in the trailing `session` segment
/// of the partition string `"{template}::{instance}::{session}"` must
/// yield **distinct** `Session.agent_id`s.
#[test]
fn apply_chat_runtime_request_partitions_session_id_per_session_segment() {
    use aura_core_types::AgentId;

    let template_uuid = "f74bc868-0a34-4195-9718-bf5ce7f67a55";
    let instance = "abcdef01-2345-6789-abcd-ef0123456789";

    let mut session_a = test_session(None);
    let mut req_a = blank_chat_request();
    req_a.agent_identity.partition_id = Some(format!("{template_uuid}::{instance}::sess-A"));
    req_a.agent_identity.template_id = Some(template_uuid.to_string());
    req_a.project = Some(ProjectContext {
        project_id: "proj-test".to_string(),
        project_info: None,
        aura_agent_id: Some(template_uuid.to_string()),
        aura_org_id: None,
        aura_session_id: None,
    });
    session_a.apply_chat_runtime_request(req_a).unwrap();

    let mut session_b = test_session(None);
    let mut req_b = blank_chat_request();
    req_b.agent_identity.partition_id = Some(format!("{template_uuid}::{instance}::sess-B"));
    req_b.agent_identity.template_id = Some(template_uuid.to_string());
    req_b.project = Some(ProjectContext {
        project_id: "proj-test".to_string(),
        project_info: None,
        aura_agent_id: Some(template_uuid.to_string()),
        aura_org_id: None,
        aura_session_id: None,
    });
    session_b.apply_chat_runtime_request(req_b).unwrap();

    assert_ne!(
        session_a.agent_id, session_b.agent_id,
        "sessions differing only in the trailing session segment must \
         yield distinct Session.agent_ids so they get distinct record \
         logs and turn locks",
    );

    let template_agent_id = AgentId::from_uuid(
        uuid::Uuid::parse_str(template_uuid).expect("static template uuid is valid"),
    );
    assert_eq!(
        session_a.memory_agent_id(),
        template_agent_id,
        "memory_agent_id must resolve to the template uuid so memory \
         is shared across sessions of the same template+instance",
    );
    assert_eq!(
        session_b.memory_agent_id(),
        template_agent_id,
        "memory_agent_id must resolve to the template uuid so memory \
         is shared across sessions of the same template+instance",
    );
}

#[test]
fn apply_chat_runtime_request_applies_explicit_agent_permissions() {
    use aura_core_protocol::{AgentPermissionsWire, AgentScopeWire, CapabilityWire};
    let mut session = test_session(None);
    let mut req = blank_chat_request();
    req.agent_permissions = AgentPermissionsWire {
        scope: AgentScopeWire {
            orgs: vec!["org-a".into()],
            ..AgentScopeWire::default()
        },
        capabilities: vec![CapabilityWire::SpawnAgent, CapabilityWire::ReadAgent],
    };
    session.apply_chat_runtime_request(req).unwrap();
    let perms = &session.agent_permissions;
    assert_eq!(perms.scope.orgs, vec!["org-a".to_string()]);
    assert!(perms.capabilities.contains(&Capability::SpawnAgent));
    assert!(perms.capabilities.contains(&Capability::ReadAgent));
}

#[test]
fn apply_chat_runtime_request_applies_ceo_preset_when_wired_explicitly() {
    use aura_core_protocol::{AgentPermissionsWire, CapabilityWire};
    let mut session = test_session(None);
    let mut req = blank_chat_request();
    req.project = Some(ProjectContext {
        project_id: "proj-test".to_string(),
        project_info: None,
        aura_org_id: Some("org-uuid".into()),
        aura_session_id: None,
        aura_agent_id: None,
    });
    req.agent_permissions = AgentPermissionsWire {
        scope: Default::default(),
        capabilities: vec![
            CapabilityWire::SpawnAgent,
            CapabilityWire::ControlAgent,
            CapabilityWire::ReadAgent,
            CapabilityWire::ListAgents,
            CapabilityWire::ManageOrgMembers,
            CapabilityWire::ManageBilling,
            CapabilityWire::InvokeProcess,
            CapabilityWire::PostToFeed,
            CapabilityWire::GenerateMedia,
            CapabilityWire::ReadAllProjects,
            CapabilityWire::WriteAllProjects,
        ],
    };
    req.agent_capabilities.computer_use = true;
    session.apply_chat_runtime_request(req).unwrap();
    assert_eq!(session.agent_permissions, AgentPermissions::ceo_preset());
}

/// Chat-WS migration regression: when ANY typed-field is populated
/// on the runtime request, the session prompt must be built via
/// `SystemPromptBuilder` from those typed inputs.
#[test]
fn apply_chat_runtime_request_typed_fields_path_builds_assembled_prompt() {
    let mut session = test_session(None);
    session
        .apply_chat_runtime_request(typed_fields_request())
        .unwrap();

    let prompt = &session.system_prompt;
    assert_identity_sections(prompt);
    assert_project_sections(prompt);
}

/// Chat request with every typed identity/project field populated.
fn typed_fields_request() -> aura_core_protocol::RuntimeRequest {
    use aura_core_protocol::AgentPersona;

    let mut req = blank_chat_request();
    req.agent_identity.persona = Some(AgentPersona {
        name: "Atlas".into(),
        role: "Engineer".into(),
        personality: "Precise and methodical.".into(),
    });
    req.agent_identity.skills = vec!["Rust".into(), "TypeScript".into()];
    req.agent_identity.system_prompt = Some("Use TDD on every change.".into());
    req.project = Some(ProjectContext {
        project_id: "proj-test".to_string(),
        project_info: Some(ChatProjectInfoWire {
            id: "00000000-0000-0000-0000-000000000001".into(),
            name: "Demo".into(),
            description: "A demo project.".into(),
            workspace_root: String::new(),
            build_command: "cargo build".into(),
            test_command: "cargo test".into(),
        }),
        aura_org_id: None,
        aura_session_id: None,
        aura_agent_id: None,
    });
    req
}

/// Identity, skills and operator-prompt sections of the assembled prompt.
fn assert_identity_sections(prompt: &str) {
    assert!(
        prompt.contains("<chat_capabilities>"),
        "missing <chat_capabilities>: {prompt}"
    );
    assert!(
        prompt.contains("<agent_identity>"),
        "missing <agent_identity>: {prompt}"
    );
    assert!(
        prompt.contains("name: Atlas"),
        "missing identity name: {prompt}"
    );
    assert!(
        prompt.contains("role: Engineer"),
        "missing identity role: {prompt}"
    );
    assert!(
        prompt.contains("Precise and methodical"),
        "missing personality: {prompt}"
    );
    assert!(
        prompt.contains("<agent_skills>"),
        "missing <agent_skills>: {prompt}"
    );
    assert!(prompt.contains("- Rust"), "missing Rust skill: {prompt}");
    assert!(
        prompt.contains("- TypeScript"),
        "missing TypeScript skill: {prompt}"
    );
    assert!(
        prompt.contains("<agent_system_prompt>"),
        "missing <agent_system_prompt>: {prompt}"
    );
    assert!(
        prompt.contains("Use TDD on every change."),
        "missing operator prompt body: {prompt}"
    );
}

/// Project-context section of the assembled prompt.
fn assert_project_sections(prompt: &str) {
    assert!(
        prompt.contains("<project_context>"),
        "missing <project_context>: {prompt}"
    );
    assert!(
        prompt.contains("project_id: 00000000-0000-0000-0000-000000000001"),
        "missing project id: {prompt}",
    );
    assert!(
        prompt.contains("project_name: Demo"),
        "missing project name: {prompt}"
    );
    assert!(
        prompt.contains("build_command: cargo build"),
        "missing build command: {prompt}"
    );
    assert!(
        prompt.contains("test_command: cargo test"),
        "missing test command: {prompt}"
    );
}
