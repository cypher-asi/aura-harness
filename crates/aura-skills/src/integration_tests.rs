#![allow(clippy::needless_pass_by_value)]

use crate::error::SkillError;
use crate::install::{SkillInstallStore, SkillInstallation};
use crate::loader::{SkillLoader, SkillLoaderConfig};
use crate::manager::SkillManager;
use crate::types::SkillSource;
use chrono::Utc;
use rocksdb::{ColumnFamilyDescriptor, DBWithThreadMode, MultiThreaded, Options};
use std::path::Path;
use std::sync::Arc;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn test_db(dir: &Path) -> Arc<DBWithThreadMode<MultiThreaded>> {
    let mut opts = Options::default();
    opts.create_if_missing(true);
    opts.create_missing_column_families(true);

    let cfs = vec![
        ColumnFamilyDescriptor::new("record", Options::default()),
        ColumnFamilyDescriptor::new("agent_meta", Options::default()),
        ColumnFamilyDescriptor::new("inbox", Options::default()),
        ColumnFamilyDescriptor::new("memory_facts", Options::default()),
        ColumnFamilyDescriptor::new("memory_events", Options::default()),
        ColumnFamilyDescriptor::new("memory_procedures", Options::default()),
        ColumnFamilyDescriptor::new("agent_skills", Options::default()),
    ];

    Arc::new(DBWithThreadMode::<MultiThreaded>::open_cf_descriptors(&opts, dir, cfs).unwrap())
}

fn make_skill_dir(base: &Path, name: &str, desc: &str) {
    let dir = base.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {desc}\n---\nBody for {name}."),
    )
    .unwrap();
}

fn make_skill_dir_ext(base: &Path, name: &str, extra_frontmatter: &str) {
    let dir = base.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {name} desc\n{extra_frontmatter}---\nBody for {name}."),
    )
    .unwrap();
}

fn workspace_loader(workspace: &Path) -> SkillLoader {
    SkillLoader::new(SkillLoaderConfig {
        workspace_root: Some(workspace.to_path_buf()),
        ..SkillLoaderConfig::default()
    })
}

// ===========================================================================
// 1. SkillManager end-to-end
// ===========================================================================

#[test]
fn manager_new_loads_skills_from_workspace() {
    let tmp = TempDir::new().unwrap();
    let skills = tmp.path().join("skills");
    make_skill_dir(&skills, "deploy", "Deploy the app");
    make_skill_dir(&skills, "test-runner", "Run tests");

    let mgr = SkillManager::new(workspace_loader(tmp.path()));
    let all = mgr.list_all();
    assert_eq!(all.len(), 2);

    let names: Vec<&str> = all.iter().map(|m| m.name.as_str()).collect();
    assert!(names.contains(&"deploy"));
    assert!(names.contains(&"test-runner"));
}

#[test]
fn manager_inject_skills_adds_xml_to_prompt() {
    let tmp = TempDir::new().unwrap();
    let skills = tmp.path().join("skills");
    make_skill_dir(&skills, "deploy", "Deploy the app");

    let mgr = SkillManager::new(workspace_loader(tmp.path()));

    let mut prompt = "You are an assistant.".to_string();
    mgr.inject_skills(&mut prompt);

    assert!(prompt.starts_with("You are an assistant."));
    assert!(prompt.contains("<available_skills>"));
    assert!(prompt.contains("name=\"deploy\""));
    assert!(prompt.contains("</available_skills>"));
}

#[test]
fn manager_inject_skills_empty_when_no_skills() {
    let tmp = TempDir::new().unwrap();
    // No skills directory at all
    let mgr = SkillManager::new(workspace_loader(tmp.path()));

    let mut prompt = "System prompt.".to_string();
    mgr.inject_skills(&mut prompt);
    assert_eq!(prompt, "System prompt.");
}

#[test]
fn manager_activate_returns_rendered_content() {
    let tmp = TempDir::new().unwrap();
    let skills = tmp.path().join("skills");
    let dir = skills.join("greeter");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        "---\nname: greeter\ndescription: Greet someone\n---\nHello $ARGUMENTS! Welcome $0.",
    )
    .unwrap();

    let mgr = SkillManager::new(workspace_loader(tmp.path()));
    let act = mgr.activate("greeter", "world").unwrap();

    assert_eq!(act.skill_name, "greeter");
    assert_eq!(act.rendered_content, "Hello world! Welcome world.");
    assert!(!act.fork_context);
    assert!(act.allowed_tools.is_empty());
}

#[test]
fn manager_get_returns_error_for_nonexistent_skill() {
    let tmp = TempDir::new().unwrap();
    let mgr = SkillManager::new(workspace_loader(tmp.path()));

    let err = mgr.get("no-such-skill").unwrap_err();
    assert!(err.is_not_found());
}

