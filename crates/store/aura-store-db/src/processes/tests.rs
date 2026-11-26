use super::*;
use chrono::Utc;
use rocksdb::{ColumnFamilyDescriptor, IteratorMode, Options};

fn test_db(dir: &std::path::Path) -> Arc<DBWithThreadMode<MultiThreaded>> {
    let mut opts = Options::default();
    opts.create_if_missing(true);
    opts.create_missing_column_families(true);
    let cfs = vec![
        ColumnFamilyDescriptor::new(crate::cf::PROCESSES, Options::default()),
        ColumnFamilyDescriptor::new(crate::cf::PROCESS_RUNS, Options::default()),
    ];
    Arc::new(DBWithThreadMode::<MultiThreaded>::open_cf_descriptors(&opts, dir, cfs).unwrap())
}

fn new_process(name: &str) -> NewProcess {
    NewProcess {
        name: name.into(),
        description: Some("nightly report".into()),
        cron: "0 3 * * *".into(),
        prompt: "Summarize yesterday's commits and email the team.".into(),
        config: None,
        enabled: true,
    }
}

#[test]
fn crud_roundtrip_plaintext() {
    let dir = tempfile::tempdir().unwrap();
    let store = ProcessStore::new(test_db(dir.path()));

    let created = store.create(new_process("nightly")).unwrap();
    assert!(created.enabled);
    assert!(created.next_run_at.unwrap() > Utc::now());
    assert!(created.last_run_at.is_none());

    let fetched = store.get(&created.id).unwrap().unwrap();
    assert_eq!(fetched.name, "nightly");
    assert_eq!(fetched.prompt, created.prompt);
    assert_eq!(fetched.created_at, created.created_at);

    // Update: rename + disable; created_at preserved.
    let updated = store
        .update(
            &created.id,
            ProcessUpdate {
                name: Some("nightly-v2".into()),
                enabled: Some(false),
                ..ProcessUpdate::default()
            },
        )
        .unwrap();
    assert_eq!(updated.name, "nightly-v2");
    assert!(!updated.enabled);
    assert_eq!(updated.created_at, created.created_at);
    assert!(updated.updated_at >= created.updated_at);
    // Prompt untouched by a partial update.
    assert_eq!(updated.prompt, created.prompt);

    assert_eq!(store.list().unwrap().len(), 1);
    assert!(store.delete(&created.id).unwrap());
    assert!(store.get(&created.id).unwrap().is_none());
    assert!(!store.delete(&created.id).unwrap());
}

#[test]
fn invalid_inputs_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let store = ProcessStore::new(test_db(dir.path()));

    let mut bad_cron = new_process("p");
    bad_cron.cron = "not a cron".into();
    assert!(matches!(
        store.create(bad_cron),
        Err(ProcessError::InvalidCron(_))
    ));

    let mut empty_name = new_process("");
    empty_name.name = "  ".into();
    assert!(matches!(
        store.create(empty_name),
        Err(ProcessError::InvalidName(_))
    ));

    let mut empty_prompt = new_process("p");
    empty_prompt.prompt = String::new();
    assert!(matches!(
        store.create(empty_prompt),
        Err(ProcessError::InvalidPrompt(_))
    ));

    let mut huge_prompt = new_process("p");
    huge_prompt.prompt = "x".repeat(MAX_PROCESS_PROMPT_BYTES + 1);
    assert!(matches!(
        store.create(huge_prompt),
        Err(ProcessError::InvalidPrompt(_))
    ));

    // Update with a bad cron is rejected and leaves the record alone.
    let created = store.create(new_process("ok")).unwrap();
    assert!(matches!(
        store.update(
            &created.id,
            ProcessUpdate {
                cron: Some("garbage".into()),
                ..ProcessUpdate::default()
            }
        ),
        Err(ProcessError::InvalidCron(_))
    ));
    assert_eq!(store.get(&created.id).unwrap().unwrap().cron, "0 3 * * *");
}

