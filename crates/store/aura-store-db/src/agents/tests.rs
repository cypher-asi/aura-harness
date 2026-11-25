use super::*;
use crate::{FaultAt, RocksStore};
use tempfile::TempDir;

/// A plaintext and a sealed store, each in its own temp dir, labelled for
/// assertion messages.
fn stores() -> Vec<(&'static str, RocksStore, TempDir)> {
    let plain_dir = TempDir::new().unwrap();
    let plain = RocksStore::open(plain_dir.path(), false).unwrap();
    let sealed_dir = TempDir::new().unwrap();
    let cipher = Arc::new(SealCipher::new(&[5u8; 32]));
    let sealed = RocksStore::open_sealed(sealed_dir.path(), false, Some(cipher)).unwrap();
    vec![
        ("plaintext", plain, plain_dir),
        ("sealed", sealed, sealed_dir),
    ]
}

fn ts(secs: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(secs, 0).unwrap()
}

fn agent(zns_id: &str, position: u64) -> AgentRecord {
    AgentRecord {
        agent_id: derive_agent_id(zns_id),
        zns_id: zns_id.to_string(),
        name: format!("agent {zns_id}"),
        created_at: ts(1_700_000_000 + i64::try_from(position).unwrap()),
        position,
        definition: None,
        parent: None,
        project_id: None,
        folder: None,
        archived: false,
        removed: false,
        title: None,
        code: None,
        subtitle: None,
        archived_at: None,
    }
}

fn project(id: &str, position: u64) -> ProjectRecord {
    ProjectRecord {
        id: id.to_string(),
        folder: PathBuf::from(format!("/work/{id}")),
        created_at: ts(1_690_000_000),
        position,
        removed: false,
    }
}

fn meta(active: Option<AgentId>) -> RegistryMeta {
    RegistryMeta {
        version: 3,
        active,
        prefixes: BTreeMap::from([
            ("agent".to_string(), "AG".to_string()),
            ("rust-engineer".to_string(), "RE".to_string()),
        ]),
        last_numbers: BTreeMap::from([("AG".to_string(), 4), ("RE".to_string(), 36)]),
    }
}

/// Three agents (one spawned by another, with folder and code), two
/// projects and meta, listed out of position order.
fn sample_diff() -> RegistryDiff {
    let root = agent("zns-root", 0);
    let mut child = agent("zns-child", 2);
    child.definition = Some("rust-engineer".to_string());
    child.parent = Some(root.agent_id);
    child.project_id = Some("proj-a".to_string());
    child.folder = Some(PathBuf::from("/work/proj-a/sub"));
    child.code = Some("RE36".to_string());
    child.title = Some("Fix the build".to_string());
    child.subtitle = Some("tests".to_string());
    let mut other = agent("zns-other", 1);
    other.archived = true;
    other.code = Some("AG4".to_string());
    let mut removed_project = project("proj-b", 0);
    removed_project.removed = true;
    RegistryDiff {
        agents_put: vec![child, other, root.clone()],
        agents_del: Vec::new(),
        projects_put: vec![project("proj-a", 1), removed_project],
        projects_del: Vec::new(),
        meta: Some(meta(Some(root.agent_id))),
    }
}

/// What [`AgentStore::load`] should return after [`sample_diff`].
fn sorted_expected(diff: &RegistryDiff) -> RegistrySnapshot {
    let mut agents = diff.agents_put.clone();
    agents.sort_by_key(|a| a.position);
    let mut projects = diff.projects_put.clone();
    projects.sort_by_key(|p| p.position);
    RegistrySnapshot {
        agents,
        projects,
        meta: diff.meta.clone(),
    }
}

/// Seal `plain` the way the store does in its mode.
fn raw_value(store: &RocksStore, plain: &[u8]) -> Vec<u8> {
    match store.cipher() {
        Some(cipher) => cipher.seal(plain).unwrap(),
        None => plain.to_vec(),
    }
}

fn raw_get(store: &RocksStore, family: &str, key: &[u8]) -> Option<Vec<u8>> {
    let db = store.db_handle();
    let cf = db.cf_handle(family).unwrap();
    db.get_cf(&cf, key).unwrap()
}

