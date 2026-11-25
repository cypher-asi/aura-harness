use super::*;
use crate::agents::derive_agent_id;
use crate::{FaultAt, RocksStore};
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
    }
}

fn new_channel(slug: &str) -> NewChannel {
    NewChannel {
        slug: slug.to_string(),
        title: format!("Channel {slug}"),
        description: format!("about {slug}"),
    }
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
