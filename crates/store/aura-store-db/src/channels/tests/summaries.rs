//! The kept summaries of the user's long messages.

use super::support::*;
use crate::channels::*;

fn pair(key: &str, summary: &str) -> (String, String) {
    (key.to_string(), summary.to_string())
}

#[test]
fn a_summary_round_trips_in_both_modes() {
    for (mode, store, _dir) in stores() {
        let channels = ChannelStore::for_store(&store);
        assert_eq!(channels.post_summary("ab12").unwrap(), None, "{mode}");
        channels
            .set_post_summaries(&[pair("ab12", "Ship the parser"), pair("cd", "Other")])
            .unwrap();
        let kept = channels.post_summary("ab12").unwrap();
        assert_eq!(kept.as_deref(), Some("Ship the parser"), "{mode}");
        assert_eq!(
            channels.post_summary("cd").unwrap().as_deref(),
            Some("Other")
        );
        assert_eq!(channels.post_summary("ab13").unwrap(), None, "{mode}");
    }
}

#[test]
fn a_sealed_summary_is_not_stored_in_the_clear() {
    let dir = tempfile::TempDir::new().unwrap();
    let store = open(true, dir.path());
    let channels = ChannelStore::for_store(&store);
    channels
        .set_post_summaries(&[pair("ff", "secret plan words")])
        .unwrap();
    let rows = raw_values(&store, crate::cf::CHANNELS);
    assert!(!rows.is_empty());
    assert!(rows
        .iter()
        .all(|(_, value)| !String::from_utf8_lossy(value).contains("secret plan")));
}

#[test]
fn a_summary_survives_reopening_the_store() {
    let dir = tempfile::TempDir::new().unwrap();
    {
        let store = open(true, dir.path());
        let channels = ChannelStore::for_store(&store);
        channels.set_post_summaries(&[pair("0a", "Kept")]).unwrap();
    }
    let store = open(true, dir.path());
    let channels = ChannelStore::for_store(&store);
    assert_eq!(
        channels.post_summary("0a").unwrap().as_deref(),
        Some("Kept")
    );
}

#[test]
fn keys_that_are_not_lowercase_hex_are_refused_and_nothing_lands() {
    let (_, store, _dir) = stores().remove(0);
    let channels = ChannelStore::for_store(&store);
    for key in ["", "AB", "../x", "a/b", &"a".repeat(65)] {
        let batch = [pair("aa", "good"), pair(key, "bad")];
        assert!(matches!(
            channels.set_post_summaries(&batch),
            Err(ChannelError::InvalidSummaryKey(_))
        ));
        assert!(matches!(
            channels.post_summary(key),
            Err(ChannelError::InvalidSummaryKey(_))
        ));
    }
    assert_eq!(channels.post_summary("aa").unwrap(), None, "all or nothing");
}
