use super::*;

#[test]
fn definitions_reserve_their_prefix_before_any_instance_exists() {
    let dir = tempfile::tempdir().unwrap();
    let launch = tempfile::tempdir().unwrap();
    let mut registry = open(dir.path(), launch.path());
    let keys = ["rust-engineer", "reviewer", "explorer"];
    assert_eq!(
        registry.reserve_prefixes(&keys).unwrap(),
        ["REN", "REV", "EXP"]
    );
    assert_eq!(
        saved_prefixes(dir.path()),
        serde_json::json!({
            "agent": "AGE", "rust-engineer": "REN", "reviewer": "REV", "explorer": "EXP",
        })
    );

    let engineer = registry.create_instance("rust-engineer", None).unwrap();
    let reviewer = registry.create_instance("reviewer", None).unwrap();
    assert_eq!((code_of(&engineer), code_of(&reviewer)), ("REN1", "REV1"));

    let mut reloaded = open(dir.path(), launch.path());
    let body = stored_body(dir.path());
    assert_eq!(
        reloaded.reserve_prefixes(&keys).unwrap(),
        ["REN", "REV", "EXP"]
    );
    let explorer = reloaded.create_instance("explorer", None).unwrap();
    assert_eq!(code_of(&explorer), "EXP1");
    assert_codes_consistent(&reloaded);
    // Asking again reserves nothing new, so the file is left alone.
    let after = stored_body(dir.path());
    assert_ne!(after, body, "creating the explorer saved");
    reloaded.reserve_prefixes(&keys).unwrap();
    assert_eq!(stored_body(dir.path()), after);
}

#[test]
fn prefixes_are_reserved_in_the_order_asked() {
    let dir = tempfile::tempdir().unwrap();
    let launch = tempfile::tempdir().unwrap();
    let mut registry = open(dir.path(), launch.path());
    assert_eq!(
        registry
            .reserve_prefixes(&["research-engineer", "rust-engineer"])
            .unwrap(),
        ["REN", "RUE"]
    );
    let engineer = registry.create_instance("rust-engineer", None).unwrap();
    assert_eq!(code_of(&engineer), "RUE1");
}

#[test]
fn a_registry_without_a_prefix_table_rebuilds_it_from_its_codes() {
    let dir = tempfile::tempdir().unwrap();
    let launch = tempfile::tempdir().unwrap();
    let agent = |zns: &str, name: &str, definition: Option<&str>, code: &str| {
        serde_json::json!({
            "zns_id": zns, "name": name, "created_at": "2026-10-31T01:48:50Z",
            "definition": definition, "project_id": "p", "code": code,
        })
    };
    let file = serde_json::json!({
        "version": REGISTRY_VERSION,
        "projects": [{"id": "p", "folder": launch.path(), "created_at": "2026-10-31T01:48:50Z"}],
        "agents": [
            agent("0://terminal/rev", "reviewer #1", Some("reviewer"), "REW1"),
            agent("0://terminal/re36", "rust-engineer #36", Some("rust-engineer"), "RUS36"),
            agent("0://terminal/agent", "Agent", None, "AGT5"),
            agent("0://terminal/rev2", "reviewer #2", Some("reviewer"), "REW2"),
            // Hand-edited: `REW` is the reviewer's, so this code is dropped
            // and the key gets a prefix of its own.
            agent("0://terminal/re", "re #1", Some("re"), "REW7"),
        ],
    });
    fs::write(dir.path().join(REGISTRY_FILE), file.to_string()).unwrap();

    let mut registry = open(dir.path(), launch.path());
    assert_eq!(
        saved_prefixes(dir.path()),
        serde_json::json!({"reviewer": "REW", "rust-engineer": "RUS", "agent": "AGT", "re": "REE"})
    );
    assert_eq!(code_of(&registry.agents()[4]), "REE1");
    assert_codes_consistent(&registry);
    let keys = ["rust-engineer", "reviewer", "agent", "explorer", "re"];
    assert_eq!(
        registry.reserve_prefixes(&keys).unwrap(),
        ["RUS", "REW", "AGT", "EXP", "REE"]
    );
    let engineer = registry.create_instance("rust-engineer", None).unwrap();
    assert_eq!(code_of(&engineer), "RUS37");
    let reviewer = registry.create_instance("reviewer", None).unwrap();
    assert_eq!(code_of(&reviewer), "REW3");
}

#[test]
fn reserved_prefixes_are_unique_across_definitions() {
    let dir = tempfile::tempdir().unwrap();
    let launch = tempfile::tempdir().unwrap();
    let mut registry = open(dir.path(), launch.path());
    let keys = [
        "re",
        "research-engineer",
        "reviewer",
        "rust-engineer",
        "rust_engineer",
        "explorer",
        "explore",
        "ex",
        "e",
        "frontend-engineer",
        "agent",
        "--",
        "tests",
        "test",
    ];
    let prefixes = registry.reserve_prefixes(&keys).unwrap();
    let unique: HashSet<&String> = prefixes.iter().collect();
    assert_eq!(unique.len(), keys.len(), "{prefixes:?}");
    assert_eq!(prefixes[..3], ["REE", "REN", "REV"]);
    assert!(
        prefixes.iter().all(|prefix| prefix.len() == 3),
        "{prefixes:?}"
    );

    let mut reloaded = open(dir.path(), launch.path());
    assert_eq!(reloaded.reserve_prefixes(&keys).unwrap(), prefixes);
    // A key asked twice gets the same prefix both times.
    assert_eq!(
        reloaded
            .reserve_prefixes(&["reviewer", "reviewer"])
            .unwrap(),
        ["REV", "REV"]
    );
    for (key, prefix) in keys
        .iter()
        .zip(&prefixes)
        .filter(|(key, _)| **key != "agent")
    {
        let instance = reloaded.create_instance(key, None).unwrap();
        assert_eq!(code_of(&instance), format!("{prefix}1"), "{key}");
    }
    assert_codes_consistent(&reloaded);
}

#[test]
fn codes_serialize_only_when_present() {
    let mut agent = AgentRegistry::new_agent("rust-engineer #36".to_string());
    agent.definition = Some("rust-engineer".to_string());
    let json = serde_json::to_value(&agent).unwrap();
    assert!(json.get("code").is_none(), "{json}");
    let back: RegisteredAgent = serde_json::from_value(json).unwrap();
    assert_eq!(back.code, None);

    agent.code = Some("REN36".to_string());
    let json = serde_json::to_string(&agent).unwrap();
    assert!(json.contains(r#""code":"REN36""#), "{json}");
    let back: RegisteredAgent = serde_json::from_str(&json).unwrap();
    assert_eq!(back, agent);
    assert_eq!(back.code.as_deref(), Some("REN36"));
}