#[test]
fn round_trip_returns_equal_values_sorted_by_position() {
    for (mode, store, _dir) in stores() {
        let agents = AgentStore::for_store(&store);
        let diff = sample_diff();
        agents.apply(&diff).unwrap();

        let loaded = agents.load().unwrap();
        assert_eq!(loaded, sorted_expected(&diff), "{mode}");
        let positions: Vec<u64> = loaded.agents.iter().map(|a| a.position).collect();
        assert_eq!(positions, vec![0, 1, 2], "{mode}");
        assert_eq!(
            loaded.agents[2].parent,
            Some(derive_agent_id("zns-root")),
            "{mode}"
        );
    }
}

#[test]
fn round_trip_stores_sealed_values_only_in_sealed_mode() {
    for (mode, store, _dir) in stores() {
        let agents = AgentStore::for_store(&store);
        agents.apply(&sample_diff()).unwrap();
        let id = derive_agent_id("zns-root");
        let rows = [
            raw_get(&store, cf::AGENTS, id.as_bytes()).unwrap(),
            raw_get(&store, cf::PROJECTS, b"proj-a").unwrap(),
            raw_get(&store, cf::REGISTRY_META, META_PREFIXES).unwrap(),
        ];
        for raw in rows {
            assert_eq!(
                SealCipher::is_sealed(&raw),
                store.cipher().is_some(),
                "{mode}"
            );
        }
    }
}

#[test]
fn equal_positions_are_ordered_by_key() {
    for (mode, store, _dir) in stores() {
        let agents = AgentStore::for_store(&store);
        let records = vec![agent("tie-1", 7), agent("tie-2", 7), agent("tie-3", 7)];
        agents
            .apply(&RegistryDiff {
                agents_put: records,
                ..RegistryDiff::default()
            })
            .unwrap();
        let ids: Vec<AgentId> = agents
            .load()
            .unwrap()
            .agents
            .iter()
            .map(|a| a.agent_id)
            .collect();
        let mut by_key = ids.clone();
        by_key.sort_by_key(|id| *id.as_bytes());
        assert_eq!(ids, by_key, "{mode}");
    }
}

#[test]
fn meta_absent_until_written_and_active_none_clears_it() {
    for (mode, store, _dir) in stores() {
        let agents = AgentStore::for_store(&store);
        assert_eq!(
            agents.load().unwrap(),
            RegistrySnapshot::default(),
            "{mode}"
        );

        agents
            .apply(&RegistryDiff {
                meta: Some(meta(Some(derive_agent_id("zns-root")))),
                ..RegistryDiff::default()
            })
            .unwrap();
        agents
            .apply(&RegistryDiff {
                meta: Some(meta(None)),
                ..RegistryDiff::default()
            })
            .unwrap();
        assert_eq!(agents.load().unwrap().meta, Some(meta(None)), "{mode}");
        assert_eq!(
            raw_get(&store, cf::REGISTRY_META, META_ACTIVE),
            None,
            "{mode}"
        );
    }
}

#[test]
fn load_skips_rows_whose_key_is_not_derived_from_zns_id() {
    for (mode, store, _dir) in stores() {
        let agents = AgentStore::for_store(&store);
        let good = agent("zns-good", 0);
        agents
            .apply(&RegistryDiff {
                agents_put: vec![good.clone()],
                ..RegistryDiff::default()
            })
            .unwrap();

        let db = store.db_handle();
        let cf_agents = db.cf_handle(cf::AGENTS).unwrap();
        // Key and `agent_id` agree, but neither is derived from `zns_id`.
        let mut forged = agent("zns-forged", 1);
        forged.agent_id = derive_agent_id("zns-someone-else");
        let plain = serde_json::to_vec(&forged).unwrap();
        db.put_cf(
            &cf_agents,
            forged.agent_id.as_bytes(),
            raw_value(&store, &plain),
        )
        .unwrap();
        // A valid record stored under another agent's key.
        let moved = agent("zns-moved", 2);
        let plain = serde_json::to_vec(&moved).unwrap();
        db.put_cf(
            &cf_agents,
            derive_agent_id("zns-elsewhere").as_bytes(),
            raw_value(&store, &plain),
        )
        .unwrap();
        // A key that is not 32 bytes.
        db.put_cf(&cf_agents, b"short", raw_value(&store, &plain))
            .unwrap();
        // A project row under a key other than its id.
        let cf_projects = db.cf_handle(cf::PROJECTS).unwrap();
        let plain = serde_json::to_vec(&project("proj-real", 0)).unwrap();
        db.put_cf(&cf_projects, b"proj-alias", raw_value(&store, &plain))
            .unwrap();

        let loaded = agents.load().unwrap();
        assert_eq!(loaded.agents, vec![good], "{mode}");
        assert!(loaded.projects.is_empty(), "{mode}");
    }
}