#[test]
fn manager_list_all_and_list_user_invocable() {
    let tmp = TempDir::new().unwrap();
    let skills = tmp.path().join("skills");
    make_skill_dir_ext(&skills, "alpha", "user-invocable: true\n");
    make_skill_dir_ext(&skills, "beta", "");
    make_skill_dir_ext(&skills, "gamma", "user-invocable: true\n");

    let mgr = SkillManager::new(workspace_loader(tmp.path()));

    assert_eq!(mgr.list_all().len(), 3);

    let user_invocable = mgr.list_user_invocable();
    assert_eq!(user_invocable.len(), 2);
    let names: Vec<&str> = user_invocable.iter().map(|m| m.name.as_str()).collect();
    assert!(names.contains(&"alpha"));
    assert!(names.contains(&"gamma"));
}

#[test]
fn manager_reload_picks_up_new_skills() {
    let tmp = TempDir::new().unwrap();
    let skills = tmp.path().join("skills");
    make_skill_dir(&skills, "first", "first skill");

    let mut mgr = SkillManager::new(workspace_loader(tmp.path()));
    assert_eq!(mgr.list_all().len(), 1);

    make_skill_dir(&skills, "second", "second skill");
    mgr.reload();
    assert_eq!(mgr.list_all().len(), 2);
}

#[test]
fn manager_activate_with_indexed_arguments() {
    let tmp = TempDir::new().unwrap();
    let skills = tmp.path().join("skills");
    let dir = skills.join("deployer");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        "---\nname: deployer\ndescription: Deploy\n---\nDeploy $ARGUMENTS[0] to $ARGUMENTS[1].",
    )
    .unwrap();

    let mgr = SkillManager::new(workspace_loader(tmp.path()));
    let act = mgr.activate("deployer", "myapp production").unwrap();
    assert_eq!(act.rendered_content, "Deploy myapp to production.");
}

// ===========================================================================
// 2. SkillInstallStore CRUD
// ===========================================================================

#[test]
fn install_store_install_writes_to_db() {
    let tmp = TempDir::new().unwrap();
    let db = test_db(tmp.path());
    let store = SkillInstallStore::new(db);

    let inst = SkillInstallation {
        agent_id: "agent-1".to_string(),
        skill_name: "deploy".to_string(),
        source_url: Some("https://example.com/deploy".to_string()),
        installed_at: Utc::now(),
        version: Some("1.0.0".to_string()),
    };

    store.install(&inst).unwrap();
    assert!(store.is_installed("agent-1", "deploy").unwrap());
}

#[test]
fn install_store_list_for_agent_returns_correct_skills() {
    let tmp = TempDir::new().unwrap();
    let db = test_db(tmp.path());
    let store = SkillInstallStore::new(db);

    for name in &["alpha", "beta", "gamma"] {
        store
            .install(&SkillInstallation {
                agent_id: "agent-1".to_string(),
                skill_name: (*name).to_string(),
                source_url: None,
                installed_at: Utc::now(),
                version: None,
            })
            .unwrap();
    }

    store
        .install(&SkillInstallation {
            agent_id: "agent-2".to_string(),
            skill_name: "other".to_string(),
            source_url: None,
            installed_at: Utc::now(),
            version: None,
        })
        .unwrap();

    let agent1_skills = store.list_for_agent("agent-1").unwrap();
    assert_eq!(agent1_skills.len(), 3);
    let names: Vec<&str> = agent1_skills.iter().map(|s| s.skill_name.as_str()).collect();
    assert!(names.contains(&"alpha"));
    assert!(names.contains(&"beta"));
    assert!(names.contains(&"gamma"));
    assert!(!names.contains(&"other"));
}

#[test]
fn install_store_is_installed_returns_true_false() {
    let tmp = TempDir::new().unwrap();
    let db = test_db(tmp.path());
    let store = SkillInstallStore::new(db);

    assert!(!store.is_installed("agent-1", "deploy").unwrap());

    store
        .install(&SkillInstallation {
            agent_id: "agent-1".to_string(),
            skill_name: "deploy".to_string(),
            source_url: None,
            installed_at: Utc::now(),
            version: None,
        })
        .unwrap();

    assert!(store.is_installed("agent-1", "deploy").unwrap());
    assert!(!store.is_installed("agent-1", "other-skill").unwrap());
    assert!(!store.is_installed("agent-2", "deploy").unwrap());
}

#[test]
fn install_store_uninstall_removes_installation() {
    let tmp = TempDir::new().unwrap();
    let db = test_db(tmp.path());
    let store = SkillInstallStore::new(db);

    store
        .install(&SkillInstallation {
            agent_id: "agent-1".to_string(),
            skill_name: "deploy".to_string(),
            source_url: None,
            installed_at: Utc::now(),
            version: None,
        })
        .unwrap();

    assert!(store.is_installed("agent-1", "deploy").unwrap());
    store.uninstall("agent-1", "deploy").unwrap();
    assert!(!store.is_installed("agent-1", "deploy").unwrap());
    assert!(store.list_for_agent("agent-1").unwrap().is_empty());
}

