use super::*;

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
