
use super::*;
use crate::loader::SkillLoaderConfig;
use rocksdb::{ColumnFamilyDescriptor, DBWithThreadMode, MultiThreaded, Options};

fn install_store(dir: &std::path::Path) -> Arc<SkillInstallStore> {
    let mut options = Options::default();
    options.create_if_missing(true);
    options.create_missing_column_families(true);
    let columns = vec![ColumnFamilyDescriptor::new(
        aura_store_db::cf::AGENT_SKILLS,
        Options::default(),
    )];
    let db =
        DBWithThreadMode::<MultiThreaded>::open_cf_descriptors(&options, dir, columns).unwrap();
    Arc::new(SkillInstallStore::new(Arc::new(db)))
}

#[test]
fn create_with_agent_target_persists_canonical_binding() {
    let tmp = tempfile::tempdir().unwrap();
    let skills_dir = tmp.path().join("skills");
    let loader = SkillLoader::new(SkillLoaderConfig {
        personal_dir: Some(skills_dir.clone()),
        ..SkillLoaderConfig::default()
    });
    let mut manager = SkillManager::new(loader);

    let skill = manager
        .create_with_agent_target(
            "request-review",
            "Ask the reviewer",
            "Delegate this review.",
            true,
            Some("00000000-0000-0000-0000-000000000002"),
            Some("Security Reviewer"),
        )
        .unwrap();

    assert_eq!(
        skill.frontmatter.agent_target_id.as_deref(),
        Some("00000000-0000-0000-0000-000000000002")
    );
    assert_eq!(
        skill.frontmatter.agent_target_name.as_deref(),
        Some("Security Reviewer")
    );
    let content = std::fs::read_to_string(skills_dir.join("request-review/SKILL.md")).unwrap();
    assert!(content.contains("agent-target-id: \"00000000-0000-0000-0000-000000000002\""));
    assert!(content.contains("agent-target-name: \"Security Reviewer\""));
}

#[test]
fn installed_bound_skill_injects_exact_send_to_agent_target() {
    let tmp = tempfile::tempdir().unwrap();
    let skills_dir = tmp.path().join("skills");
    let store_dir = tmp.path().join("store");
    let loader = SkillLoader::new(SkillLoaderConfig {
        personal_dir: Some(skills_dir),
        ..SkillLoaderConfig::default()
    });
    let mut manager = SkillManager::with_install_store(loader, install_store(&store_dir));
    manager
        .create_with_agent_target(
            "request-review",
            "Ask the reviewer",
            "Delegate this review.",
            true,
            Some("00000000-0000-0000-0000-000000000002"),
            Some("Security Reviewer"),
        )
        .unwrap();

    let source_agent =
        AgentId::from_uuid(uuid::Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap());
    manager
        .install_for_agent(source_agent, "request-review", None, vec![], vec![])
        .unwrap();
    let mut prompt = "You are the project lead.".to_string();
    let injected = manager.inject_agent_skills("00000000-0000-0000-0000-000000000001", &mut prompt);

    assert_eq!(injected.len(), 1);
    assert!(prompt.contains("Delegate this review."));
    assert!(prompt.contains(
        "<skill_agent_target name=\"Security Reviewer\" \
             agent_id=\"00000000-0000-0000-0000-000000000002\"/>"
    ));
    assert!(prompt.contains("call `send_to_agent` with this exact agent_id"));
}