// ===========================================================================
// 3. SkillManager with install store
// ===========================================================================

#[test]
fn manager_install_for_agent() {
    let tmp_ws = TempDir::new().unwrap();
    let tmp_db = TempDir::new().unwrap();
    let db = test_db(tmp_db.path());
    let store = Arc::new(SkillInstallStore::new(db));

    let loader = workspace_loader(tmp_ws.path());
    let mgr = SkillManager::with_install_store(loader, store);

    let inst = mgr
        .install_for_agent("agent-1", "deploy", Some("https://example.com".to_string()))
        .unwrap();

    assert_eq!(inst.agent_id, "agent-1");
    assert_eq!(inst.skill_name, "deploy");
    assert_eq!(inst.source_url.as_deref(), Some("https://example.com"));
}

#[test]
fn manager_list_agent_skills() {
    let tmp_ws = TempDir::new().unwrap();
    let tmp_db = TempDir::new().unwrap();
    let db = test_db(tmp_db.path());
    let store = Arc::new(SkillInstallStore::new(db));

    let loader = workspace_loader(tmp_ws.path());
    let mgr = SkillManager::with_install_store(loader, store);

    mgr.install_for_agent("agent-1", "deploy", None).unwrap();
    mgr.install_for_agent("agent-1", "test-runner", None)
        .unwrap();

    let skills = mgr.list_agent_skills("agent-1").unwrap();
    assert_eq!(skills.len(), 2);
}

#[test]
fn manager_uninstall_from_agent() {
    let tmp_ws = TempDir::new().unwrap();
    let tmp_db = TempDir::new().unwrap();
    let db = test_db(tmp_db.path());
    let store = Arc::new(SkillInstallStore::new(db));

    let loader = workspace_loader(tmp_ws.path());
    let mgr = SkillManager::with_install_store(loader, Arc::clone(&store));

    mgr.install_for_agent("agent-1", "deploy", None).unwrap();
    assert_eq!(mgr.list_agent_skills("agent-1").unwrap().len(), 1);

    mgr.uninstall_from_agent("agent-1", "deploy").unwrap();
    assert!(mgr.list_agent_skills("agent-1").unwrap().is_empty());
}

#[test]
fn manager_without_install_store_returns_error() {
    let tmp = TempDir::new().unwrap();
    let mgr = SkillManager::new(workspace_loader(tmp.path()));

    let err = mgr.install_for_agent("agent-1", "x", None).unwrap_err();
    assert!(matches!(err, SkillError::Activation(_)));

    let err = mgr.list_agent_skills("agent-1").unwrap_err();
    assert!(matches!(err, SkillError::Activation(_)));

    let err = mgr.uninstall_from_agent("agent-1", "x").unwrap_err();
    assert!(matches!(err, SkillError::Activation(_)));
}

// ===========================================================================
// 4. SkillError
// ===========================================================================

#[test]
fn skill_error_is_not_found_returns_true_for_not_found() {
    let err = SkillError::NotFound("test".to_string());
    assert!(err.is_not_found());
}

#[test]
fn skill_error_is_not_found_returns_false_for_other_variants() {
    let variants: Vec<SkillError> = vec![
        SkillError::Parse("parse error".to_string()),
        SkillError::InvalidName("bad name".to_string()),
        SkillError::Activation("activation error".to_string()),
        SkillError::CommandExecution("cmd error".to_string()),
        SkillError::Store("store error".to_string()),
    ];
    for err in variants {
        assert!(
            !err.is_not_found(),
            "is_not_found() should be false for {err}"
        );
    }
}

// ===========================================================================
// 5. SkillInstallation serde round-trip
// ===========================================================================

#[test]
fn skill_installation_serde_round_trip() {
    let original = SkillInstallation {
        agent_id: "agent-42".to_string(),
        skill_name: "my-skill".to_string(),
        source_url: Some("https://example.com/skill".to_string()),
        installed_at: Utc::now(),
        version: Some("2.1.0".to_string()),
    };

    let json = serde_json::to_string(&original).unwrap();
    let deserialized: SkillInstallation = serde_json::from_str(&json).unwrap();

    assert_eq!(deserialized.agent_id, original.agent_id);
    assert_eq!(deserialized.skill_name, original.skill_name);
    assert_eq!(deserialized.source_url, original.source_url);
    assert_eq!(deserialized.installed_at, original.installed_at);
    assert_eq!(deserialized.version, original.version);
}

