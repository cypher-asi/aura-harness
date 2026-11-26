use super::*;

#[test]
fn envelope_from_json_parses_full_payload() {
    let cfg = serde_json::json!({
        "agent_identity": {
            "name": "Atlas",
            "role": "Engineer",
            "personality": "Precise and methodical.",
        },
        "agent_skills": ["Rust", "TypeScript"],
        "agent_system_prompt": "Use TDD on every change.",
    });

    let envelope = AgentIdentityEnvelope::from_json(&cfg);

    assert!(
        !envelope.is_empty(),
        "populated payload must not collapse to empty"
    );
    let info = envelope
        .as_agent_info()
        .expect("populated envelope must yield AgentInfo");
    let identity = info.identity.expect("identity present");
    assert_eq!(identity.name, "Atlas");
    assert_eq!(identity.role, "Engineer");
    assert_eq!(identity.personality, "Precise and methodical.");
    assert_eq!(info.skills, &["Rust".to_string(), "TypeScript".to_string()]);
    assert_eq!(info.system_prompt, Some("Use TDD on every change."));
}

#[test]
fn envelope_from_json_handles_missing_fields() {
    let envelope = AgentIdentityEnvelope::from_json(&serde_json::json!({}));
    assert!(
        envelope.is_empty(),
        "empty JSON object must produce an empty envelope"
    );
    assert!(
        envelope.as_agent_info().is_none(),
        "empty envelope must yield no AgentInfo so identity sections drop"
    );
}

#[test]
fn envelope_from_json_treats_blank_strings_as_empty() {
    let cfg = serde_json::json!({
        "agent_identity": {
            "name": "   ",
            "role": "",
            "personality": "\n\t",
        },
        "agent_skills": [],
        "agent_system_prompt": "   ",
    });

    let envelope = AgentIdentityEnvelope::from_json(&cfg);
    assert!(
        envelope.is_empty(),
        "blank-string fields must collapse to empty so no <agent_*> tags render"
    );
    assert!(envelope.as_agent_info().is_none());
}

#[test]
fn envelope_skills_only_still_renders_as_populated() {
    // Skills-only payloads should still render an <agent_skills> tag
    // even when identity / system prompt are absent.
    let cfg = serde_json::json!({
        "agent_skills": ["Rust"],
    });

    let envelope = AgentIdentityEnvelope::from_json(&cfg);
    assert!(!envelope.is_empty());
    let info = envelope.as_agent_info().expect("skills-only is populated");
    assert!(
        info.identity.is_none(),
        "missing identity object must leave AgentInfo.identity = None"
    );
    assert_eq!(info.skills, &["Rust".to_string()]);
    assert!(info.system_prompt.is_none());
}

#[test]
fn envelope_roundtrips_into_system_prompt_tags() {
    // End-to-end roundtrip: aura-os-shaped JSON → envelope →
    // AgentInfo → agentic_execution_system_prompt → rendered tags.
    let cfg = serde_json::json!({
        "agent_identity": {
            "name": "Atlas",
            "role": "Engineer",
            "personality": "Precise and methodical.",
        },
        "agent_skills": ["Rust", "TypeScript"],
        "agent_system_prompt": "Use TDD on every change.",
    });
    let envelope = AgentIdentityEnvelope::from_json(&cfg);
    let info = envelope.as_agent_info().expect("populated");

    let project = aura_context_prompts::ProjectInfo {
        project_id: None,
        name: "Demo",
        description: "A demo project.",
        folder_path: "/nonexistent",
        build_command: Some("cargo build"),
        test_command: Some("cargo test"),
    };
    let prompt = aura_context_prompts::agentic_execution_system_prompt(&project, Some(&info), None);

    for tag in [
        "<agent_identity>",
        "</agent_identity>",
        "<agent_skills>",
        "- Rust",
        "- TypeScript",
        "</agent_skills>",
        "<agent_system_prompt>",
        "Use TDD on every change.",
        "</agent_system_prompt>",
        "<project_context>",
    ] {
        assert!(
            prompt.contains(tag),
            "expected {tag} in the rendered roundtrip prompt; got:\n{prompt}",
        );
    }
}

#[test]
fn empty_envelope_keeps_identity_sections_off() {
    let envelope = AgentIdentityEnvelope::from_json(&serde_json::json!({}));
    let info = envelope.as_agent_info();
    assert!(info.is_none());

    let project = aura_context_prompts::ProjectInfo {
        project_id: None,
        name: "Demo",
        description: "A demo project.",
        folder_path: "/nonexistent",
        build_command: Some("cargo build"),
        test_command: Some("cargo test"),
    };
    let prompt =
        aura_context_prompts::agentic_execution_system_prompt(&project, info.as_ref(), None);

    for tag in [
        "<agent_identity>",
        "<agent_skills>",
        "<agent_system_prompt>",
    ] {
        assert!(
            !prompt.contains(tag),
            "empty envelope must NOT render {tag}; got:\n{prompt}",
        );
    }
}
