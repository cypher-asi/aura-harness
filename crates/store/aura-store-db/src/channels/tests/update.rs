//! Updating and archiving channels, directly and staged into a batch.

use super::support::*;
use crate::agents::{AgentRecord, AgentStore, RegistryDiff};
use crate::cf;
use crate::channels::*;
use crate::RocksStore;
use rocksdb::WriteBatch;

/// Sets a new title and description at `ts(5)`.
fn retitle(channels: &ChannelStore, id: &str) -> ChannelRecord {
    channels
        .update(
            id,
            ChannelUpdate {
                title: Some("New title".to_string()),
                description: Some("New description".to_string()),
                ..ChannelUpdate::default()
            },
            ts(5),
        )
        .unwrap()
}

fn create_in_project(channels: &ChannelStore) -> ChannelRecord {
    channels
        .create(
            new_channel_in("zode", "proj-a"),
            &agent("zns-coord", 0),
            ts(1),
        )
        .unwrap()
}

/// Stages a joining agent's row and a retitle with a project at `ts(5)`
/// into `batch`.
fn stage_joiner_and_update(
    store: &RocksStore,
    channel_id: &str,
    batch: &mut WriteBatch,
) -> (AgentRecord, ChannelRecord) {
    let joiner = agent("zns-joiner", 1);
    AgentStore::for_store(store)
        .stage(
            &RegistryDiff {
                agents_put: vec![joiner.clone()],
                ..RegistryDiff::default()
            },
            batch,
        )
        .unwrap();
    let staged = ChannelStore::for_store(store)
        .stage_update(
            channel_id,
            ChannelUpdate {
                title: Some("Staged".to_string()),
                project_id: Some(Some("proj-a".to_string())),
                ..ChannelUpdate::default()
            },
            ts(5),
            batch,
        )
        .unwrap();
    (joiner, staged)
}

#[test]
fn update_sets_title_and_description() {
    for (mode, store, _dir) in stores() {
        let channels = ChannelStore::for_store(&store);
        let ch = create_channel(&channels, "zode", "zns-coord", 0);
        let updated = retitle(&channels, &ch.id);
        assert_eq!(updated.title, "New title", "{mode}");
        assert_eq!(updated.description, "New description", "{mode}");
        assert_eq!(updated.updated_at, ts(5), "{mode}");
        assert_eq!(updated.created_at, ts(1), "{mode}");
        assert_eq!(
            channels.get(&ch.id).unwrap(),
            Some(updated.clone()),
            "{mode}"
        );
    }
}

#[test]
fn update_with_none_leaves_a_field_as_it_is() {
    for (mode, store, _dir) in stores() {
        let channels = ChannelStore::for_store(&store);
        let ch = create_channel(&channels, "zode", "zns-coord", 0);
        retitle(&channels, &ch.id);
        let partial = channels
            .update(
                &ch.id,
                ChannelUpdate {
                    description: Some("Only this".to_string()),
                    ..ChannelUpdate::default()
                },
                ts(6),
            )
            .unwrap();
        assert_eq!(partial.title, "New title", "{mode}");
        assert_eq!(partial.description, "Only this", "{mode}");
    }
}

#[test]
fn archive_marks_the_channel_and_keeps_it_listed() {
    for (mode, store, _dir) in stores() {
        let channels = ChannelStore::for_store(&store);
        let ch = create_channel(&channels, "zode", "zns-coord", 0);
        let archived = channels.archive(&ch.id, ts(9)).unwrap();
        assert!(archived.archived, "{mode}");
        assert_eq!(archived.updated_at, ts(9), "{mode}");
        assert_eq!(channels.list().unwrap(), vec![archived.clone()], "{mode}");
        assert_eq!(channels.by_slug("zode").unwrap(), Some(archived), "{mode}");
    }
}

#[test]
fn update_and_archive_refuse_an_unknown_channel() {
    for (mode, store, _dir) in stores() {
        let channels = ChannelStore::for_store(&store);
        create_channel(&channels, "zode", "zns-coord", 0);
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
fn update_sets_and_changes_the_project() {
    for (mode, store, _dir) in stores() {
        let channels = ChannelStore::for_store(&store);
        let ch = create_channel(&channels, "zode", "zns-coord", 0);

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
    }
}

#[test]
fn update_leaves_the_project_on_none_and_clears_it_on_some_none() {
    for (mode, store, _dir) in stores() {
        let channels = ChannelStore::for_store(&store);
        let ch = create_channel(&channels, "zode", "zns-coord", 0);
        channels
            .update(&ch.id, set_project(Some("proj-b")), ts(3))
            .unwrap();

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
        let ch = create_in_project(&channels);
        let before = raw_values(&store, cf::CHANNELS);

        for bad in ["", "   "] {
            let err = channels
                .update(
                    &ch.id,
                    ChannelUpdate {
                        title: Some("Should not land".to_string()),
                        description: Some("Nor this".to_string()),
                        project_id: Some(Some(bad.to_string())),
                        placement: Some(ChannelPlacement::Cloud),
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
        let coord = agent("zns-coord", 0);
        let ch = channels.create(new_channel("zode"), &coord, ts(1)).unwrap();

        let mut batch = WriteBatch::default();
        let (joiner, staged) = stage_joiner_and_update(&store, &ch.id, &mut batch);
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
    }
}

#[test]
fn stage_update_whose_batch_is_dropped_leaves_the_record() {
    for (mode, store, _dir) in stores() {
        let channels = ChannelStore::for_store(&store);
        let ch = create_channel(&channels, "zode", "zns-coord", 0);
        let mut batch = WriteBatch::default();
        let (_, staged) = stage_joiner_and_update(&store, &ch.id, &mut batch);
        channels.write(batch).unwrap();

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
fn stage_update_error_leaves_an_empty_batch_empty() {
    for (mode, store, _dir) in stores() {
        let channels = ChannelStore::for_store(&store);
        let ch = create_in_project(&channels);
        let mut empty = WriteBatch::default();
        let err = channels
            .stage_update(&ch.id, set_project(Some("")), ts(2), &mut empty)
            .unwrap_err();
        assert!(
            matches!(err, ChannelError::InvalidProjectId(_)),
            "{mode}: {err:?}"
        );
        assert_eq!(empty.len(), 0, "{mode}");
    }
}

#[test]
fn stage_update_error_keeps_exactly_another_stores_rows() {
    for (mode, store, _dir) in stores() {
        let channels = ChannelStore::for_store(&store);
        let ch = create_in_project(&channels);
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