#[test]
fn skill_installation_serde_round_trip_minimal() {
    let original = SkillInstallation {
        agent_id: "a".to_string(),
        skill_name: "b".to_string(),
        source_url: None,
        installed_at: Utc::now(),
        version: None,
    };

    let json = serde_json::to_string(&original).unwrap();
    let deserialized: SkillInstallation = serde_json::from_str(&json).unwrap();

    assert_eq!(deserialized.agent_id, "a");
    assert_eq!(deserialized.skill_name, "b");
    assert!(deserialized.source_url.is_none());
    assert!(deserialized.version.is_none());
}

// ===========================================================================
// 6. SkillSource precedence
// ===========================================================================

#[test]
fn skill_source_precedence_ordering() {
    assert!(SkillSource::Workspace.precedence() > SkillSource::AgentPersonal.precedence());
    assert!(SkillSource::AgentPersonal.precedence() > SkillSource::Personal.precedence());
    assert!(SkillSource::Personal.precedence() > SkillSource::Extra(std::path::PathBuf::from("/x")).precedence());
    assert!(SkillSource::Extra(std::path::PathBuf::from("/x")).precedence() > SkillSource::Bundled.precedence());

    assert_eq!(SkillSource::Workspace.precedence(), 5);
    assert_eq!(SkillSource::AgentPersonal.precedence(), 4);
    assert_eq!(SkillSource::Personal.precedence(), 3);
    assert_eq!(SkillSource::Extra(std::path::PathBuf::from("/any")).precedence(), 2);
    assert_eq!(SkillSource::Bundled.precedence(), 1);
}

// ===========================================================================
// 7. Multiple agents with different skills installed
// ===========================================================================

#[test]
fn multiple_agents_different_skills() {
    let tmp = TempDir::new().unwrap();
    let db = test_db(tmp.path());
    let store = SkillInstallStore::new(db);

    let agents = [("agent-a", vec!["deploy", "lint"]), ("agent-b", vec!["test-runner"]), ("agent-c", vec!["deploy", "test-runner", "lint"])];

    for (agent, skills) in &agents {
        for skill in skills {
            store
                .install(&SkillInstallation {
                    agent_id: agent.to_string(),
                    skill_name: skill.to_string(),
                    source_url: None,
                    installed_at: Utc::now(),
                    version: None,
                })
                .unwrap();
        }
    }

    let a_skills = store.list_for_agent("agent-a").unwrap();
    assert_eq!(a_skills.len(), 2);

    let b_skills = store.list_for_agent("agent-b").unwrap();
    assert_eq!(b_skills.len(), 1);
    assert_eq!(b_skills[0].skill_name, "test-runner");

    let c_skills = store.list_for_agent("agent-c").unwrap();
    assert_eq!(c_skills.len(), 3);

    assert!(store.is_installed("agent-a", "deploy").unwrap());
    assert!(!store.is_installed("agent-b", "deploy").unwrap());
    assert!(store.is_installed("agent-c", "deploy").unwrap());
}

// ===========================================================================
// 8. Activation edge cases (through manager)
// ===========================================================================

#[test]
fn activate_skill_dir_substitution() {
    let tmp = TempDir::new().unwrap();
    let skills = tmp.path().join("skills");
    let dir = skills.join("reader");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        "---\nname: reader\ndescription: Read files\n---\nRead ${SKILL_DIR}/data.json",
    )
    .unwrap();

    let mgr = SkillManager::new(workspace_loader(tmp.path()));
    let act = mgr.activate("reader", "").unwrap();
    let rendered = act.rendered_content.replace('\\', "/");
    assert!(
        rendered.contains("data.json"),
        "expected SKILL_DIR substitution, got: {rendered}"
    );
}

#[test]
fn activate_nonexistent_skill_returns_not_found() {
    let tmp = TempDir::new().unwrap();
    let mgr = SkillManager::new(workspace_loader(tmp.path()));
    let result = mgr.activate("nonexistent", "args");
    assert!(result.is_err());
    assert!(result.unwrap_err().is_not_found());
}

#[test]
fn activate_with_fork_context_and_allowed_tools() {
    let tmp = TempDir::new().unwrap();
    let skills = tmp.path().join("skills");
    let dir = skills.join("restricted");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        "---\nname: restricted\ndescription: Restricted skill\ncontext: fork\nallowed-tools:\n  - read\n  - write\nagent: sub-agent\n---\nRestricted body.",
    )
    .unwrap();

    let mgr = SkillManager::new(workspace_loader(tmp.path()));
    let act = mgr.activate("restricted", "").unwrap();

    assert!(act.fork_context);
    assert_eq!(act.allowed_tools, vec!["read", "write"]);
    assert_eq!(act.agent_type.as_deref(), Some("sub-agent"));
}
