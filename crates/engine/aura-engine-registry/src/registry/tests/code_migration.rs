use super::*;

#[test]
fn a_registry_without_codes_gets_them_on_load_and_keeps_them() {
    let launch = tempfile::tempdir().unwrap();
    let agent = |zns: &str, name: &str, definition: Option<&str>| {
        serde_json::json!({
            "zns_id": zns, "name": name, "created_at": "2026-10-31T01:48:50Z",
            "definition": definition, "project_id": "p",
        })
    };
    let file = serde_json::json!({
        "version": REGISTRY_VERSION,
        "projects": [{"id": "p", "folder": launch.path(), "created_at": "2026-10-31T01:48:50Z"}],
        "agents": [
            agent("0://terminal/agent", "Agent", None),
            agent("0://terminal/re36", "rust-engineer #36", Some("rust-engineer")),
            agent("0://terminal/rev", "reviewer #1", Some("reviewer")),
            agent("0://terminal/agent2", "Agent 2", None),
        ],
    })
    .to_string();
    assert!(!file.contains("\"code\""), "{file}");

    let load = || {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(REGISTRY_FILE), &file).unwrap();
        let registry = open(dir.path(), launch.path());
        (dir, codes(&registry))
    };
    let (dir, first) = load();
    let (_other, second) = load();
    let expected = [
        ("0://terminal/agent", "AGE1"),
        ("0://terminal/re36", "REN36"),
        ("0://terminal/rev", "REV1"),
        ("0://terminal/agent2", "AGE2"),
    ]
    .map(|(zns, code)| (zns.to_string(), Some(code.to_string())));
    assert_eq!(first, expected);
    assert_eq!(second, first, "the same registry backfills the same codes");

    let body = stored_body(dir.path());
    let saved: serde_json::Value = serde_json::from_str(&body).unwrap();
    let saved: Vec<&str> = saved["agents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["code"].as_str().unwrap())
        .collect();
    assert_eq!(saved, ["AGE1", "REN36", "REV1", "AGE2"]);
    assert_eq!(codes(&open(dir.path(), launch.path())), first);
}

#[test]
fn backfilling_keeps_the_codes_a_registry_has() {
    let dir = tempfile::tempdir().unwrap();
    let launch = tempfile::tempdir().unwrap();
    let agent = |zns: &str, name: &str, definition: Option<&str>, code: Option<&str>| {
        serde_json::json!({
            "zns_id": zns, "name": name, "created_at": "2026-10-31T01:48:50Z",
            "definition": definition, "project_id": "p", "code": code,
        })
    };
    let file = serde_json::json!({
        "version": REGISTRY_VERSION,
        "projects": [{"id": "p", "folder": launch.path(), "created_at": "2026-10-31T01:48:50Z"}],
        "agents": [
            agent("0://terminal/re36", "rust-engineer #36", Some("rust-engineer"), None),
            agent("0://terminal/rev", "reviewer #1", Some("reviewer"), Some("REN1")),
            agent("0://terminal/agent", "Agent", None, Some("AGE5")),
            agent("0://terminal/agent2", "Agent 2", None, None),
            agent("0://terminal/rev2", "reviewer #2", Some("reviewer"), None),
        ],
    });
    fs::write(dir.path().join(REGISTRY_FILE), file.to_string()).unwrap();

    let registry = open(dir.path(), launch.path());
    let got: Vec<&str> = registry.agents().iter().map(code_of).collect();
    // `REN` is the reviewer's (a hand edit), coded later in the list but
    // coded already, so the engineer takes its next candidate.
    assert_eq!(got, ["RUE36", "REN1", "AGE5", "AGE6", "REN2"]);
    assert_codes_consistent(&registry);
}

#[test]
fn older_codes_and_prefixes_are_migrated_on_load_and_kept() {
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
            agent("0://terminal/agent", "Agent", None, "AG-1"),
            agent("0://terminal/re36", "rust-engineer #36", Some("rust-engineer"), "RE-36"),
            // Three letters: joined up and kept.
            agent("0://terminal/rev", "reviewer #1", Some("reviewer"), "REV-1"),
            agent("0://terminal/res", "research-engineer #1", Some("research-engineer"), "RE2-1"),
            // A two-letter code in the current format is dropped too.
            agent("0://terminal/re37", "rust-engineer #37", Some("rust-engineer"), "RE36"),
        ],
        "prefixes": {
            "agent": "AG", "rust-engineer": "RE", "reviewer": "REVI", "research-engineer": "RE2",
        },
    })
    .to_string();

    let load = || {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(REGISTRY_FILE), &file).unwrap();
        let registry = open(dir.path(), launch.path());
        (dir, codes(&registry))
    };
    let (dir, first) = load();
    let (_other, second) = load();
    let expected = [
        ("0://terminal/agent", "AGE1"),
        ("0://terminal/re36", "REN36"),
        ("0://terminal/rev", "REV1"),
        ("0://terminal/res", "REE1"),
        ("0://terminal/re37", "REN37"),
    ]
    .map(|(zns, code)| (zns.to_string(), Some(code.to_string())));
    assert_eq!(first, expected);
    assert_eq!(second, first, "the same registry migrates the same way");
    assert_eq!(
        saved_prefixes(dir.path()),
        serde_json::json!({
            "agent": "AGE", "rust-engineer": "REN", "reviewer": "REV", "research-engineer": "REE",
        })
    );

    // Saved migrated, so loading again changes nothing.
    let body = stored_body(dir.path());
    let reloaded = open(dir.path(), launch.path());
    assert_eq!(codes(&reloaded), first);
    assert_codes_consistent(&reloaded);
    assert_eq!(stored_body(dir.path()), body);
}

#[test]
fn codes_stay_valid_and_unique_over_many_definitions() {
    let dir = tempfile::tempdir().unwrap();
    let launch = tempfile::tempdir().unwrap();
    let mut registry = open(dir.path(), launch.path());
    let words = [
        "rust", "research", "review", "re", "engineer", "explorer", "e", "agent", "x", "test",
    ];
    let keys: Vec<String> = (0..100)
        .map(|i| match (words[i / 10], words[i % 10]) {
            (a, b) if a == b => a.to_string(),
            (a, b) => format!("{a}-{b}"),
        })
        .collect();
    for key in &keys {
        registry.create_instance(key, None).unwrap();
    }
    for key in &keys[..10] {
        registry.create_instance(key, None).unwrap();
    }
    assert_eq!(registry.agents().len(), 1 + 110);
    assert_codes_consistent(&registry);
    let reloaded = open(dir.path(), launch.path());
    assert_eq!(codes(&reloaded), codes(&registry));
}