#[test]
fn apply_rejects_agent_id_not_derived_from_zns_id() {
    for (mode, store, _dir) in stores() {
        let agents = AgentStore::for_store(&store);
        let mut bad = agent("zns-bad", 1);
        bad.agent_id = derive_agent_id("zns-not-bad");
        let diff = RegistryDiff {
            agents_put: vec![agent("zns-fine", 0), bad],
            projects_put: vec![project("proj-a", 0)],
            ..RegistryDiff::default()
        };
        let err = agents.apply(&diff).unwrap_err();
        assert!(
            matches!(err, AgentStoreError::IdMismatch { .. }),
            "{mode}: {err}"
        );
        assert!(agents.is_empty().unwrap(), "{mode}");
        assert_eq!(
            agents.load().unwrap(),
            RegistrySnapshot::default(),
            "{mode}"
        );
    }
}

#[test]
fn apply_rejects_empty_project_id_and_writes_nothing() {
    for (mode, store, _dir) in stores() {
        let agents = AgentStore::for_store(&store);
        let diff = RegistryDiff {
            agents_put: vec![agent("zns-fine", 0)],
            projects_put: vec![project("", 0)],
            ..RegistryDiff::default()
        };
        let err = agents.apply(&diff).unwrap_err();
        assert!(
            matches!(err, AgentStoreError::InvalidProjectId(_)),
            "{mode}: {err}"
        );
        assert!(agents.is_empty().unwrap(), "{mode}");
    }
}

#[test]
fn diff_writes_only_changed_rows_and_deletes_remove_rows() {
    for (mode, store, _dir) in stores() {
        let agents = AgentStore::for_store(&store);
        assert!(agents.is_empty().unwrap(), "{mode}");
        assert!(RegistryDiff::default().is_empty(), "{mode}");
        agents.apply(&RegistryDiff::default()).unwrap();
        assert!(agents.is_empty().unwrap(), "{mode}");

        let diff = sample_diff();
        agents.apply(&diff).unwrap();
        assert!(!agents.is_empty().unwrap(), "{mode}");
        let before = agents.load().unwrap();
        let untouched_raw = raw_get(&store, cf::AGENTS, derive_agent_id("zns-root").as_bytes());

        // One put: only that row changes.
        let mut renamed = agent("zns-other", 1);
        renamed.name = "renamed".to_string();
        let one_put = RegistryDiff {
            agents_put: vec![renamed.clone()],
            ..RegistryDiff::default()
        };
        assert!(!one_put.is_empty(), "{mode}");
        agents.apply(&one_put).unwrap();
        let after = agents.load().unwrap();
        let mut expected = before.clone();
        expected.agents[1] = renamed;
        assert_eq!(after, expected, "{mode}");
        // Byte-identical: the untouched row was not rewritten (a rewrite
        // in sealed mode would get a fresh nonce).
        assert_eq!(
            raw_get(&store, cf::AGENTS, derive_agent_id("zns-root").as_bytes()),
            untouched_raw,
            "{mode}"
        );

        // Deletes remove exactly the named rows.
        agents
            .apply(&RegistryDiff {
                agents_del: vec![derive_agent_id("zns-other")],
                projects_del: vec!["proj-b".to_string()],
                ..RegistryDiff::default()
            })
            .unwrap();
        let after_del = agents.load().unwrap();
        let mut expected = after;
        expected.agents.remove(1);
        expected.projects.retain(|p| p.id != "proj-b");
        assert_eq!(after_del, expected, "{mode}");

        // A key both put and deleted in one diff ends up deleted.
        agents
            .apply(&RegistryDiff {
                agents_put: vec![agent("zns-flip", 9)],
                agents_del: vec![derive_agent_id("zns-flip")],
                ..RegistryDiff::default()
            })
            .unwrap();
        assert_eq!(agents.load().unwrap(), expected, "{mode}");

        agents
            .apply(&RegistryDiff {
                agents_del: expected.agents.iter().map(|a| a.agent_id).collect(),
                ..RegistryDiff::default()
            })
            .unwrap();
        assert!(agents.is_empty().unwrap(), "{mode}");
    }
}

