use super::*;
use crate::agents::{derive_agent_id, AgentRecord, RegistryDiff};
use crate::cf;
use crate::channels::keys::*;
use crate::{FaultAt, RocksStore};
use aura_core_types::AgentId;
use chrono::{DateTime, Utc};
use rocksdb::{IteratorMode, WriteBatch};
use std::path::Path;
use tempfile::TempDir;

const SEAL_KEY: [u8; 32] = [5u8; 32];

fn open(sealed: bool, path: &Path) -> RocksStore {
    if sealed {
        let cipher = Arc::new(SealCipher::new(&SEAL_KEY));
        RocksStore::open_sealed(path, false, Some(cipher)).unwrap()
    } else {
        RocksStore::open(path, false).unwrap()
    }
}

/// A plaintext and a sealed store, each in its own temp dir, labelled for
/// assertion messages.
fn stores() -> Vec<(&'static str, RocksStore, TempDir)> {
    let plain_dir = TempDir::new().unwrap();
    let plain = open(false, plain_dir.path());
    let sealed_dir = TempDir::new().unwrap();
    let sealed = open(true, sealed_dir.path());
    vec![
        ("plaintext", plain, plain_dir),
        ("sealed", sealed, sealed_dir),
    ]
}

fn ts(secs: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(1_700_000_000 + secs, 0).unwrap()
}