#[test]
fn cron_five_and_six_field_accepted() {
    // Standard 5-field (normalized with a 0-seconds prefix).
    assert!(validate_cron("*/5 * * * *").is_ok());
    // 6-field with seconds.
    assert!(validate_cron("30 */10 * * * *").is_ok());
    // 7-field with year.
    assert!(validate_cron("0 0 4 * * * 2030").is_ok());
    assert!(validate_cron("").is_err());
    assert!(validate_cron("61 * * * *").is_err());

    let next = next_run_after("0 3 * * *", Utc::now()).unwrap().unwrap();
    assert!(next > Utc::now());
    assert_eq!(next.format("%H:%M:%S").to_string(), "03:00:00");
}

/// Sealed mode: prompt bytes must not appear in plaintext anywhere
/// in the raw on-disk column family (same pattern as the vault's
/// sealed-at-rest test).
#[test]
fn sealed_at_rest_prompt_not_plaintext_on_disk() {
    let dir = tempfile::tempdir().unwrap();
    let db = test_db(dir.path());
    let cipher = Arc::new(SealCipher::new(&[7u8; 32]));
    let store = ProcessStore::with_cipher(Arc::clone(&db), Some(cipher));

    let prompt = b"check the production dashboards and rotate keys";
    let mut input = new_process("sealed");
    input.prompt = String::from_utf8(prompt.to_vec()).unwrap();
    let created = store.create(input).unwrap();

    // Roundtrip still works through the store.
    assert_eq!(
        store.get(&created.id).unwrap().unwrap().prompt.as_bytes(),
        prompt
    );

    let cf = db.cf_handle(crate::cf::PROCESSES).unwrap();
    let raw = db
        .iterator_cf(&cf, IteratorMode::Start)
        .next()
        .unwrap()
        .unwrap()
        .1;
    assert!(SealCipher::is_sealed(&raw));
    assert!(
        !raw.windows(prompt.len()).any(|w| w == prompt.as_slice()),
        "sealed bytes must not contain the plaintext prompt"
    );

    // Run records are sealed too.
    let run = store.start_run(&created.id).unwrap();
    store
        .finish_run(
            &created.id,
            &run.run_id,
            ProcessRunStatus::Failure,
            None,
            Some("secret-failure-detail".into()),
        )
        .unwrap();
    let runs_cf = db.cf_handle(crate::cf::PROCESS_RUNS).unwrap();
    let raw_run = db
        .iterator_cf(&runs_cf, IteratorMode::Start)
        .next()
        .unwrap()
        .unwrap()
        .1;
    assert!(SealCipher::is_sealed(&raw_run));
    let needle = b"secret-failure-detail";
    assert!(!raw_run
        .windows(needle.len())
        .any(|w| w == needle.as_slice()));
}

#[test]
fn run_lifecycle_and_bookkeeping() {
    let dir = tempfile::tempdir().unwrap();
    let store = ProcessStore::new(test_db(dir.path()));
    let created = store.create(new_process("runs")).unwrap();

    let run = store.start_run(&created.id).unwrap();
    assert_eq!(run.status, ProcessRunStatus::Running);
    assert!(run.finished_at.is_none());

    // start_run stamps last_run_at and keeps next_run_at fresh.
    let after = store.get(&created.id).unwrap().unwrap();
    assert!(after.last_run_at.is_some());
    assert!(after.next_run_at.unwrap() > Utc::now());

    let finished = store
        .finish_run(
            &created.id,
            &run.run_id,
            ProcessRunStatus::Success,
            Some("done".into()),
            None,
        )
        .unwrap();
    assert_eq!(finished.status, ProcessRunStatus::Success);
    assert!(finished.finished_at.is_some());

    let runs = store.list_runs(&created.id).unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].summary.as_deref(), Some("done"));

    // Unknown run id errors.
    assert!(matches!(
        store.finish_run(&created.id, "nope", ProcessRunStatus::Failure, None, None),
        Err(ProcessError::RunNotFound(_))
    ));
}

