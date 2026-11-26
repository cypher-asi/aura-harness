//! Fault injection on create: all or nothing, except the documented
//! partial write of the coordinator's agent row.

use super::support::*;
use crate::agents::AgentRecord;
use crate::cf;
use crate::channels::*;
use crate::FaultAt;

fn assert_injected(err: &ChannelError, fault: FaultAt, mode: &str) {
    assert!(
        matches!(err, ChannelError::InjectedFault(at) if *at == fault),
        "{mode}: {err:?}"
    );
}

fn create_faulted(
    channels: &ChannelStore,
    channel: NewChannel,
    coordinator: &AgentRecord,
    secs: i64,
    fault: FaultAt,
) -> ChannelError {
    channels
        .create_with_fault(channel, coordinator, ts(secs), fault)
        .unwrap_err()
}

#[test]
fn create_with_fault_before_batch_write_writes_nothing() {
    for (mode, store, _dir) in stores() {
        let channels = ChannelStore::for_store(&store);
        let before = agent("zns-before", 0);
        let fault = FaultAt::BeforeBatchWrite;
        let err = create_faulted(&channels, new_channel("before"), &before, 1, fault);
        assert_injected(&err, fault, mode);
        assert!(agent_ids(&store).is_empty(), "{mode}");
        assert_eq!(channels.by_slug("before").unwrap(), None, "{mode}");
        assert!(channels.list().unwrap().is_empty(), "{mode}");
        assert_eq!(
            channels.channel_of(&before.agent_id).unwrap(),
            None,
            "{mode}"
        );
        assert_no_channel_rows(&store, mode);
    }
}

#[test]
fn create_with_fault_after_batch_write_writes_everything() {
    for (mode, store, _dir) in stores() {
        let channels = ChannelStore::for_store(&store);
        let after = agent("zns-after", 1);
        let fault = FaultAt::AfterBatchWrite;
        let err = create_faulted(&channels, new_channel("after"), &after, 2, fault);
        assert_injected(&err, fault, mode);
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
    }
}

#[test]
fn create_with_fault_inside_batch_writes_only_the_agent_row() {
    for (mode, store, _dir) in stores() {
        let channels = ChannelStore::for_store(&store);
        let after = agent("zns-after", 1);
        let record = channels
            .create(new_channel("after"), &after, ts(2))
            .unwrap();

        let inside = agent("zns-inside", 2);
        let fault = FaultAt::InsideBatch;
        let err = create_faulted(&channels, new_channel("inside"), &inside, 3, fault);
        assert_injected(&err, fault, mode);
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
fn create_with_fault_before_batch_write_with_a_project_writes_nothing() {
    for (mode, store, _dir) in stores() {
        let channels = ChannelStore::for_store(&store);
        let before = agent("zns-before", 0);
        let channel = new_channel_in("before", "proj-before");
        let fault = FaultAt::BeforeBatchWrite;
        let err = create_faulted(&channels, channel, &before, 1, fault);
        assert_injected(&err, fault, mode);
        assert!(agent_ids(&store).is_empty(), "{mode}");
        assert_no_channel_rows(&store, mode);
    }
}

#[test]
fn create_with_fault_inside_batch_with_a_project_writes_no_channel() {
    for (mode, store, _dir) in stores() {
        let channels = ChannelStore::for_store(&store);
        // Only the coordinator's agent row, no channel, so no project
        // either.
        let inside = agent("zns-inside", 1);
        let channel = new_channel_in("inside", "proj-inside");
        let fault = FaultAt::InsideBatch;
        let err = create_faulted(&channels, channel, &inside, 2, fault);
        assert_injected(&err, fault, mode);
        assert_eq!(agent_ids(&store), vec![inside.agent_id], "{mode}");
        assert_eq!(channels.by_slug("inside").unwrap(), None, "{mode}");
        assert!(is_empty_cf(&store, cf::CHANNELS), "{mode}");
        assert_eq!(
            channels.channel_of(&inside.agent_id).unwrap(),
            None,
            "{mode}"
        );
    }
}

#[test]
fn create_with_fault_after_batch_write_with_a_project_writes_the_project() {
    for (mode, store, _dir) in stores() {
        let channels = ChannelStore::for_store(&store);
        let after = agent("zns-after", 2);
        let channel = new_channel_in("after", "proj-after");
        let fault = FaultAt::AfterBatchWrite;
        let err = create_faulted(&channels, channel, &after, 3, fault);
        assert_injected(&err, fault, mode);
        let record = channels.by_slug("after").unwrap().unwrap();
        assert_eq!(record.project_id.as_deref(), Some("proj-after"), "{mode}");
        assert_eq!(record.coordinator, after.agent_id, "{mode}");
        assert_eq!(
            channels.channel_of(&after.agent_id).unwrap(),
            Some(record.id.clone()),
            "{mode}"
        );
        assert_eq!(channels.list().unwrap(), vec![record], "{mode}");
    }
}

#[test]
fn create_with_fault_and_a_blank_project_fails_validation_first() {
    for (mode, store, _dir) in stores() {
        let channels = ChannelStore::for_store(&store);
        let blank = agent("zns-blank", 3);
        let channel = new_channel_in("blank", " ");
        let err = create_faulted(&channels, channel, &blank, 4, FaultAt::InsideBatch);
        assert!(
            matches!(err, ChannelError::InvalidProjectId(_)),
            "{mode}: {err:?}"
        );
        assert!(!agent_ids(&store).contains(&blank.agent_id), "{mode}");
        assert_eq!(channels.by_slug("blank").unwrap(), None, "{mode}");
    }
}