#[test]
fn stage_into_caller_batch_matches_apply() {
    let diff = sample_diff();
    // Pairs of fresh stores in the same mode: one staged, one applied.
    for ((mode, store, _dir), (_, other, _other_dir)) in stores().into_iter().zip(stores()) {
        let agents = AgentStore::for_store(&store);
        let mut batch = WriteBatch::default();
        agents.stage(&diff, &mut batch).unwrap();
        assert!(agents.is_empty().unwrap(), "{mode}: staging writes nothing");
        store.db_handle().write(batch).unwrap();
        let staged = agents.load().unwrap();

        let applied_store = AgentStore::for_store(&other);
        applied_store.apply(&diff).unwrap();
        assert_eq!(staged, applied_store.load().unwrap(), "{mode}");
        assert_eq!(staged, sorted_expected(&diff), "{mode}");
    }
}

#[test]
fn stage_error_leaves_caller_batch_untouched() {
    for (mode, store, _dir) in stores() {
        let agents = AgentStore::for_store(&store);
        let mut batch = WriteBatch::default();
        agents
            .stage(
                &RegistryDiff {
                    agents_put: vec![agent("zns-first", 0)],
                    ..RegistryDiff::default()
                },
                &mut batch,
            )
            .unwrap();
        let staged_len = batch.len();

        let mut bad = agent("zns-bad", 1);
        bad.agent_id = derive_agent_id("zns-x");
        let bad_diff = RegistryDiff {
            agents_put: vec![agent("zns-second", 2), bad],
            ..RegistryDiff::default()
        };
        assert!(agents.stage(&bad_diff, &mut batch).is_err(), "{mode}");
        assert_eq!(batch.len(), staged_len, "{mode}");
    }
}

#[test]
fn fault_before_batch_write_writes_nothing() {
    for (mode, store, _dir) in stores() {
        let agents = AgentStore::for_store(&store);
        let err = agents
            .apply_with_fault(&sample_diff(), FaultAt::BeforeBatchWrite)
            .unwrap_err();
        assert!(
            matches!(
                err,
                AgentStoreError::InjectedFault(FaultAt::BeforeBatchWrite)
            ),
            "{mode}: {err}"
        );
        assert!(agents.is_empty().unwrap(), "{mode}");
        assert_eq!(
            agents.load().unwrap(),
            RegistrySnapshot::default(),
            "{mode}"
        );
    }
}

#[test]
fn fault_after_batch_write_commits_everything() {
    for (mode, store, _dir) in stores() {
        let agents = AgentStore::for_store(&store);
        let diff = sample_diff();
        let err = agents
            .apply_with_fault(&diff, FaultAt::AfterBatchWrite)
            .unwrap_err();
        assert!(
            matches!(
                err,
                AgentStoreError::InjectedFault(FaultAt::AfterBatchWrite)
            ),
            "{mode}: {err}"
        );
        assert_eq!(agents.load().unwrap(), sorted_expected(&diff), "{mode}");
    }
}

#[test]
fn fault_inside_batch_commits_only_agent_puts() {
    for (mode, store, _dir) in stores() {
        let agents = AgentStore::for_store(&store);
        // An existing project the diff deletes, to show deletes are lost.
        agents
            .apply(&RegistryDiff {
                projects_put: vec![project("proj-old", 5)],
                ..RegistryDiff::default()
            })
            .unwrap();
        let mut diff = sample_diff();
        diff.projects_del = vec!["proj-old".to_string()];

        let err = agents
            .apply_with_fault(&diff, FaultAt::InsideBatch)
            .unwrap_err();
        assert!(
            matches!(err, AgentStoreError::InjectedFault(FaultAt::InsideBatch)),
            "{mode}: {err}"
        );
        let loaded = agents.load().unwrap();
        assert_eq!(loaded.agents, sorted_expected(&diff).agents, "{mode}");
        assert_eq!(loaded.projects, vec![project("proj-old", 5)], "{mode}");
        assert_eq!(loaded.meta, None, "{mode}");
    }
}