#[test]
fn run_history_capped_at_max() {
    let dir = tempfile::tempdir().unwrap();
    let store = ProcessStore::new(test_db(dir.path()));
    let created = store.create(new_process("capped")).unwrap();

    let total = MAX_PROCESS_RUNS_KEPT + 7;
    let mut run_ids = Vec::new();
    for _ in 0..total {
        run_ids.push(store.start_run(&created.id).unwrap().run_id);
    }

    let runs = store.list_runs(&created.id).unwrap();
    assert_eq!(runs.len(), MAX_PROCESS_RUNS_KEPT, "history is capped");
    // Newest-first listing: the most recent run id comes first and
    // the oldest 7 were pruned.
    assert_eq!(runs[0].run_id, *run_ids.last().unwrap());
    let kept: std::collections::HashSet<_> = runs.iter().map(|r| r.run_id.clone()).collect();
    for pruned in &run_ids[..7] {
        assert!(!kept.contains(pruned), "oldest runs must be pruned");
    }
}

#[test]
fn delete_removes_run_history() {
    let dir = tempfile::tempdir().unwrap();
    let store = ProcessStore::new(test_db(dir.path()));
    let created = store.create(new_process("cleanup")).unwrap();
    store.start_run(&created.id).unwrap();
    store.start_run(&created.id).unwrap();
    assert_eq!(store.list_runs(&created.id).unwrap().len(), 2);

    assert!(store.delete(&created.id).unwrap());
    assert!(store.list_runs(&created.id).unwrap().is_empty());
}

#[test]
fn trigger_metadata_contains_no_sensitive_fields() {
    let dir = tempfile::tempdir().unwrap();
    let store = ProcessStore::new(test_db(dir.path()));
    let mut input = new_process("exportable");
    input.prompt = "ULTRA-SENSITIVE-INSTRUCTION".into();
    let mut config = serde_json::Map::new();
    config.insert(
        "target".into(),
        serde_json::Value::String("CONFIDENTIAL-TARGET".into()),
    );
    input.config = Some(config);
    let created = store.create(input).unwrap();
    store.start_run(&created.id).unwrap();

    let metas = store.trigger_metadata().unwrap();
    assert_eq!(metas.len(), 1);
    assert_eq!(metas[0].process_id, created.id);
    assert_eq!(metas[0].cron, created.cron);
    assert!(metas[0].enabled);
    assert!(metas[0].next_run_at.is_some());

    // The serialized export must not carry any in-TEE payload.
    let json = serde_json::to_string(&metas).unwrap();
    assert!(!json.contains("ULTRA-SENSITIVE-INSTRUCTION"));
    assert!(!json.contains("CONFIDENTIAL-TARGET"));
    assert!(!json.contains("prompt"));
    assert!(!json.contains("config"));
    assert!(!json.contains("summary"));
}

#[test]
fn debug_redacts_prompt_and_config() {
    let mut config = serde_json::Map::new();
    config.insert("k".into(), serde_json::Value::String("hidden-cfg".into()));
    let record = ProcessRecord {
        id: "p1".into(),
        name: "n".into(),
        description: None,
        cron: "0 3 * * *".into(),
        prompt: "very-secret-instruction".into(),
        config: Some(config),
        enabled: true,
        created_at: Utc::now(),
        updated_at: Utc::now(),
        last_run_at: None,
        next_run_at: None,
    };
    let rendered = format!("{record:?}");
    assert!(rendered.contains("<redacted>"));
    assert!(!rendered.contains("very-secret-instruction"));
    assert!(!rendered.contains("hidden-cfg"));
}

#[test]
fn sealed_wrong_key_fails() {
    let dir = tempfile::tempdir().unwrap();
    let db = test_db(dir.path());
    let store =
        ProcessStore::with_cipher(Arc::clone(&db), Some(Arc::new(SealCipher::new(&[1u8; 32]))));
    let created = store.create(new_process("locked")).unwrap();

    let other = ProcessStore::with_cipher(db, Some(Arc::new(SealCipher::new(&[2u8; 32]))));
    assert!(other.get(&created.id).is_err());
}