fn agent(zns_id: &str, position: u64) -> AgentRecord {
    AgentRecord {
        agent_id: derive_agent_id(zns_id),
        zns_id: zns_id.to_string(),
        name: format!("agent {zns_id}"),
        created_at: ts(i64::try_from(position).unwrap()),
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

fn new_channel(slug: &str) -> NewChannel {
    NewChannel {
        slug: slug.to_string(),
        title: format!("Channel {slug}"),
        description: format!("about {slug}"),
        project_id: None,
    }
}

fn new_channel_in(slug: &str, project_id: &str) -> NewChannel {
    NewChannel {
        project_id: Some(project_id.to_string()),
        ..new_channel(slug)
    }
}

/// An update that only sets (`Some`) or clears (`None`) the project.
fn set_project(project_id: Option<&str>) -> ChannelUpdate {
    ChannelUpdate {
        project_id: Some(project_id.map(str::to_string)),
        ..ChannelUpdate::default()
    }
}

fn raw_put(store: &RocksStore, family: &str, key: &[u8], value: &[u8]) {
    let db = store.db_handle();
    db.put_cf(&db.cf_handle(family).unwrap(), key, value)
        .unwrap();
}

fn raw_delete(store: &RocksStore, family: &str, key: &[u8]) {
    let db = store.db_handle();
    db.delete_cf(&db.cf_handle(family).unwrap(), key).unwrap();
}

/// Keys of every `p/` row left in the channels family.
fn link_keys(store: &RocksStore) -> Vec<Vec<u8>> {
    raw_values(store, cf::CHANNELS)
        .into_iter()
        .map(|(key, _)| key)
        .filter(|key| key.starts_with(LINK_PREFIX.as_bytes()))
        .collect()
}

fn member(agent_id: AgentId, role: MemberRole, parent: Option<AgentId>, at: i64) -> MemberRecord {
    MemberRecord {
        agent_id,
        role,
        parent,
        joined_at: ts(at),
    }
}

fn post(text: &str, at_ms: u64) -> NewPost {
    NewPost {
        author: PostAuthor::User,
        text: text.to_string(),
        mentions: Vec::new(),
        reply_to: None,
        at_ms,
    }
}

fn agent_ids(store: &RocksStore) -> Vec<AgentId> {
    AgentStore::for_store(store)
        .load()
        .unwrap()
        .agents
        .into_iter()
        .map(|a| a.agent_id)
        .collect()
}

fn raw_values(store: &RocksStore, family: &str) -> Vec<(Vec<u8>, Vec<u8>)> {
    let db = store.db_handle();
    let cf = db.cf_handle(family).unwrap();
    db.iterator_cf(&cf, IteratorMode::Start)
        .map(|item| {
            let (k, v) = item.unwrap();
            (k.to_vec(), v.to_vec())
        })
        .collect()
}

fn is_empty_cf(store: &RocksStore, family: &str) -> bool {
    raw_values(store, family).is_empty()
}

#[test]
fn create_commits_coordinator_agent_and_channel_together() {
    for (mode, store, _dir) in stores() {
        let channels = ChannelStore::for_store(&store);
        let coordinator = agent("zns-coord", 0);
        let record = channels
            .create(new_channel("zode"), &coordinator, ts(100))
            .unwrap();

        assert_eq!(record.slug, "zode", "{mode}");
        assert_eq!(record.title, "Channel zode", "{mode}");
        assert_eq!(record.description, "about zode", "{mode}");
        assert_eq!(record.coordinator, coordinator.agent_id, "{mode}");
        assert_eq!(record.created_at, ts(100), "{mode}");
        assert_eq!(record.updated_at, ts(100), "{mode}");
        assert!(!record.archived, "{mode}");
        assert!(uuid::Uuid::parse_str(&record.id).is_ok(), "{mode}");

        let snapshot = AgentStore::for_store(&store).load().unwrap();
        assert_eq!(snapshot.agents, vec![coordinator.clone()], "{mode}");

        assert_eq!(
            channels.get(&record.id).unwrap(),
            Some(record.clone()),
            "{mode}"
        );
        assert_eq!(
            channels.by_slug("zode").unwrap(),
            Some(record.clone()),
            "{mode}"
        );
        assert_eq!(channels.list().unwrap(), vec![record.clone()], "{mode}");
        assert_eq!(
            channels.members(&record.id).unwrap(),
            vec![member(
                coordinator.agent_id,
                MemberRole::Coordinator,
                None,
                100
            )],
            "{mode}"
        );
        assert_eq!(
            channels.channel_of(&coordinator.agent_id).unwrap(),
            Some(record.id.clone()),
            "{mode}"
        );
        assert_eq!(channels.by_slug("other").unwrap(), None, "{mode}");
        assert_eq!(channels.get("missing").unwrap(), None, "{mode}");
    }
}

#[test]
fn create_rejects_coordinator_already_in_a_channel() {
    for (mode, store, _dir) in stores() {
        let channels = ChannelStore::for_store(&store);
        let coordinator = agent("zns-coord", 0);
        let first = channels
            .create(new_channel("first"), &coordinator, ts(1))
            .unwrap();
        let err = channels
            .create(new_channel("second"), &coordinator, ts(2))
            .unwrap_err();
        assert!(
            matches!(&err, ChannelError::AlreadyMember { channel, .. } if *channel == first.id),
            "{mode}: {err:?}"
        );
        assert_eq!(channels.by_slug("second").unwrap(), None, "{mode}");
        assert_eq!(channels.list().unwrap().len(), 1, "{mode}");
    }
}

#[test]
fn create_with_fault_is_all_or_nothing_except_documented_partial() {
    for (mode, store, _dir) in stores() {
        let channels = ChannelStore::for_store(&store);

        // BeforeBatchWrite: nothing at all.
        let before = agent("zns-before", 0);
        let err = channels
            .create_with_fault(
                new_channel("before"),
                &before,
                ts(1),
                FaultAt::BeforeBatchWrite,
            )
            .unwrap_err();
        assert!(
            matches!(err, ChannelError::InjectedFault(FaultAt::BeforeBatchWrite)),
            "{mode}: {err:?}"
        );
        assert!(agent_ids(&store).is_empty(), "{mode}");
        assert_eq!(channels.by_slug("before").unwrap(), None, "{mode}");
        assert!(channels.list().unwrap().is_empty(), "{mode}");
        assert_eq!(
            channels.channel_of(&before.agent_id).unwrap(),
            None,
            "{mode}"
        );
        assert!(is_empty_cf(&store, cf::CHANNELS), "{mode}");
        assert!(is_empty_cf(&store, cf::CHANNEL_MEMBERS), "{mode}");

        // AfterBatchWrite: everything, despite the error.
        let after = agent("zns-after", 1);
        let err = channels
            .create_with_fault(
                new_channel("after"),
                &after,
                ts(2),
                FaultAt::AfterBatchWrite,
            )
            .unwrap_err();
        assert!(
            matches!(err, ChannelError::InjectedFault(FaultAt::AfterBatchWrite)),
            "{mode}: {err:?}"
        );
        assert_eq!(agent_ids(&store), vec![after.agent_id], "{mode}");
        let record = channels.by_slug("after").unwrap().unwrap();
        assert_eq!(record.coordinator, after.agent_id, "{mode}");
        assert_eq!(channels.list().unwrap(), vec![record.clone()], "{mode}");
        assert_eq!(
            channels.channel_of(&after.agent_id).unwrap(),
            Some(record.id.clone()),
            "{mode}"
        );
        assert_eq!(
            channels.members(&record.id).unwrap(),
            vec![member(after.agent_id, MemberRole::Coordinator, None, 2)],
            "{mode}"
        );

        // InsideBatch: only the coordinator's agent row.
        let inside = agent("zns-inside", 2);
        let err = channels
            .create_with_fault(new_channel("inside"), &inside, ts(3), FaultAt::InsideBatch)
            .unwrap_err();
        assert!(
            matches!(err, ChannelError::InjectedFault(FaultAt::InsideBatch)),
            "{mode}: {err:?}"
        );
        assert_eq!(
            agent_ids(&store),
            vec![after.agent_id, inside.agent_id],
            "{mode}"
        );
        assert_eq!(channels.by_slug("inside").unwrap(), None, "{mode}");
        assert_eq!(channels.list().unwrap(), vec![record], "{mode}");
        assert_eq!(
            channels.channel_of(&inside.agent_id).unwrap(),
            None,
            "{mode}"
        );
    }
}

#[test]
fn valid_slug_accepts_and_rejects() {
    for ok in ["zode", "a-1", "a", "0", "-", &"a".repeat(40)] {
        assert!(valid_slug(ok), "{ok:?}");
    }
    for bad in [
        "Zode",
        "ZODE",
        "has space",
        "",
        &"a".repeat(41),
        "a/b",
        "a_b",
        "é",
    ] {
        assert!(!valid_slug(bad), "{bad:?}");
    }
}

#[test]
fn create_rejects_invalid_slugs_and_writes_nothing() {
    for (mode, store, _dir) in stores() {
        let channels = ChannelStore::for_store(&store);
        let long = "a".repeat(41);
        for (i, bad) in ["Zode", "has space", "", long.as_str(), "a/b"]
            .into_iter()
            .enumerate()
        {
            let coordinator = agent(&format!("zns-bad-{i}"), u64::try_from(i).unwrap());
            let err = channels
                .create(new_channel(bad), &coordinator, ts(1))
                .unwrap_err();
            assert!(
                matches!(&err, ChannelError::InvalidSlug(s) if s == bad),
                "{mode} {bad:?}: {err:?}"
            );
            assert_eq!(channels.by_slug(bad).unwrap(), None, "{mode} {bad:?}");
        }
        assert!(agent_ids(&store).is_empty(), "{mode}");
        assert!(is_empty_cf(&store, cf::CHANNELS), "{mode}");
        assert!(is_empty_cf(&store, cf::CHANNEL_MEMBERS), "{mode}");
    }
}

#[test]
fn duplicate_slug_is_taken_and_writes_nothing() {
    for (mode, store, _dir) in stores() {
        let channels = ChannelStore::for_store(&store);
        let first_coord = agent("zns-first", 0);
        let first = channels
            .create(new_channel("zode"), &first_coord, ts(1))
            .unwrap();

        let second_coord = agent("zns-second", 1);
        let err = channels
            .create(new_channel("zode"), &second_coord, ts(2))
            .unwrap_err();
        assert!(
            matches!(&err, ChannelError::SlugTaken(s) if s == "zode"),
            "{mode}: {err:?}"
        );
        assert_eq!(agent_ids(&store), vec![first_coord.agent_id], "{mode}");
        assert_eq!(
            channels.channel_of(&second_coord.agent_id).unwrap(),
            None,
            "{mode}"
        );
        assert_eq!(channels.list().unwrap(), vec![first.clone()], "{mode}");
        assert_eq!(channels.by_slug("zode").unwrap(), Some(first), "{mode}");
    }
}

#[test]
fn members_and_reverse_index() {
    for (mode, store, _dir) in stores() {
        let channels = ChannelStore::for_store(&store);
        let coord = agent("zns-coord", 0);
        let ch = channels
            .create(new_channel("one"), &coord, ts(100))
            .unwrap();
        let other_coord = agent("zns-other-coord", 1);
        let other = channels
            .create(new_channel("two"), &other_coord, ts(100))
            .unwrap();

        let full = member(
            derive_agent_id("zns-full"),
            MemberRole::Agent,
            Some(coord.agent_id),
            300,
        );
        let sub = member(
            derive_agent_id("zns-sub"),
            MemberRole::Subagent,
            Some(full.agent_id),
            200,
        );
        channels.add_member(&ch.id, &full).unwrap();
        channels.add_member(&ch.id, &sub).unwrap();

        let coord_member = member(coord.agent_id, MemberRole::Coordinator, None, 100);
        assert_eq!(
            channels.members(&ch.id).unwrap(),
            vec![coord_member.clone(), sub.clone(), full.clone()],
            "{mode}"
        );
        for id in [coord.agent_id, full.agent_id, sub.agent_id] {
            assert_eq!(
                channels.channel_of(&id).unwrap(),
                Some(ch.id.clone()),
                "{mode}"
            );
        }

        // Idempotent in the same channel: keeps the existing record.
        let mut again = full.clone();
        again.joined_at = ts(999);
        again.role = MemberRole::Subagent;
        channels.add_member(&ch.id, &again).unwrap();
        assert_eq!(
            channels.members(&ch.id).unwrap(),
            vec![coord_member.clone(), sub.clone(), full.clone()],
            "{mode}"
        );

        // Another channel: refused, nothing written there.
        let err = channels.add_member(&other.id, &full).unwrap_err();
        assert!(
            matches!(&err, ChannelError::AlreadyMember { agent, channel }
                if *agent == full.agent_id.to_hex() && *channel == ch.id),
            "{mode}: {err:?}"
        );
        assert_eq!(
            channels.members(&other.id).unwrap(),
            vec![member(
                other_coord.agent_id,
                MemberRole::Coordinator,
                None,
                100
            )],
            "{mode}"
        );
        assert_eq!(
            channels.channel_of(&full.agent_id).unwrap(),
            Some(ch.id.clone()),
            "{mode}"
        );

        // Unknown channel.
        let stray = member(derive_agent_id("zns-stray"), MemberRole::Agent, None, 1);
        let err = channels.add_member("no-such-channel", &stray).unwrap_err();
        assert!(
            matches!(&err, ChannelError::UnknownChannel(id) if id == "no-such-channel"),
            "{mode}: {err:?}"
        );
        assert_eq!(
            channels.channel_of(&stray.agent_id).unwrap(),
            None,
            "{mode}"
        );

        // Removing from the wrong channel leaves the reverse index alone.
        channels.remove_member(&other.id, &full.agent_id).unwrap();
        assert_eq!(
            channels.channel_of(&full.agent_id).unwrap(),
            Some(ch.id.clone()),
            "{mode}"
        );

        // Remove deletes both m/ and a/ rows.
        channels.remove_member(&ch.id, &full.agent_id).unwrap();
        assert_eq!(channels.channel_of(&full.agent_id).unwrap(), None, "{mode}");
        assert_eq!(
            channels.members(&ch.id).unwrap(),
            vec![coord_member.clone(), sub.clone()],
            "{mode}"
        );
        let member_row = member_key(&ch.id, &full.agent_id);
        let reverse_row = reverse_key(&full.agent_id);
        assert!(
            raw_values(&store, cf::CHANNEL_MEMBERS)
                .iter()
                .all(|(k, _)| *k != member_row && *k != reverse_row),
            "{mode}"
        );
        // Removing a non-member is a no-op.
        channels.remove_member(&ch.id, &full.agent_id).unwrap();

        // After removal it may join another channel.
        channels.add_member(&other.id, &full).unwrap();
        assert_eq!(
            channels.channel_of(&full.agent_id).unwrap(),
            Some(other.id.clone()),
            "{mode}"
        );

        // stage_member + write.
        let staged = member(derive_agent_id("zns-staged"), MemberRole::Agent, None, 400);
        let mut batch = WriteBatch::default();
        channels.stage_member(&ch.id, &staged, &mut batch).unwrap();
        assert_eq!(
            channels.channel_of(&staged.agent_id).unwrap(),
            None,
            "{mode}"
        );
        channels.write(batch).unwrap();
        assert_eq!(
            channels.channel_of(&staged.agent_id).unwrap(),
            Some(ch.id.clone()),
            "{mode}"
        );
        assert_eq!(
            channels.members(&ch.id).unwrap(),
            vec![coord_member, sub, staged],
            "{mode}"
        );

        let mut batch = WriteBatch::default();
        let err = channels
            .stage_member("bad/id", &stray, &mut batch)
            .unwrap_err();
        assert!(matches!(err, ChannelError::InvalidId(_)), "{mode}: {err:?}");
        assert_eq!(batch.len(), 0, "{mode}");
    }
}

#[test]
fn posts_are_time_ordered_with_unique_growing_seqs_across_reopen() {
    for (mode, store, dir) in stores() {
        let sealed = store.cipher().is_some();
        let channels = ChannelStore::for_store(&store);
        let a = channels
            .create(new_channel("a"), &agent("zns-a", 0), ts(1))
            .unwrap();
        let b = channels
            .create(new_channel("b"), &agent("zns-b", 1), ts(1))
            .unwrap();

        let p300 = channels.append_post(&a.id, post("three", 300)).unwrap();
        let p100 = channels.append_post(&a.id, post("one", 100)).unwrap();
        let pb = channels.append_post(&b.id, post("other", 150)).unwrap();
        let p200 = channels.append_post(&a.id, post("two", 200)).unwrap();

        let seqs = [p300.seq, p100.seq, pb.seq, p200.seq];
        assert!(seqs.windows(2).all(|w| w[0] < w[1]), "{mode}: {seqs:?}");
        assert_eq!(p300.channel_id, a.id, "{mode}");
        assert_eq!(p300.text, "three", "{mode}");
        assert_eq!(p300.author, PostAuthor::User, "{mode}");

        assert_eq!(
            channels.posts(&a.id, None).unwrap(),
            vec![p100.clone(), p200.clone(), p300.clone()],
            "{mode}"
        );
        assert_eq!(
            channels.posts(&a.id, Some(150)).unwrap(),
            vec![p200.clone(), p300.clone()],
            "{mode}"
        );
        assert_eq!(
            channels.posts(&a.id, Some(200)).unwrap(),
            vec![p300.clone()],
            "{mode}"
        );
        assert!(
            channels.posts(&a.id, Some(300)).unwrap().is_empty(),
            "{mode}"
        );
        assert!(
            channels.posts(&a.id, Some(u64::MAX)).unwrap().is_empty(),
            "{mode}"
        );
        assert_eq!(
            channels.posts(&b.id, None).unwrap(),
            vec![pb.clone()],
            "{mode}"
        );

        let err = channels
            .append_post("no-such-channel", post("x", 1))
            .unwrap_err();
        assert!(
            matches!(err, ChannelError::UnknownChannel(_)),
            "{mode}: {err:?}"
        );

        // Reopen: a fresh store seeds the counter from disk.
        drop(channels);
        drop(store);
        let store = open(sealed, dir.path());
        let channels = ChannelStore::for_store(&store);
        let same_ms = channels.append_post(&a.id, post("again", 200)).unwrap();
        assert!(same_ms.seq > p200.seq, "{mode}");
        assert!(seqs.iter().all(|&s| s < same_ms.seq), "{mode}");
        let p250 = channels.append_post(&a.id, post("late", 250)).unwrap();
        assert!(p250.seq > same_ms.seq, "{mode}");
        assert_eq!(
            channels.posts(&a.id, None).unwrap(),
            vec![p100, p200, same_ms, p250, p300],
            "{mode}"
        );
        assert_eq!(channels.posts(&b.id, None).unwrap(), vec![pb], "{mode}");
    }
}

#[test]
fn project_links() {
    for (mode, store, _dir) in stores() {
        let channels = ChannelStore::for_store(&store);
        let ch = channels
            .create(new_channel("zode"), &agent("zns-coord", 0), ts(1))
            .unwrap();
        let other = channels
            .create(new_channel("other"), &agent("zns-other", 1), ts(1))
            .unwrap();

        let b = channels.link_project(&ch.id, "proj-b", ts(10)).unwrap();
        let a = channels.link_project(&ch.id, "proj-a", ts(20)).unwrap();
        assert_eq!(b.project_id, "proj-b", "{mode}");
        assert_eq!(b.linked_at, ts(10), "{mode}");
        assert_eq!(
            channels.projects(&ch.id).unwrap(),
            vec![a.clone(), b.clone()],
            "{mode}"
        );
        assert!(channels.projects(&other.id).unwrap().is_empty(), "{mode}");

        // Relinking refreshes linked_at.
        let b2 = channels.link_project(&ch.id, "proj-b", ts(30)).unwrap();
        assert_eq!(
            channels.projects(&ch.id).unwrap(),
            vec![a.clone(), b2],
            "{mode}"
        );

        channels.unlink_project(&ch.id, "proj-b").unwrap();
        assert_eq!(channels.projects(&ch.id).unwrap(), vec![a], "{mode}");
        // Unlinking something not linked is a no-op.
        channels.unlink_project(&ch.id, "proj-b").unwrap();

        let err = channels
            .link_project("no-such-channel", "proj-a", ts(1))
            .unwrap_err();
        assert!(
            matches!(&err, ChannelError::UnknownChannel(id) if id == "no-such-channel"),
            "{mode}: {err:?}"
        );
        assert!(
            channels.projects("no-such-channel").unwrap().is_empty(),
            "{mode}"
        );
        let err = channels.link_project(&ch.id, "", ts(1)).unwrap_err();
        assert!(
            matches!(err, ChannelError::InvalidProjectId(_)),
            "{mode}: {err:?}"
        );
        let err = channels.projects("bad/id").unwrap_err();
        assert!(matches!(err, ChannelError::InvalidId(_)), "{mode}: {err:?}");
    }
}

#[test]
fn update_and_archive() {
    for (mode, store, _dir) in stores() {
        let channels = ChannelStore::for_store(&store);
        let ch = channels
            .create(new_channel("zode"), &agent("zns-coord", 0), ts(1))
            .unwrap();

        let updated = channels
            .update(
                &ch.id,
                ChannelUpdate {
                    title: Some("New title".to_string()),
                    description: Some("New description".to_string()),
                    project_id: None,
                },
                ts(5),
            )
            .unwrap();
        assert_eq!(updated.title, "New title", "{mode}");
        assert_eq!(updated.description, "New description", "{mode}");
        assert_eq!(updated.updated_at, ts(5), "{mode}");
        assert_eq!(updated.created_at, ts(1), "{mode}");
        assert_eq!(
            channels.get(&ch.id).unwrap(),
            Some(updated.clone()),
            "{mode}"
        );

        // None leaves a field as it is.
        let partial = channels
            .update(
                &ch.id,
                ChannelUpdate {
                    title: None,
                    description: Some("Only this".to_string()),
                    project_id: None,
                },
                ts(6),
            )
            .unwrap();
        assert_eq!(partial.title, "New title", "{mode}");
        assert_eq!(partial.description, "Only this", "{mode}");

        let archived = channels.archive(&ch.id, ts(9)).unwrap();
        assert!(archived.archived, "{mode}");
        assert_eq!(archived.updated_at, ts(9), "{mode}");
        assert_eq!(channels.list().unwrap(), vec![archived.clone()], "{mode}");
        assert_eq!(channels.by_slug("zode").unwrap(), Some(archived), "{mode}");

        let err = channels
            .update("no-such-channel", ChannelUpdate::default(), ts(1))
            .unwrap_err();
        assert!(
            matches!(err, ChannelError::UnknownChannel(_)),
            "{mode}: {err:?}"
        );
        let err = channels.archive("no-such-channel", ts(1)).unwrap_err();
        assert!(
            matches!(err, ChannelError::UnknownChannel(_)),
            "{mode}: {err:?}"
        );
    }
}

#[test]
fn list_sorts_by_created_at_then_slug() {
    for (mode, store, _dir) in stores() {
        let channels = ChannelStore::for_store(&store);
        let late = channels
            .create(new_channel("aaa"), &agent("zns-1", 0), ts(9))
            .unwrap();
        let tie_b = channels
            .create(new_channel("bbb"), &agent("zns-2", 1), ts(1))
            .unwrap();
        let tie_a = channels
            .create(new_channel("abb"), &agent("zns-3", 2), ts(1))
            .unwrap();
        assert_eq!(channels.list().unwrap(), vec![tie_a, tie_b, late], "{mode}");
    }
}

#[test]
fn channel_column_families_are_registered_and_sealed() {
    for family in [cf::CHANNELS, cf::CHANNEL_MEMBERS, cf::CHANNEL_POSTS] {
        assert!(crate::SEALED_VALUE_CFS.contains(&family), "{family}");
        assert!(crate::migrate::ALL_CFS.contains(&family), "{family}");
    }

    for (mode, store, _dir) in stores() {
        let sealed = store.cipher().is_some();
        let channels = ChannelStore::for_store(&store);
        let ch = channels
            .create(new_channel("zode"), &agent("zns-coord", 0), ts(1))
            .unwrap();
        channels.link_project(&ch.id, "proj-a", ts(2)).unwrap();
        channels
            .append_post(&ch.id, post("secret words", 10))
            .unwrap();

        for family in [cf::CHANNELS, cf::CHANNEL_MEMBERS, cf::CHANNEL_POSTS] {
            let rows = raw_values(&store, family);
            assert!(!rows.is_empty(), "{mode} {family}");
            for (key, value) in rows {
                let parsed = serde_json::from_slice::<serde_json::Value>(&value);
                assert_eq!(
                    parsed.is_ok(),
                    !sealed,
                    "{mode} {family} {}",
                    String::from_utf8_lossy(&key)
                );
                if sealed {
                    for needle in [&b"zode"[..], b"secret", b"proj-a", ch.id.as_bytes()] {
                        assert!(
                            !value.windows(needle.len()).any(|w| w == needle),
                            "{mode} {family}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn last_read_is_kept_per_channel_and_left_out_of_the_list() {
    for (mode, store, _dir) in stores() {
        let channels = ChannelStore::for_store(&store);
        let ch = channels
            .create(new_channel("zode"), &agent("zns-coord", 0), ts(1))
            .unwrap();
        let other = channels
            .create(new_channel("other"), &agent("zns-other", 1), ts(1))
            .unwrap();
        assert_eq!(channels.seen(&ch.id).unwrap(), None, "{mode}");
        channels.set_seen(&ch.id, 1_234).unwrap();
        channels.set_seen(&ch.id, 5_678).unwrap();
        assert_eq!(channels.seen(&ch.id).unwrap(), Some(5_678), "{mode}");
        assert_eq!(channels.seen(&other.id).unwrap(), None, "{mode}");
        assert_eq!(channels.list().unwrap().len(), 2, "{mode}");
        assert!(matches!(
            channels.set_seen("a/b", 1),
            Err(ChannelError::InvalidId(_))
        ));
    }
}

#[test]
fn create_with_a_project_round_trips_and_without_one_has_none() {
    for (mode, store, _dir) in stores() {
        let channels = ChannelStore::for_store(&store);
        let with = channels
            .create(
                new_channel_in("with", "proj-a"),
                &agent("zns-with", 0),
                ts(1),
            )
            .unwrap();
        let without = channels
            .create(new_channel("without"), &agent("zns-without", 1), ts(2))
            .unwrap();

        assert_eq!(with.project_id.as_deref(), Some("proj-a"), "{mode}");
        assert_eq!(without.project_id, None, "{mode}");
        assert_eq!(
            channels.get(&with.id).unwrap(),
            Some(with.clone()),
            "{mode}"
        );
        assert_eq!(
            channels.by_slug("with").unwrap(),
            Some(with.clone()),
            "{mode}"
        );
        assert_eq!(
            channels.get(&without.id).unwrap(),
            Some(without.clone()),
            "{mode}"
        );
        assert_eq!(channels.list().unwrap(), vec![with, without], "{mode}");
    }
}

#[test]
fn create_rejects_blank_project_ids_and_writes_nothing() {
    for (mode, store, _dir) in stores() {
        let channels = ChannelStore::for_store(&store);
        for (i, bad) in ["", "  ", "\t\n"].into_iter().enumerate() {
            let coordinator = agent(&format!("zns-bad-{i}"), u64::try_from(i).unwrap());
            let err = channels
                .create(new_channel_in("zode", bad), &coordinator, ts(1))
                .unwrap_err();
            assert!(
                matches!(&err, ChannelError::InvalidProjectId(p) if p == bad),
                "{mode} {bad:?}: {err:?}"
            );
            assert_eq!(
                channels.channel_of(&coordinator.agent_id).unwrap(),
                None,
                "{mode} {bad:?}"
            );
        }
        assert!(agent_ids(&store).is_empty(), "{mode}");
        assert!(is_empty_cf(&store, cf::CHANNELS), "{mode}");
        assert!(is_empty_cf(&store, cf::CHANNEL_MEMBERS), "{mode}");

        // The slug is still free.
        let record = channels
            .create(new_channel_in("zode", "proj-a"), &agent("zns-ok", 9), ts(2))
            .unwrap();
        assert_eq!(channels.by_slug("zode").unwrap(), Some(record), "{mode}");
    }
}

#[test]
fn create_with_fault_is_all_or_nothing_with_a_project() {
    for (mode, store, _dir) in stores() {
        let channels = ChannelStore::for_store(&store);

        // BeforeBatchWrite: nothing at all.
        let before = agent("zns-before", 0);
        let err = channels
            .create_with_fault(
                new_channel_in("before", "proj-before"),
                &before,
                ts(1),
                FaultAt::BeforeBatchWrite,
            )
            .unwrap_err();
        assert!(
            matches!(err, ChannelError::InjectedFault(FaultAt::BeforeBatchWrite)),
            "{mode}: {err:?}"
        );
        assert!(agent_ids(&store).is_empty(), "{mode}");
        assert!(is_empty_cf(&store, cf::CHANNELS), "{mode}");
        assert!(is_empty_cf(&store, cf::CHANNEL_MEMBERS), "{mode}");

        // InsideBatch: only the coordinator's agent row, no channel, so
        // no project either.
        let inside = agent("zns-inside", 1);
        let err = channels
            .create_with_fault(
                new_channel_in("inside", "proj-inside"),
                &inside,
                ts(2),
                FaultAt::InsideBatch,
            )
            .unwrap_err();
        assert!(
            matches!(err, ChannelError::InjectedFault(FaultAt::InsideBatch)),
            "{mode}: {err:?}"
        );
        assert_eq!(agent_ids(&store), vec![inside.agent_id], "{mode}");
        assert_eq!(channels.by_slug("inside").unwrap(), None, "{mode}");
        assert!(is_empty_cf(&store, cf::CHANNELS), "{mode}");
        assert_eq!(
            channels.channel_of(&inside.agent_id).unwrap(),
            None,
            "{mode}"
        );

        // AfterBatchWrite: the whole channel, project included.
        let after = agent("zns-after", 2);
        let err = channels
            .create_with_fault(
                new_channel_in("after", "proj-after"),
                &after,
                ts(3),
                FaultAt::AfterBatchWrite,
            )
            .unwrap_err();
        assert!(
            matches!(err, ChannelError::InjectedFault(FaultAt::AfterBatchWrite)),
            "{mode}: {err:?}"
        );
        let record = channels.by_slug("after").unwrap().unwrap();
        assert_eq!(record.project_id.as_deref(), Some("proj-after"), "{mode}");
        assert_eq!(record.coordinator, after.agent_id, "{mode}");
        assert_eq!(
            channels.channel_of(&after.agent_id).unwrap(),
            Some(record.id.clone()),
            "{mode}"
        );
        assert_eq!(channels.list().unwrap(), vec![record], "{mode}");

        // A blank project fails validation before any fault mode writes.
        let blank = agent("zns-blank", 3);
        let err = channels
            .create_with_fault(
                new_channel_in("blank", " "),
                &blank,
                ts(4),
                FaultAt::InsideBatch,
            )
            .unwrap_err();
        assert!(
            matches!(err, ChannelError::InvalidProjectId(_)),
            "{mode}: {err:?}"
        );
        assert!(!agent_ids(&store).contains(&blank.agent_id), "{mode}");
        assert_eq!(channels.by_slug("blank").unwrap(), None, "{mode}");
    }
}

#[test]
fn update_sets_changes_leaves_and_clears_the_project() {
    for (mode, store, _dir) in stores() {
        let channels = ChannelStore::for_store(&store);
        let ch = channels
            .create(new_channel("zode"), &agent("zns-coord", 0), ts(1))
            .unwrap();

        let set = channels
            .update(&ch.id, set_project(Some("proj-a")), ts(2))
            .unwrap();
        assert_eq!(set.project_id.as_deref(), Some("proj-a"), "{mode}");
        assert_eq!(set.updated_at, ts(2), "{mode}");
        assert_eq!(set.created_at, ts(1), "{mode}");
        assert_eq!(set.title, ch.title, "{mode}");
        assert_eq!(channels.get(&ch.id).unwrap(), Some(set.clone()), "{mode}");

        let changed = channels
            .update(&ch.id, set_project(Some("proj-b")), ts(3))
            .unwrap();
        assert_eq!(changed.project_id.as_deref(), Some("proj-b"), "{mode}");
        assert_eq!(changed.updated_at, ts(3), "{mode}");
        assert_eq!(
            channels.get(&ch.id).unwrap(),
            Some(changed.clone()),
            "{mode}"
        );

        // None leaves the project while the title changes.
        let retitled = channels
            .update(
                &ch.id,
                ChannelUpdate {
                    title: Some("Retitled".to_string()),
                    ..ChannelUpdate::default()
                },
                ts(4),
            )
            .unwrap();
        assert_eq!(retitled.title, "Retitled", "{mode}");
        assert_eq!(retitled.project_id.as_deref(), Some("proj-b"), "{mode}");
        assert_eq!(retitled.updated_at, ts(4), "{mode}");
        assert_eq!(
            channels.get(&ch.id).unwrap(),
            Some(retitled.clone()),
            "{mode}"
        );

        // Some(None) clears it.
        let cleared = channels.update(&ch.id, set_project(None), ts(5)).unwrap();
        assert_eq!(cleared.project_id, None, "{mode}");
        assert_eq!(cleared.title, "Retitled", "{mode}");
        assert_eq!(cleared.updated_at, ts(5), "{mode}");
        assert_eq!(
            channels.get(&ch.id).unwrap(),
            Some(cleared.clone()),
            "{mode}"
        );
        // Clearing again is fine and stays cleared.
        let again = channels.update(&ch.id, set_project(None), ts(6)).unwrap();
        assert_eq!(again.project_id, None, "{mode}");
        assert_eq!(again.updated_at, ts(6), "{mode}");
    }
}

#[test]
fn update_rejects_blank_project_ids_and_writes_nothing() {
    for (mode, store, _dir) in stores() {
        let channels = ChannelStore::for_store(&store);
        let ch = channels
            .create(
                new_channel_in("zode", "proj-a"),
                &agent("zns-coord", 0),
                ts(1),
            )
            .unwrap();
        let before = raw_values(&store, cf::CHANNELS);

        for bad in ["", "   "] {
            let err = channels
                .update(
                    &ch.id,
                    ChannelUpdate {
                        title: Some("Should not land".to_string()),
                        description: Some("Nor this".to_string()),
                        project_id: Some(Some(bad.to_string())),
                    },
                    ts(9),
                )
                .unwrap_err();
            assert!(
                matches!(&err, ChannelError::InvalidProjectId(p) if p == bad),
                "{mode} {bad:?}: {err:?}"
            );
            assert_eq!(
                channels.get(&ch.id).unwrap(),
                Some(ch.clone()),
                "{mode} {bad:?}"
            );
        }
        assert_eq!(raw_values(&store, cf::CHANNELS), before, "{mode}");
    }
}

#[test]
fn stage_update_lands_with_an_agent_row_only_when_written() {
    for (mode, store, _dir) in stores() {
        let channels = ChannelStore::for_store(&store);
        let agents = AgentStore::for_store(&store);
        let coord = agent("zns-coord", 0);
        let ch = channels.create(new_channel("zode"), &coord, ts(1)).unwrap();

        let joiner = agent("zns-joiner", 1);
        let mut batch = WriteBatch::default();
        agents
            .stage(
                &RegistryDiff {
                    agents_put: vec![joiner.clone()],
                    ..RegistryDiff::default()
                },
                &mut batch,
            )
            .unwrap();
        let staged = channels
            .stage_update(
                &ch.id,
                ChannelUpdate {
                    title: Some("Staged".to_string()),
                    project_id: Some(Some("proj-a".to_string())),
                    ..ChannelUpdate::default()
                },
                ts(5),
                &mut batch,
            )
            .unwrap();
        assert_eq!(staged.title, "Staged", "{mode}");
        assert_eq!(staged.project_id.as_deref(), Some("proj-a"), "{mode}");
        assert_eq!(staged.updated_at, ts(5), "{mode}");
        assert_eq!(staged.created_at, ts(1), "{mode}");

        // Staging writes nothing.
        assert_eq!(channels.get(&ch.id).unwrap(), Some(ch.clone()), "{mode}");
        assert_eq!(agent_ids(&store), vec![coord.agent_id], "{mode}");

        channels.write(batch).unwrap();
        assert_eq!(
            channels.get(&ch.id).unwrap(),
            Some(staged.clone()),
            "{mode}"
        );
        assert_eq!(
            agent_ids(&store),
            vec![coord.agent_id, joiner.agent_id],
            "{mode}"
        );

        // A staged update whose batch is dropped leaves the record alone.
        let mut dropped = WriteBatch::default();
        let unwritten = channels
            .stage_update(&ch.id, set_project(None), ts(6), &mut dropped)
            .unwrap();
        assert_eq!(unwritten.project_id, None, "{mode}");
        assert!(!dropped.is_empty(), "{mode}");
        drop(dropped);
        assert_eq!(
            channels.get(&ch.id).unwrap(),
            Some(staged.clone()),
            "{mode}"
        );
    }
}

#[test]
fn stage_update_error_leaves_the_batch_untouched() {
    for (mode, store, _dir) in stores() {
        let channels = ChannelStore::for_store(&store);
        let ch = channels
            .create(
                new_channel_in("zode", "proj-a"),
                &agent("zns-coord", 0),
                ts(1),
            )
            .unwrap();

        let mut empty = WriteBatch::default();
        let err = channels
            .stage_update(&ch.id, set_project(Some("")), ts(2), &mut empty)
            .unwrap_err();
        assert!(
            matches!(err, ChannelError::InvalidProjectId(_)),
            "{mode}: {err:?}"
        );
        assert_eq!(empty.len(), 0, "{mode}");

        // A batch already holding another store's rows keeps exactly them.
        let mut batch = WriteBatch::default();
        AgentStore::for_store(&store)
            .stage(
                &RegistryDiff {
                    agents_put: vec![agent("zns-joiner", 1)],
                    ..RegistryDiff::default()
                },
                &mut batch,
            )
            .unwrap();
        let staged_len = batch.len();
        let failures: [(&str, ChannelUpdate); 3] = [
            (ch.id.as_str(), set_project(Some("  "))),
            ("no-such-channel", set_project(Some("proj-b"))),
            ("bad/id", set_project(Some("proj-b"))),
        ];
        for (id, update) in failures {
            let err = channels
                .stage_update(id, update, ts(3), &mut batch)
                .unwrap_err();
            assert!(
                matches!(
                    err,
                    ChannelError::InvalidProjectId(_)
                        | ChannelError::UnknownChannel(_)
                        | ChannelError::InvalidId(_)
                ),
                "{mode} {id}: {err:?}"
            );
            assert_eq!(batch.len(), staged_len, "{mode} {id}");
        }
        assert_eq!(channels.get(&ch.id).unwrap(), Some(ch), "{mode}");
    }
}

#[test]
fn channel_row_without_project_id_decodes_with_none() {
    for (mode, store, _dir) in stores() {
        let channels = ChannelStore::for_store(&store);
        let ch = channels
            .create(
                new_channel_in("zode", "proj-a"),
                &agent("zns-coord", 0),
                ts(1),
            )
            .unwrap();

        // Rewrite the row as one from before the field existed.
        let mut legacy = serde_json::to_value(&ch).unwrap();
        let removed = legacy.as_object_mut().unwrap().remove("project_id");
        assert_eq!(removed, Some(serde_json::json!("proj-a")), "{mode}");
        let value = channels.encode(&legacy, "legacy channel").unwrap();
        raw_put(&store, cf::CHANNELS, &channel_key(&ch.id), &value);

        let expected = ChannelRecord {
            project_id: None,
            ..ch.clone()
        };
        assert_eq!(
            channels.get(&ch.id).unwrap(),
            Some(expected.clone()),
            "{mode}"
        );
        assert_eq!(channels.list().unwrap(), vec![expected], "{mode}");
    }
}

#[test]
fn channel_record_without_a_project_serializes_without_the_key() {
    let record = ChannelRecord {
        id: "id-1".to_string(),
        slug: "zode".to_string(),
        title: "Zode".to_string(),
        description: "about zode".to_string(),
        coordinator: derive_agent_id("zns-coord"),
        created_at: ts(1),
        updated_at: ts(2),
        archived: false,
        project_id: None,
    };
    let json = serde_json::to_value(&record).unwrap();
    assert!(!json.as_object().unwrap().contains_key("project_id"));
    assert_eq!(
        serde_json::from_value::<ChannelRecord>(json).unwrap(),
        record
    );

    let with = ChannelRecord {
        project_id: Some("proj-a".to_string()),
        ..record
    };
    let json = serde_json::to_value(&with).unwrap();
    assert_eq!(json["project_id"], serde_json::json!("proj-a"));
    assert_eq!(serde_json::from_value::<ChannelRecord>(json).unwrap(), with);
}

#[test]
fn channel_update_json_tells_absent_from_null() {
    let absent: ChannelUpdate = serde_json::from_str(r#"{"title":"T"}"#).unwrap();
    assert_eq!(absent.project_id, None);
    assert_eq!(absent.title.as_deref(), Some("T"));

    let empty: ChannelUpdate = serde_json::from_str("{}").unwrap();
    assert_eq!(empty, ChannelUpdate::default());

    let null: ChannelUpdate = serde_json::from_str(r#"{"project_id":null}"#).unwrap();
    assert_eq!(null.project_id, Some(None));

    let set: ChannelUpdate = serde_json::from_str(r#"{"project_id":"p"}"#).unwrap();
    assert_eq!(set.project_id, Some(Some("p".to_string())));

    assert!(serde_json::from_str::<ChannelUpdate>(r#"{"project_id":1}"#).is_err());

    // Each of the three states survives a round trip.
    for update in [
        ChannelUpdate::default(),
        set_project(None),
        set_project(Some("p")),
    ] {
        let json = serde_json::to_string(&update).unwrap();
        assert_eq!(
            serde_json::from_str::<ChannelUpdate>(&json).unwrap(),
            update,
            "{json}"
        );
    }
}

#[test]
fn migrate_links_folds_the_earliest_link_into_each_channel() {
    for (mode, store, _dir) in stores() {
        let channels = ChannelStore::for_store(&store);
        assert_eq!(
            channels.migrate_links(|_| true).unwrap(),
            0,
            "{mode}: empty store"
        );

        // Earliest linked_at wins; a blank id (only writable raw) is never
        // taken even when earliest.
        let earliest = channels
            .create(new_channel("earliest"), &agent("zns-1", 0), ts(1))
            .unwrap();
        channels
            .link_project(&earliest.id, "proj-c", ts(30))
            .unwrap();
        channels
            .link_project(&earliest.id, "proj-b", ts(10))
            .unwrap();
        channels
            .link_project(&earliest.id, "proj-a", ts(20))
            .unwrap();
        let blank = ProjectLink {
            project_id: " ".to_string(),
            linked_at: ts(0),
        };
        raw_put(
            &store,
            cf::CHANNELS,
            &link_key(&earliest.id, " "),
            &channels.encode(&blank, "blank link").unwrap(),
        );

        // Tied linked_at: lowest project id wins.
        let tie = channels
            .create(new_channel("tie"), &agent("zns-2", 1), ts(1))
            .unwrap();
        channels.link_project(&tie.id, "proj-z", ts(10)).unwrap();
        channels.link_project(&tie.id, "proj-y", ts(10)).unwrap();
        channels.link_project(&tie.id, "proj-x", ts(15)).unwrap();

        // Already has a project: keeps it, loses its rows.
        let has = channels
            .create(new_channel_in("has", "proj-own"), &agent("zns-3", 2), ts(1))
            .unwrap();
        channels.link_project(&has.id, "proj-early", ts(0)).unwrap();

        // No links: untouched and not counted.
        let none = channels
            .create(new_channel("none"), &agent("zns-4", 3), ts(1))
            .unwrap();

        let migrated = channels.migrate_links(|_| true).unwrap();
        assert_eq!(migrated, 3, "{mode}");

        // updated_at is left alone: the migration is not a user change.
        let expected_earliest = ChannelRecord {
            project_id: Some("proj-b".to_string()),
            ..earliest.clone()
        };
        let expected_tie = ChannelRecord {
            project_id: Some("proj-y".to_string()),
            ..tie.clone()
        };
        assert_eq!(
            channels.get(&earliest.id).unwrap(),
            Some(expected_earliest.clone()),
            "{mode}"
        );
        assert_eq!(
            channels.get(&tie.id).unwrap(),
            Some(expected_tie.clone()),
            "{mode}"
        );
        assert_eq!(channels.get(&has.id).unwrap(), Some(has.clone()), "{mode}");
        assert_eq!(
            channels.get(&none.id).unwrap(),
            Some(none.clone()),
            "{mode}"
        );
        for ch in [&earliest, &tie, &has, &none] {
            assert!(
                channels.projects(&ch.id).unwrap().is_empty(),
                "{mode} {}",
                ch.slug
            );
        }
        assert!(link_keys(&store).is_empty(), "{mode}");

        // Idempotent.
        assert_eq!(channels.migrate_links(|_| true).unwrap(), 0, "{mode}");
        assert_eq!(
            channels.list().unwrap(),
            vec![expected_earliest, has, none, expected_tie],
            "{mode}"
        );
    }
}

#[test]
fn migrate_links_with_only_blank_links_deletes_them_and_sets_nothing() {
    for (mode, store, _dir) in stores() {
        let channels = ChannelStore::for_store(&store);
        let ch = channels
            .create(new_channel("zode"), &agent("zns-coord", 0), ts(1))
            .unwrap();
        let blank = ProjectLink {
            project_id: "  ".to_string(),
            linked_at: ts(2),
        };
        raw_put(
            &store,
            cf::CHANNELS,
            &link_key(&ch.id, "  "),
            &channels.encode(&blank, "blank link").unwrap(),
        );

        assert_eq!(channels.migrate_links(|_| true).unwrap(), 1, "{mode}");
        assert_eq!(channels.get(&ch.id).unwrap(), Some(ch), "{mode}");
        assert!(link_keys(&store).is_empty(), "{mode}");
        assert_eq!(channels.migrate_links(|_| true).unwrap(), 0, "{mode}");
    }
}

#[test]
fn migrate_links_drops_links_of_a_missing_channel_and_skips_malformed_keys() {
    for (mode, store, _dir) in stores() {
        let channels = ChannelStore::for_store(&store);
        let kept = channels
            .create(new_channel("kept"), &agent("zns-kept", 0), ts(1))
            .unwrap();
        channels.link_project(&kept.id, "proj-kept", ts(2)).unwrap();
        let gone = channels
            .create(new_channel("gone"), &agent("zns-gone", 1), ts(1))
            .unwrap();
        channels.link_project(&gone.id, "proj-gone", ts(2)).unwrap();
        // There is no delete API: drop the channel's rows by hand.
        raw_delete(&store, cf::CHANNELS, &channel_key(&gone.id));
        raw_delete(&store, cf::CHANNELS, &slug_key(&gone.slug));
        assert_eq!(channels.get(&gone.id).unwrap(), None, "{mode}");

        // No '/' after the channel id, and an empty channel id.
        let malformed: [&[u8]; 2] = [b"p/no-slash", b"p//proj-x"];
        for key in malformed {
            raw_put(&store, cf::CHANNELS, key, b"not a link");
        }

        // Only the existing channel counts.
        assert_eq!(channels.migrate_links(|_| true).unwrap(), 1, "{mode}");
        assert_eq!(
            channels
                .get(&kept.id)
                .unwrap()
                .unwrap()
                .project_id
                .as_deref(),
            Some("proj-kept"),
            "{mode}"
        );
        assert!(channels.projects(&gone.id).unwrap().is_empty(), "{mode}");
        assert_eq!(channels.get(&gone.id).unwrap(), None, "{mode}");
        let mut left = link_keys(&store);
        left.sort();
        assert_eq!(
            left,
            vec![b"p//proj-x".to_vec(), b"p/no-slash".to_vec()],
            "{mode}: malformed rows are left in place"
        );

        assert_eq!(channels.migrate_links(|_| true).unwrap(), 0, "{mode}");
        assert_eq!(link_keys(&store).len(), 2, "{mode}");
    }
}

#[test]
fn migrate_links_skips_unknown_projects() {
    for (mode, store, _dir) in stores() {
        let channels = ChannelStore::for_store(&store);
        // The earliest link is to a project that no longer exists: the
        // earliest known one is taken instead.
        let some = channels
            .create(new_channel("some"), &agent("zns-some", 0), ts(1))
            .unwrap();
        channels.link_project(&some.id, "proj-gone", ts(1)).unwrap();
        channels
            .link_project(&some.id, "proj-late", ts(30))
            .unwrap();
        channels
            .link_project(&some.id, "proj-known", ts(20))
            .unwrap();
        // Only unknown projects: no project, and the rows still go.
        let none = channels
            .create(new_channel("none"), &agent("zns-none", 1), ts(1))
            .unwrap();
        channels.link_project(&none.id, "proj-gone", ts(1)).unwrap();

        let known = |id: &str| id != "proj-gone";
        assert_eq!(channels.migrate_links(known).unwrap(), 2, "{mode}");
        assert_eq!(
            channels
                .get(&some.id)
                .unwrap()
                .unwrap()
                .project_id
                .as_deref(),
            Some("proj-known"),
            "{mode}"
        );
        assert_eq!(channels.get(&none.id).unwrap(), Some(none), "{mode}");
        assert!(link_keys(&store).is_empty(), "{mode}");
        assert_eq!(channels.migrate_links(known).unwrap(), 0, "{mode}");
    }
}

#[test]
fn migrate_links_carries_on_past_a_channel_with_a_corrupt_link() {
    for (mode, store, _dir) in stores() {
        let channels = ChannelStore::for_store(&store);
        let bad = channels
            .create(new_channel("bad"), &agent("zns-bad", 0), ts(1))
            .unwrap();
        channels.link_project(&bad.id, "proj-ok", ts(2)).unwrap();
        raw_put(
            &store,
            cf::CHANNELS,
            &link_key(&bad.id, "proj-corrupt"),
            b"not a link",
        );
        let good = channels
            .create(new_channel("good"), &agent("zns-good", 1), ts(1))
            .unwrap();
        channels.link_project(&good.id, "proj-good", ts(2)).unwrap();

        assert_eq!(channels.migrate_links(|_| true).unwrap(), 1, "{mode}");
        assert_eq!(
            channels
                .get(&good.id)
                .unwrap()
                .unwrap()
                .project_id
                .as_deref(),
            Some("proj-good"),
            "{mode}"
        );
        // The bad channel is left as it was, rows included, for a rerun.
        assert_eq!(channels.get(&bad.id).unwrap(), Some(bad.clone()), "{mode}");
        let mut left = link_keys(&store);
        left.sort();
        let mut expected = vec![
            link_key(&bad.id, "proj-corrupt"),
            link_key(&bad.id, "proj-ok"),
        ];
        expected.sort();
        assert_eq!(left, expected, "{mode}");
        assert_eq!(channels.migrate_links(|_| true).unwrap(), 0, "{mode}");
    }
}
