//! Golden snapshots of full rendered prompts under `src/system/__snapshots__`.

use super::super::sections::dev_loop_workflow::platform_info_string;
use super::super::*;
use super::support::demo_project;
use crate::descriptors::{AgentIdentity, AgentInfo, ProjectInfo};

const SNAPSHOT_DIR: &str = "__snapshots__";

fn snapshot_path(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/system")
        .join(SNAPSHOT_DIR)
        .join(format!("{name}.txt"))
}

fn assert_snapshot(name: &str, actual: &str) {
    let path = snapshot_path(name);
    if std::env::var("UPDATE_SNAPSHOTS").is_ok() {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create snapshot dir");
        }
        std::fs::write(&path, actual).expect("write snapshot");
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_else(|err| {
        panic!(
            "snapshot {} could not be read ({err}); rerun with `UPDATE_SNAPSHOTS=1`",
            path.display()
        )
    });
    let expected_norm = expected.replace("\r\n", "\n");
    assert_eq!(
        expected_norm,
        actual,
        "snapshot {} mismatch",
        path.display()
    );
}

fn scrub(s: &str, dir: &str) -> String {
    let mut out = s.replace(dir, "<TEMPDIR>");
    let norm = dir.replace('\\', "/");
    if norm != dir {
        out = out.replace(&norm, "<TEMPDIR>");
    }
    let platform = platform_info_string();
    out = out.replace(platform, "<PLATFORM_INFO>");
    out
}

#[test]
fn snapshot_dev_loop_default() {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().to_string_lossy().into_owned();
    let project = demo_project(&folder);
    let prompt = agentic_execution_system_prompt(&project, None, None);
    let scrubbed = scrub(&prompt, &folder);
    assert_snapshot("dev_loop_default", &scrubbed);
}

#[test]
fn snapshot_dev_loop_with_agents_md() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("AGENTS.md"),
        "Always run cargo check before tests.\nNo emojis.\n",
    )
    .unwrap();
    let folder = dir.path().to_string_lossy().into_owned();
    let project = demo_project(&folder);
    let prompt = agentic_execution_system_prompt(&project, None, None);
    let scrubbed = scrub(&prompt, &folder);
    assert_snapshot("dev_loop_with_agents_md", &scrubbed);
}

#[test]
fn snapshot_dev_loop_with_identity() {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().to_string_lossy().into_owned();
    let project = demo_project(&folder);
    let skills = vec!["Rust".to_string(), "TypeScript".to_string()];
    let agent = AgentInfo {
        identity: Some(AgentIdentity {
            name: "Atlas",
            role: "Engineer",
            personality: "Precise and methodical.",
        }),
        skills: &skills,
        system_prompt: Some("Use TDD on every change."),
    };
    let prompt = agentic_execution_system_prompt(&project, Some(&agent), None);
    let scrubbed = scrub(&prompt, &folder);
    assert_snapshot("dev_loop_with_identity", &scrubbed);
}

#[test]
fn snapshot_dev_loop_with_identity_and_agents_md() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("AGENTS.md"),
        "Always run cargo check before tests.\nNo emojis.\n",
    )
    .unwrap();
    let folder = dir.path().to_string_lossy().into_owned();
    let project = demo_project(&folder);
    let skills = vec!["Rust".to_string(), "TypeScript".to_string()];
    let agent = AgentInfo {
        identity: Some(AgentIdentity {
            name: "Atlas",
            role: "Engineer",
            personality: "Precise and methodical.",
        }),
        skills: &skills,
        system_prompt: Some("Use TDD on every change."),
    };
    let prompt = agentic_execution_system_prompt(&project, Some(&agent), None);
    let scrubbed = scrub(&prompt, &folder);
    assert_snapshot("dev_loop_with_identity_and_agents_md", &scrubbed);
}

#[test]
fn snapshot_chat_default() {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().to_string_lossy().into_owned();
    let project = demo_project(&folder);
    let prompt = build_chat_system_prompt(&project, "", None);
    let scrubbed = scrub(&prompt, &folder);
    assert_snapshot("chat_default", &scrubbed);
}

#[test]
fn snapshot_chat_with_agents_md() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("AGENTS.md"),
        "Always run cargo check before tests.\nNo emojis.\n",
    )
    .unwrap();
    let folder = dir.path().to_string_lossy().into_owned();
    let project = demo_project(&folder);
    let prompt = build_chat_system_prompt(&project, "", None);
    let scrubbed = scrub(&prompt, &folder);
    assert_snapshot("chat_with_agents_md", &scrubbed);
}

#[test]
fn snapshot_chat_with_identity() {
    // Chat-WS migration: identity / skills / operator-prompt populated,
    // no AGENTS.md. Mirrors the dev-loop identity snapshot but with the
    // chat-preset section selection (`<chat_capabilities>` instead of
    // `<dev_loop_workflow>` + `<tool_discipline>`). The `<project_context>`
    // block carries `project_id` because the chat path threads the
    // typed `ChatProjectInfoWire.id` through `ProjectInfo::project_id` —
    // pinning that the dev-loop continues to omit `project_id` while
    // chat surfaces the legacy aura-os field for tool grounding.
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().to_string_lossy().into_owned();
    let project = ProjectInfo {
        project_id: Some("00000000-0000-0000-0000-000000000001"),
        name: "Demo",
        description: "A demo project.",
        folder_path: &folder,
        build_command: Some("cargo build"),
        test_command: Some("cargo test"),
    };
    let skills = vec!["Rust".to_string(), "TypeScript".to_string()];
    let agent = AgentInfo {
        identity: Some(AgentIdentity {
            name: "Atlas",
            role: "Engineer",
            personality: "Precise and methodical.",
        }),
        skills: &skills,
        system_prompt: Some("Use TDD on every change."),
    };
    let prompt = build_chat_system_prompt(&project, "", Some(&agent));
    let scrubbed = scrub(&prompt, &folder);
    assert_snapshot("chat_with_identity", &scrubbed);
}

#[test]
fn snapshot_chat_with_identity_and_agents_md() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("AGENTS.md"),
        "Always run cargo check before tests.\nNo emojis.\n",
    )
    .unwrap();
    let folder = dir.path().to_string_lossy().into_owned();
    let project = ProjectInfo {
        project_id: Some("00000000-0000-0000-0000-000000000001"),
        name: "Demo",
        description: "A demo project.",
        folder_path: &folder,
        build_command: Some("cargo build"),
        test_command: Some("cargo test"),
    };
    let skills = vec!["Rust".to_string(), "TypeScript".to_string()];
    let agent = AgentInfo {
        identity: Some(AgentIdentity {
            name: "Atlas",
            role: "Engineer",
            personality: "Precise and methodical.",
        }),
        skills: &skills,
        system_prompt: Some("Use TDD on every change."),
    };
    let prompt = build_chat_system_prompt(&project, "", Some(&agent));
    let scrubbed = scrub(&prompt, &folder);
    assert_snapshot("chat_with_identity_and_agents_md", &scrubbed);
}
