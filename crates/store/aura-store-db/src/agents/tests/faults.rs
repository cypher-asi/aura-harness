use super::*;

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
