use super::*;

#[test]
fn diff_writes_only_changed_rows() {
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
        let one_put = RegistryDiff {
            agents_put: vec![renamed_other()],
            ..RegistryDiff::default()
        };
        assert!(!one_put.is_empty(), "{mode}");
        agents.apply(&one_put).unwrap();
        let after = agents.load().unwrap();
        let mut expected = before.clone();
        expected.agents[1] = renamed_other();
        assert_eq!(after, expected, "{mode}");
        // Byte-identical: the untouched row was not rewritten (a rewrite
        // in sealed mode would get a fresh nonce).
        assert_eq!(
            raw_get(&store, cf::AGENTS, derive_agent_id("zns-root").as_bytes()),
            untouched_raw,
            "{mode}"
        );
    }
}

#[test]
fn diff_deletes_remove_rows() {
    for (mode, store, _dir) in stores() {
        let agents = AgentStore::for_store(&store);
        agents.apply(&sample_diff()).unwrap();
        agents
            .apply(&RegistryDiff {
                agents_put: vec![renamed_other()],
                ..RegistryDiff::default()
            })
            .unwrap();
        let after = agents.load().unwrap();

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
