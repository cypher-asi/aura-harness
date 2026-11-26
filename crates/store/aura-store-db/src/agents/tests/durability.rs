use super::*;

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
