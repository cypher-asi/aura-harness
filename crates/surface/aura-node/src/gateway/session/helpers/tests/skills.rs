//! A session's skills: listed in its prompt as an index, loaded in full
//! with `load_skill`.

use super::support::test_context;
use crate::gateway::session::skills::{inject_skills, SessionSkills};
use crate::gateway::session::Session;
use aura_context_skills::loader::SkillLoaderConfig;
use aura_context_skills::{SkillInstallStore, SkillLoader, SkillManager};
use aura_core_types::AgentId;
use aura_exec_tools::skills::SkillSource;
use aura_store_db::RocksStore;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

const AGENT: &str = "00000000-0000-0000-0000-000000000001";
const BODY: &str = "Tag the release, build it, then publish the notes.";

/// A context whose skill manager has `deploy` installed for [`AGENT`],
/// and a session of that agent.
fn session_with_a_skill() -> (Session, crate::gateway::session::WsContext) {
    let dir = tempfile::tempdir().unwrap();
    let rocks = RocksStore::open(dir.path().join("db"), false).unwrap();
    let store = Arc::new(SkillInstallStore::new(rocks.db_handle().clone()));
    let loader = SkillLoader::new(SkillLoaderConfig {
        personal_dir: Some(dir.path().join("skills")),
        ..SkillLoaderConfig::default()
    });
    let mut manager = SkillManager::with_install_store(loader, store);
    manager
        .create("deploy", "Ship a release.", BODY, true)
        .unwrap();
    let agent = AgentId::from_uuid(uuid::Uuid::parse_str(AGENT).unwrap());
    manager
        .install_for_agent(agent, "deploy", None, vec![], vec![])
        .unwrap();
    std::mem::forget(dir);
    let mut ctx = test_context();
    ctx.skill_manager = Some(Arc::new(RwLock::new(manager)));
    let mut session = Session::new(PathBuf::from("/tmp/aura"));
    session.skill_agent_id = Some(AGENT.to_string());
    (session, ctx)
}

#[test]
fn a_session_lists_its_skills_without_their_bodies() {
    let (session, ctx) = session_with_a_skill();
    let mut config = session.agent_loop_config();
    let before = config.system_prompt.len();
    let names = inject_skills(&session, &ctx, &mut config);
    assert_eq!(names, ["deploy"]);
    assert!(config.system_prompt.contains("name=\"deploy\""));
    assert!(
        !config.system_prompt.contains(BODY),
        "no body on every turn"
    );
    assert_eq!(config.skills_chars, config.system_prompt.len() - before);
}

#[test]
fn load_skill_returns_an_installed_skill_in_full() {
    let (session, ctx) = session_with_a_skill();
    let skills = SessionSkills::of(&session, &ctx).expect("it has skills");
    assert!(skills.load(None, "deploy", None).unwrap().contains(BODY));
    let unknown = skills.load(None, "launch", None).unwrap_err();
    assert!(unknown.contains("your skills: deploy"), "{unknown}");

    let mut other = Session::new(PathBuf::from("/tmp/aura"));
    other.skill_agent_id = Some("00000000-0000-0000-0000-000000000009".into());
    assert!(
        SessionSkills::of(&other, &ctx).is_none(),
        "nothing installed: no tool"
    );
}
