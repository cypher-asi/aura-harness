use super::load::shared_store;
use super::*;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// The registry in `dir`, launched from `launch`.
fn open(dir: &Path, launch: &Path) -> AgentRegistry {
    AgentRegistry::load_or_init_in(dir, launch).unwrap()
}

fn names(tree: Vec<(&RegisteredAgent, usize)>) -> Vec<(String, usize)> {
    tree.into_iter().map(|(a, d)| (a.name.clone(), d)).collect()
}

fn project_names(registry: &AgentRegistry) -> Vec<String> {
    registry
        .listed_projects()
        .iter()
        .map(|p| p.name())
        .collect()
}

fn hex(zns: &str) -> String {
    Identity::new(zns, "x").agent_id.to_hex()
}

/// The registry stored in `dir`'s database, as the JSON of the in-memory
/// registry it loads as.
fn stored_body(dir: &Path) -> String {
    let rocks = shared_store(&dir.join(DB_DIR)).unwrap();
    let snapshot = AgentStore::for_store(&rocks).load().unwrap();
    serde_json::to_string_pretty(&persist::from_snapshot(snapshot)).unwrap()
}

/// A fixed archive time, `second` seconds into 2026.
fn at(second: u32) -> DateTime<Utc> {
    use chrono::TimeZone;
    Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, second).unwrap()
}

/// The code of `agent`, which every registered agent has.
fn code_of(agent: &RegisteredAgent) -> &str {
    agent
        .code
        .as_deref()
        .unwrap_or_else(|| panic!("{} has no code", agent.name))
}

/// Every agent's ZNS id with its code, in registry order.
fn codes(registry: &AgentRegistry) -> Vec<(String, Option<String>)> {
    registry
        .agents()
        .iter()
        .map(|a| (a.zns_id.clone(), a.code.clone()))
        .collect()
}

/// The prefix and number of `code`, which must be `<PREFIX><N>`.
fn split_code(code: &str) -> (&str, u64) {
    let digits = code.find(|c: char| c.is_ascii_digit()).unwrap();
    let (prefix, number) = code.split_at(digits);
    assert!(!number.starts_with('0'), "{code}");
    (prefix, number.parse().unwrap())
}

/// Checks the code invariants over every agent of `registry`: each has a
/// `<PREFIX><N>` code with a prefix of exactly three uppercase letters, no
/// two share one, all agents of a source key share a prefix and no two
/// keys do.
fn assert_codes_consistent(registry: &AgentRegistry) {
    let mut seen = HashSet::new();
    let mut prefix_of_key: HashMap<String, String> = HashMap::new();
    for agent in registry.agents() {
        let code = code_of(agent);
        assert!(seen.insert(code.to_string()), "{code} given twice");
        let (prefix, number) = split_code(code);
        assert_eq!(prefix.len(), 3, "{code}");
        assert!(prefix.bytes().all(|b| b.is_ascii_uppercase()), "{code}");
        assert!(number > 0, "{code}");
        let key = agent.definition.clone().unwrap_or_else(|| "agent".into());
        let known = prefix_of_key
            .entry(key.clone())
            .or_insert_with(|| prefix.to_string());
        assert_eq!(known, prefix, "{key} has two prefixes");
    }
    let prefixes: HashSet<&String> = prefix_of_key.values().collect();
    assert_eq!(prefixes.len(), prefix_of_key.len(), "{prefix_of_key:?}");
}

/// The highest numbers given, as saved in `dir`'s registry file.
fn saved_last_numbers(dir: &Path) -> serde_json::Value {
    let body = stored_body(dir);
    let saved: serde_json::Value = serde_json::from_str(&body).unwrap();
    saved["last_numbers"].clone()
}

/// The prefix table as saved in `dir`'s registry file.
fn saved_prefixes(dir: &Path) -> serde_json::Value {
    let body = stored_body(dir);
    let saved: serde_json::Value = serde_json::from_str(&body).unwrap();
    saved["prefixes"].clone()
}

