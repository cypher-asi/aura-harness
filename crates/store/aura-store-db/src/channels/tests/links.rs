//! Legacy project links and their migration onto the channel record.

use super::support::*;
use crate::cf;
use crate::channels::keys::{channel_key, link_key, slug_key};
use crate::channels::*;
use crate::RocksStore;

fn with_project(record: &ChannelRecord, project_id: &str) -> ChannelRecord {
    ChannelRecord {
        project_id: Some(project_id.to_string()),
        ..record.clone()
    }
}

fn put_link(store: &RocksStore, channels: &ChannelStore, channel_id: &str, link: &ProjectLink) {
    raw_put(
        store,
        cf::CHANNELS,
        &link_key(channel_id, &link.project_id),
        &channels.encode(link, "blank link").unwrap(),
    );
}

#[test]
fn project_links() {
    for (mode, store, _dir) in stores() {
        let channels = ChannelStore::for_store(&store);
        let ch = create_channel(&channels, "zode", "zns-coord", 0);
        let other = create_channel(&channels, "other", "zns-other", 1);

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

/// Channels with legacy links covering each migration rule.
struct LegacyLinks {
    /// Earliest `linked_at` wins (`proj-b`); a blank id (only writable
    /// raw) is never taken even when earliest.
    earliest: ChannelRecord,
    /// Tied `linked_at`: lowest project id wins (`proj-y`).
    tie: ChannelRecord,
    /// Already has a project: keeps it, loses its rows.
    has: ChannelRecord,
    /// No links: untouched and not counted.
    none: ChannelRecord,
}

impl LegacyLinks {
    fn seed(store: &RocksStore, channels: &ChannelStore) -> Self {
        let earliest = create_channel(channels, "earliest", "zns-1", 0);
        for (project, at) in [("proj-c", 30), ("proj-b", 10), ("proj-a", 20)] {
            channels
                .link_project(&earliest.id, project, ts(at))
                .unwrap();
        }
        let blank = ProjectLink {
            project_id: " ".to_string(),
            linked_at: ts(0),
        };
        put_link(store, channels, &earliest.id, &blank);

        let tie = create_channel(channels, "tie", "zns-2", 1);
        channels.link_project(&tie.id, "proj-z", ts(10)).unwrap();
        channels.link_project(&tie.id, "proj-y", ts(10)).unwrap();
        channels.link_project(&tie.id, "proj-x", ts(15)).unwrap();

        let has = channels
            .create(new_channel_in("has", "proj-own"), &agent("zns-3", 2), ts(1))
            .unwrap();
        channels.link_project(&has.id, "proj-early", ts(0)).unwrap();

        let none = create_channel(channels, "none", "zns-4", 3);
        Self {
            earliest,
            tie,
            has,
            none,
        }
    }
}

#[test]
fn migrate_links_on_an_empty_store_migrates_nothing() {
    for (mode, store, _dir) in stores() {
        let channels = ChannelStore::for_store(&store);
        assert_eq!(
            channels.migrate_links(|_| true).unwrap(),
            0,
            "{mode}: empty store"
        );
    }
}

#[test]
fn migrate_links_folds_the_earliest_link_into_each_channel() {
    for (mode, store, _dir) in stores() {
        let channels = ChannelStore::for_store(&store);
        let l = LegacyLinks::seed(&store, &channels);

        let migrated = channels.migrate_links(|_| true).unwrap();
        assert_eq!(migrated, 3, "{mode}");

        // updated_at is left alone: the migration is not a user change.
        assert_eq!(
            channels.get(&l.earliest.id).unwrap(),
            Some(with_project(&l.earliest, "proj-b")),
            "{mode}"
        );
        assert_eq!(
            channels.get(&l.tie.id).unwrap(),
            Some(with_project(&l.tie, "proj-y")),
            "{mode}"
        );
        assert_eq!(
            channels.get(&l.has.id).unwrap(),
            Some(l.has.clone()),
            "{mode}"
        );
        assert_eq!(
            channels.get(&l.none.id).unwrap(),
            Some(l.none.clone()),
            "{mode}"
        );
        for ch in [&l.earliest, &l.tie, &l.has, &l.none] {
            assert!(
                channels.projects(&ch.id).unwrap().is_empty(),
                "{mode} {}",
                ch.slug
            );
        }
        assert!(link_keys(&store).is_empty(), "{mode}");
    }
}

#[test]
fn migrate_links_is_idempotent() {
    for (mode, store, _dir) in stores() {
        let channels = ChannelStore::for_store(&store);
        let l = LegacyLinks::seed(&store, &channels);
        channels.migrate_links(|_| true).unwrap();

        assert_eq!(channels.migrate_links(|_| true).unwrap(), 0, "{mode}");
        assert_eq!(
            channels.list().unwrap(),
            vec![
                with_project(&l.earliest, "proj-b"),
                l.has,
                l.none,
                with_project(&l.tie, "proj-y"),
            ],
            "{mode}"
        );
    }
}

#[test]
fn migrate_links_with_only_blank_links_deletes_them_and_sets_nothing() {
    for (mode, store, _dir) in stores() {
        let channels = ChannelStore::for_store(&store);
        let ch = create_channel(&channels, "zode", "zns-coord", 0);
        let blank = ProjectLink {
            project_id: "  ".to_string(),
            linked_at: ts(2),
        };
        put_link(&store, &channels, &ch.id, &blank);

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
        let kept = create_channel(&channels, "kept", "zns-kept", 0);
        channels.link_project(&kept.id, "proj-kept", ts(2)).unwrap();
        let gone = create_channel(&channels, "gone", "zns-gone", 1);
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
            project_of(&channels, &kept.id).as_deref(),
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

/// The stored project of an existing channel.
fn project_of(channels: &ChannelStore, id: &str) -> Option<String> {
    channels.get(id).unwrap().unwrap().project_id
}

#[test]
fn migrate_links_skips_unknown_projects() {
    for (mode, store, _dir) in stores() {
        let channels = ChannelStore::for_store(&store);
        // The earliest link is to a project that no longer exists: the
        // earliest known one is taken instead.
        let some = create_channel(&channels, "some", "zns-some", 0);
        channels.link_project(&some.id, "proj-gone", ts(1)).unwrap();
        channels
            .link_project(&some.id, "proj-late", ts(30))
            .unwrap();
        channels
            .link_project(&some.id, "proj-known", ts(20))
            .unwrap();
        // Only unknown projects: no project, and the rows still go.
        let none = create_channel(&channels, "none", "zns-none", 1);
        channels.link_project(&none.id, "proj-gone", ts(1)).unwrap();

        let known = |id: &str| id != "proj-gone";
        assert_eq!(channels.migrate_links(known).unwrap(), 2, "{mode}");
        assert_eq!(
            project_of(&channels, &some.id).as_deref(),
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
        let bad = create_channel(&channels, "bad", "zns-bad", 0);
        channels.link_project(&bad.id, "proj-ok", ts(2)).unwrap();
        raw_put(
            &store,
            cf::CHANNELS,
            &link_key(&bad.id, "proj-corrupt"),
            b"not a link",
        );
        let good = create_channel(&channels, "good", "zns-good", 1);
        channels.link_project(&good.id, "proj-good", ts(2)).unwrap();

        assert_eq!(channels.migrate_links(|_| true).unwrap(), 1, "{mode}");
        assert_eq!(
            project_of(&channels, &good.id).as_deref(),
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
