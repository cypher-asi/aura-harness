use super::*;
use crate::cf;
use crate::{FaultAt, RocksStore};
use chrono::{DateTime, Utc};
use rocksdb::WriteBatch;
use std::collections::BTreeMap;
use std::path::PathBuf;
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

/// `zns-other` from [`sample_diff`], renamed.
fn renamed_other() -> AgentRecord {
    let mut renamed = agent("zns-other", 1);
    renamed.name = "renamed".to_string();
    renamed
}

mod diff;
mod durability;
mod faults;
mod round_trip;
