//! Channel posts and the last-read marker.

use super::support::*;
use crate::channels::*;

/// Two channels and four posts appended out of time order.
struct Posts {
    a: ChannelRecord,
    b: ChannelRecord,
    p300: ChannelPost,
    p100: ChannelPost,
    pb: ChannelPost,
    p200: ChannelPost,
}

impl Posts {
    fn append(channels: &ChannelStore) -> Self {
        let a = create_channel(channels, "a", "zns-a", 0);
        let b = create_channel(channels, "b", "zns-b", 1);
        let p300 = channels.append_post(&a.id, post("three", 300)).unwrap();
        let p100 = channels.append_post(&a.id, post("one", 100)).unwrap();
        let pb = channels.append_post(&b.id, post("other", 150)).unwrap();
        let p200 = channels.append_post(&a.id, post("two", 200)).unwrap();
        Self {
            a,
            b,
            p300,
            p100,
            pb,
            p200,
        }
    }

    /// Sequence numbers in append order.
    fn seqs(&self) -> [u64; 4] {
        [self.p300.seq, self.p100.seq, self.pb.seq, self.p200.seq]
    }
}

#[test]
fn post_seqs_grow_in_append_order() {
    for (mode, store, _dir) in stores() {
        let channels = ChannelStore::for_store(&store);
        let p = Posts::append(&channels);
        let seqs = p.seqs();
        assert!(seqs.windows(2).all(|w| w[0] < w[1]), "{mode}: {seqs:?}");
        assert_eq!(p.p300.channel_id, p.a.id, "{mode}");
        assert_eq!(p.p300.text, "three", "{mode}");
        assert_eq!(p.p300.author, PostAuthor::User, "{mode}");
    }
}

#[test]
fn posts_are_time_ordered_per_channel_and_filtered_by_since() {
    for (mode, store, _dir) in stores() {
        let channels = ChannelStore::for_store(&store);
        let Posts {
            a,
            b,
            p300,
            p100,
            pb,
            p200,
        } = Posts::append(&channels);
        assert_eq!(
            channels.posts(&a.id, None).unwrap(),
            vec![p100, p200.clone(), p300.clone()],
            "{mode}"
        );
        assert_eq!(
            channels.posts(&a.id, Some(150)).unwrap(),
            vec![p200, p300.clone()],
            "{mode}"
        );
        assert_eq!(
            channels.posts(&a.id, Some(200)).unwrap(),
            vec![p300],
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
        assert_eq!(channels.posts(&b.id, None).unwrap(), vec![pb], "{mode}");
    }
}

#[test]
fn append_post_to_an_unknown_channel_is_refused() {
    for (mode, store, _dir) in stores() {
        let channels = ChannelStore::for_store(&store);
        let err = channels
            .append_post("no-such-channel", post("x", 1))
            .unwrap_err();
        assert!(
            matches!(err, ChannelError::UnknownChannel(_)),
            "{mode}: {err:?}"
        );
    }
}

#[test]
fn post_seqs_keep_growing_across_reopen() {
    for (mode, store, dir) in stores() {
        let sealed = store.cipher().is_some();
        let channels = ChannelStore::for_store(&store);
        let p = Posts::append(&channels);
        let seqs = p.seqs();

        // Reopen: a fresh store seeds the counter from disk.
        drop(channels);
        drop(store);
        let store = open(sealed, dir.path());
        let channels = ChannelStore::for_store(&store);
        let same_ms = channels.append_post(&p.a.id, post("again", 200)).unwrap();
        assert!(same_ms.seq > p.p200.seq, "{mode}");
        assert!(seqs.iter().all(|&s| s < same_ms.seq), "{mode}");
        let p250 = channels.append_post(&p.a.id, post("late", 250)).unwrap();
        assert!(p250.seq > same_ms.seq, "{mode}");
        assert_eq!(
            channels.posts(&p.a.id, None).unwrap(),
            vec![p.p100, p.p200, same_ms, p250, p.p300],
            "{mode}"
        );
        assert_eq!(channels.posts(&p.b.id, None).unwrap(), vec![p.pb], "{mode}");
    }
}

#[test]
fn last_read_is_kept_per_channel_and_left_out_of_the_list() {
    for (mode, store, _dir) in stores() {
        let channels = ChannelStore::for_store(&store);
        let ch = create_channel(&channels, "zode", "zns-coord", 0);
        let other = create_channel(&channels, "other", "zns-other", 1);
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
fn a_plan_post_keeps_its_planner_and_an_old_row_reads_without_one() {
    for (mode, store, _dir) in stores() {
        let channels = ChannelStore::for_store(&store);
        let p = Posts::append(&channels);
        let planner = aura_core_types::AgentId::generate();
        let new = NewPost {
            mentions: vec![planner],
            planner: Some(planner),
            ..post("/plan the footer", 400)
        };
        let stored = channels.append_post(&p.a.id, new).unwrap();
        assert_eq!(stored.planner, Some(planner), "{mode}");
        let read = channels.posts(&p.a.id, Some(399)).unwrap();
        assert_eq!(read, vec![stored.clone()], "{mode}");

        let mut old = serde_json::to_value(&stored).unwrap();
        assert!(old.as_object_mut().unwrap().remove("planner").is_some());
        let old: ChannelPost = serde_json::from_value(old).unwrap();
        assert_eq!(old.planner, None, "{mode}");
        let plain = serde_json::to_value(&p.p100).unwrap();
        assert!(plain.get("planner").is_none(), "not written when absent");
    }
}