/// `value[key]` as a string, `None` when absent or null.
fn json_str(value: &serde_json::Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

/// `value[key]` as a flag, `false` when absent.
fn json_flag(value: &serde_json::Value, key: &str) -> bool {
    value
        .get(key)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

fn json_time(value: &serde_json::Value) -> DateTime<Utc> {
    value["created_at"].as_str().unwrap().parse().unwrap()
}

/// Assert, field by field, that `registry` holds what the version 3
/// registry file `file` does, in its order.
fn assert_matches_file(registry: &AgentRegistry, file: &serde_json::Value) {
    assert_agents_match(registry, file);
    assert_projects_match(registry, file);
    assert_meta_matches(registry, file);
}

/// The agents of `registry` are those of `file`, field by field, in order.
fn assert_agents_match(registry: &AgentRegistry, file: &serde_json::Value) {
    let agents = file["agents"].as_array().unwrap();
    let zns: Vec<&str> = registry
        .agents()
        .iter()
        .map(|a| a.zns_id.as_str())
        .collect();
    let want: Vec<&str> = agents
        .iter()
        .map(|a| a["zns_id"].as_str().unwrap())
        .collect();
    assert_eq!(zns, want, "same agents in the same order");
    for (agent, want) in registry.agents().iter().zip(agents) {
        let id = &agent.zns_id;
        assert_eq!(Some(agent.name.clone()), json_str(want, "name"), "{id}");
        assert_eq!(agent.created_at, json_time(want), "{id}");
        assert_eq!(agent.definition, json_str(want, "definition"), "{id}");
        assert_eq!(agent.parent, json_str(want, "parent"), "{id}");
        assert_eq!(agent.project_id, json_str(want, "project_id"), "{id}");
        assert_eq!(
            agent.folder,
            json_str(want, "folder").map(PathBuf::from),
            "{id}"
        );
        assert_eq!(agent.archived, json_flag(want, "archived"), "{id}");
        assert_eq!(agent.removed, json_flag(want, "removed"), "{id}");
        assert_eq!(agent.title, json_str(want, "title"), "{id}");
        assert_eq!(agent.code, json_str(want, "code"), "{id}");
        assert_eq!(agent.subtitle, json_str(want, "subtitle"), "{id}");
    }
}

/// The projects of `registry` are those of `file`, in order.
fn assert_projects_match(registry: &AgentRegistry, file: &serde_json::Value) {
    let projects = file["projects"].as_array().unwrap();
    assert_eq!(registry.projects().len(), projects.len());
    for (project, want) in registry.projects().iter().zip(projects) {
        let id = &project.id;
        assert_eq!(Some(project.id.clone()), json_str(want, "id"));
        assert_eq!(
            Some(project.folder.clone()),
            json_str(want, "folder").map(PathBuf::from),
            "{id}"
        );
        assert_eq!(project.created_at, json_time(want), "{id}");
        assert_eq!(project.removed, json_flag(want, "removed"), "{id}");
    }
}

/// The version, active agent, prefixes and highest numbers of `registry`
/// are those of `file`.
fn assert_meta_matches(registry: &AgentRegistry, file: &serde_json::Value) {
    assert_eq!(
        u64::from(registry.file.version),
        file["version"].as_u64().unwrap()
    );
    assert_eq!(registry.file.active, json_str(file, "active"));
    assert_eq!(
        Some(registry.active().zns_id.clone()),
        json_str(file, "active")
    );
    assert_eq!(
        serde_json::to_value(registry.file.prefixes.to_map()).unwrap(),
        file["prefixes"]
    );
    assert_eq!(
        serde_json::to_value(registry.file.last_numbers.to_map()).unwrap(),
        file["last_numbers"]
    );
}

/// What a reload must keep: the list, the Archive, every project and
/// agent, and the active agent.
type State = (
    Vec<(String, usize)>,
    Vec<(String, usize)>,
    Vec<Project>,
    Vec<RegisteredAgent>,
    String,
);

fn state(registry: &AgentRegistry) -> State {
    let ids = |tree: Vec<(&RegisteredAgent, usize)>| {
        tree.into_iter()
            .map(|(a, d)| (a.agent_id().to_hex(), d))
            .collect()
    };
    (
        ids(registry.tree()),
        ids(registry.archived_tree()),
        registry.projects().to_vec(),
        registry.agents().to_vec(),
        registry.active().zns_id.clone(),
    )
}

mod archive;
mod code_migration;
mod codes;
mod compare_runs;
mod identity;
mod migration;
mod persistence;
mod prefixes;
mod projects;
mod titles;
mod tree;
