//! The cached summaries of the user's long messages.

use super::support::*;
use crate::channels::*;

#[test]
fn a_summary_round_trips_in_both_modes() {
    for (mode, store, _dir) in stores() {
        let channels = ChannelStore::for_store(&store);
        let channel = create_channel(&channels, "a", "zns-a", 0);
        assert_eq!(
            channels.summary(&channel.id, "ab12").unwrap(),
            None,
            "{mode}"
        );
        channels
            .set_summary(&channel.id, "ab12", "Ship the parser")
            .unwrap();
        assert_eq!(
            channels.summary(&channel.id, "ab12").unwrap().as_deref(),
            Some("Ship the parser"),
            "{mode}"
        );
        assert_eq!(
            channels.summary(&channel.id, "ab13").unwrap(),
            None,
            "{mode}"
        );
    }
}

#[test]
fn a_sealed_summary_is_not_stored_in_the_clear() {
    let dir = tempfile::TempDir::new().unwrap();
    let store = open(true, dir.path());
    let channels = ChannelStore::for_store(&store);
    let channel = create_channel(&channels, "a", "zns-a", 0);
    channels
        .set_summary(&channel.id, "ff", "secret plan words")
        .unwrap();
    let rows = raw_values(&store, crate::cf::CHANNELS);
    assert!(rows
        .iter()
        .all(|(_, value)| !String::from_utf8_lossy(value).contains("secret plan")));
}

#[test]
fn a_summary_survives_reopening_the_store() {
    let dir = tempfile::TempDir::new().unwrap();
    let id = {
        let store = open(false, dir.path());
        let channels = ChannelStore::for_store(&store);
        let channel = create_channel(&channels, "a", "zns-a", 0);
        channels.set_summary(&channel.id, "0a", "Kept").unwrap();
        channel.id
    };
    let store = open(false, dir.path());
    let channels = ChannelStore::for_store(&store);
    assert_eq!(
        channels.summary(&id, "0a").unwrap().as_deref(),
        Some("Kept")
    );
}

#[test]
fn keys_that_are_not_lowercase_hex_are_refused() {
    let (_, store, _dir) = stores().remove(0);
    let channels = ChannelStore::for_store(&store);
    let channel = create_channel(&channels, "a", "zns-a", 0);
    for key in ["", "AB", "../x", "a/b", &"a".repeat(65)] {
        assert!(matches!(
            channels.set_summary(&channel.id, key, "x"),
            Err(ChannelError::InvalidSummaryKey(_))
        ));
        assert!(matches!(
            channels.summary(&channel.id, key),
            Err(ChannelError::InvalidSummaryKey(_))
        ));
    }
    assert!(matches!(
        channels.summary("bad/id", "ab"),
        Err(ChannelError::InvalidId(_))
    ));
}