#[test]
fn fault_hook_rejects_invalid_diff_before_writing() {
    for (mode, store, _dir) in stores() {
        let agents = AgentStore::for_store(&store);
        let mut bad = agent("zns-bad", 0);
        bad.agent_id = derive_agent_id("zns-x");
        let diff = RegistryDiff {
            agents_put: vec![bad],
            ..RegistryDiff::default()
        };
        let err = agents
            .apply_with_fault(&diff, FaultAt::AfterBatchWrite)
            .unwrap_err();
        assert!(
            matches!(err, AgentStoreError::IdMismatch { .. }),
            "{mode}: {err}"
        );
        assert!(agents.is_empty().unwrap(), "{mode}");
    }
}

#[test]
fn data_survives_reopen() {
    let key = [11u8; 32];
    for sealed in [false, true] {
        let dir = TempDir::new().unwrap();
        let cipher = || sealed.then(|| Arc::new(SealCipher::new(&key)));
        let diff = sample_diff();
        {
            let store = RocksStore::open_sealed(dir.path(), true, cipher()).unwrap();
            AgentStore::for_store(&store).apply(&diff).unwrap();
        }
        let store = RocksStore::open_sealed(dir.path(), true, cipher()).unwrap();
        let agents = AgentStore::for_store(&store);
        assert!(!agents.is_empty().unwrap(), "sealed={sealed}");
        assert_eq!(
            agents.load().unwrap(),
            sorted_expected(&diff),
            "sealed={sealed}"
        );
    }
}

#[test]
fn apply_durable_round_trips_and_survives_reopen() {
    for sealed in [false, true] {
        let dir = TempDir::new().unwrap();
        let cipher = || sealed.then(|| Arc::new(SealCipher::new(&[12u8; 32])));
        let diff = sample_diff();
        {
            // `sync_writes = false`: the durable write must not depend on it.
            let store = RocksStore::open_sealed(dir.path(), false, cipher()).unwrap();
            let agents = AgentStore::for_store(&store);
            agents.apply_durable(&diff).unwrap();
            assert_eq!(
                agents.load().unwrap(),
                sorted_expected(&diff),
                "sealed={sealed}"
            );
            agents.apply_durable(&RegistryDiff::default()).unwrap();
        }
        let store = RocksStore::open_sealed(dir.path(), false, cipher()).unwrap();
        assert_eq!(
            AgentStore::for_store(&store).load().unwrap(),
            sorted_expected(&diff),
            "sealed={sealed}"
        );
    }
}

#[test]
fn apply_durable_rejects_invalid_diff_and_writes_nothing() {
    for (mode, store, _dir) in stores() {
        let agents = AgentStore::for_store(&store);
        let mut diff = sample_diff();
        diff.projects_put.push(project("", 9));
        let err = agents.apply_durable(&diff).unwrap_err();
        assert!(
            matches!(err, AgentStoreError::InvalidProjectId(_)),
            "{mode}: {err}"
        );
        assert!(agents.is_empty().unwrap(), "{mode}");
    }
}

#[test]
fn sealed_rows_fail_closed_under_wrong_key() {
    let dir = TempDir::new().unwrap();
    {
        let cipher = Arc::new(SealCipher::new(&[1u8; 32]));
        let store = RocksStore::open_sealed(dir.path(), false, Some(cipher)).unwrap();
        AgentStore::for_store(&store).apply(&sample_diff()).unwrap();
    }
    let wrong = Arc::new(SealCipher::new(&[2u8; 32]));
    let store = RocksStore::open_sealed(dir.path(), false, Some(wrong)).unwrap();
    let err = AgentStore::for_store(&store).load().unwrap_err();
    assert!(matches!(err, AgentStoreError::Seal { .. }), "{err}");
}

#[test]
fn registry_column_families_are_known_and_sealed() {
    for family in [cf::AGENTS, cf::PROJECTS, cf::REGISTRY_META] {
        assert!(crate::SEALED_VALUE_CFS.contains(&family), "{family}");
        assert!(crate::migrate::ALL_CFS.contains(&family), "{family}");
    }
}
