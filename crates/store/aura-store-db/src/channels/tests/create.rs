//! Creating channels: the coordinator, slugs, projects and list order.

use super::support::*;
use crate::agents::AgentStore;
use crate::channels::*;

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
        assert_no_channel_rows(&store, mode);
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
        assert_no_channel_rows(&store, mode);

        // The slug is still free.
        let record = channels
            .create(new_channel_in("zode", "proj-a"), &agent("zns-ok", 9), ts(2))
            .unwrap();
        assert_eq!(channels.by_slug("zode").unwrap(), Some(record), "{mode}");
    }
}
