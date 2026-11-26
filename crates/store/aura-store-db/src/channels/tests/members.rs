//! Channel members and the agent-to-channel reverse index.

use super::support::*;
use crate::agents::{derive_agent_id, AgentRecord};
use crate::cf;
use crate::channels::keys::{member_key, reverse_key};
use crate::channels::*;
use rocksdb::WriteBatch;

/// Two channels, `one` with a full agent and its subagent added.
struct Roster {
    ch: ChannelRecord,
    other: ChannelRecord,
    coord: AgentRecord,
    other_coord: AgentRecord,
    full: MemberRecord,
    sub: MemberRecord,
}

impl Roster {
    fn new(channels: &ChannelStore) -> Self {
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
        Self {
            ch,
            other,
            coord,
            other_coord,
            full,
            sub,
        }
    }

    fn coord_member(&self) -> MemberRecord {
        member(self.coord.agent_id, MemberRole::Coordinator, None, 100)
    }
}

#[test]
fn members_are_listed_by_join_time_and_reverse_indexed() {
    for (mode, store, _dir) in stores() {
        let channels = ChannelStore::for_store(&store);
        let r = Roster::new(&channels);
        assert_eq!(
            channels.members(&r.ch.id).unwrap(),
            vec![r.coord_member(), r.sub.clone(), r.full.clone()],
            "{mode}"
        );
        for id in [r.coord.agent_id, r.full.agent_id, r.sub.agent_id] {
            assert_eq!(
                channels.channel_of(&id).unwrap(),
                Some(r.ch.id.clone()),
                "{mode}"
            );
        }
    }
}

#[test]
fn add_member_again_in_the_same_channel_keeps_the_existing_record() {
    for (mode, store, _dir) in stores() {
        let channels = ChannelStore::for_store(&store);
        let r = Roster::new(&channels);
        let mut again = r.full.clone();
        again.joined_at = ts(999);
        again.role = MemberRole::Subagent;
        channels.add_member(&r.ch.id, &again).unwrap();
        assert_eq!(
            channels.members(&r.ch.id).unwrap(),
            vec![r.coord_member(), r.sub.clone(), r.full.clone()],
            "{mode}"
        );
    }
}

#[test]
fn add_member_to_another_channel_is_refused_and_writes_nothing_there() {
    for (mode, store, _dir) in stores() {
        let channels = ChannelStore::for_store(&store);
        let r = Roster::new(&channels);
        let err = channels.add_member(&r.other.id, &r.full).unwrap_err();
        assert!(
            matches!(&err, ChannelError::AlreadyMember { agent, channel }
                if *agent == r.full.agent_id.to_hex() && *channel == r.ch.id),
            "{mode}: {err:?}"
        );
        assert_eq!(
            channels.members(&r.other.id).unwrap(),
            vec![member(
                r.other_coord.agent_id,
                MemberRole::Coordinator,
                None,
                100
            )],
            "{mode}"
        );
        assert_eq!(
            channels.channel_of(&r.full.agent_id).unwrap(),
            Some(r.ch.id.clone()),
            "{mode}"
        );
    }
}

#[test]
fn add_member_to_an_unknown_channel_is_refused() {
    for (mode, store, _dir) in stores() {
        let channels = ChannelStore::for_store(&store);
        Roster::new(&channels);
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
    }
}

#[test]
fn remove_member_from_the_wrong_channel_leaves_the_reverse_index() {
    for (mode, store, _dir) in stores() {
        let channels = ChannelStore::for_store(&store);
        let r = Roster::new(&channels);
        channels
            .remove_member(&r.other.id, &r.full.agent_id)
            .unwrap();
        assert_eq!(
            channels.channel_of(&r.full.agent_id).unwrap(),
            Some(r.ch.id.clone()),
            "{mode}"
        );
    }
}

#[test]
fn remove_member_deletes_both_rows_and_frees_the_agent() {
    for (mode, store, _dir) in stores() {
        let channels = ChannelStore::for_store(&store);
        let r = Roster::new(&channels);
        channels.remove_member(&r.ch.id, &r.full.agent_id).unwrap();
        assert_eq!(
            channels.channel_of(&r.full.agent_id).unwrap(),
            None,
            "{mode}"
        );
        assert_eq!(
            channels.members(&r.ch.id).unwrap(),
            vec![r.coord_member(), r.sub.clone()],
            "{mode}"
        );
        let member_row = member_key(&r.ch.id, &r.full.agent_id);
        let reverse_row = reverse_key(&r.full.agent_id);
        assert!(
            raw_values(&store, cf::CHANNEL_MEMBERS)
                .iter()
                .all(|(k, _)| *k != member_row && *k != reverse_row),
            "{mode}"
        );
        // Removing a non-member is a no-op.
        channels.remove_member(&r.ch.id, &r.full.agent_id).unwrap();

        // After removal it may join another channel.
        channels.add_member(&r.other.id, &r.full).unwrap();
        assert_eq!(
            channels.channel_of(&r.full.agent_id).unwrap(),
            Some(r.other.id.clone()),
            "{mode}"
        );
    }
}

#[test]
fn stage_member_lands_only_when_written() {
    for (mode, store, _dir) in stores() {
        let channels = ChannelStore::for_store(&store);
        let r = Roster::new(&channels);
        channels.remove_member(&r.ch.id, &r.full.agent_id).unwrap();

        let staged = member(derive_agent_id("zns-staged"), MemberRole::Agent, None, 400);
        let mut batch = WriteBatch::default();
        channels
            .stage_member(&r.ch.id, &staged, &mut batch)
            .unwrap();
        assert_eq!(
            channels.channel_of(&staged.agent_id).unwrap(),
            None,
            "{mode}"
        );
        channels.write(batch).unwrap();
        assert_eq!(
            channels.channel_of(&staged.agent_id).unwrap(),
            Some(r.ch.id.clone()),
            "{mode}"
        );
        assert_eq!(
            channels.members(&r.ch.id).unwrap(),
            vec![r.coord_member(), r.sub.clone(), staged],
            "{mode}"
        );
    }
}

#[test]
fn stage_member_with_an_invalid_channel_id_stages_nothing() {
    for (mode, store, _dir) in stores() {
        let channels = ChannelStore::for_store(&store);
        Roster::new(&channels);
        let stray = member(derive_agent_id("zns-stray"), MemberRole::Agent, None, 1);
        let mut batch = WriteBatch::default();
        let err = channels
            .stage_member("bad/id", &stray, &mut batch)
            .unwrap_err();
        assert!(matches!(err, ChannelError::InvalidId(_)), "{mode}: {err:?}");
        assert_eq!(batch.len(), 0, "{mode}");
    }
}
