//! On-disk encoding: column families, sealing and the JSON shape of
//! records and updates.

use super::support::*;
use crate::agents::derive_agent_id;
use crate::cf;
use crate::channels::keys::channel_key;
use crate::channels::*;

#[test]
fn channel_column_families_are_registered_and_sealed() {
    for family in [cf::CHANNELS, cf::CHANNEL_MEMBERS, cf::CHANNEL_POSTS] {
        assert!(crate::SEALED_VALUE_CFS.contains(&family), "{family}");
        assert!(crate::migrate::ALL_CFS.contains(&family), "{family}");
    }

    for (mode, store, _dir) in stores() {
        let sealed = store.cipher().is_some();
        let channels = ChannelStore::for_store(&store);
        let ch = create_channel(&channels, "zode", "zns-coord", 0);
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
        placement: ChannelPlacement::Local,
    };
    let json = serde_json::to_value(&record).unwrap();
    assert!(!json.as_object().unwrap().contains_key("project_id"));
    assert!(
        !json.as_object().unwrap().contains_key("placement"),
        "a local channel's row leaves its placement out"
    );
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

    let cloud = ChannelRecord {
        placement: ChannelPlacement::Cloud,
        ..with
    };
    let mut json = serde_json::to_value(&cloud).unwrap();
    assert_eq!(json["placement"], serde_json::json!("cloud"));
    assert_eq!(
        serde_json::from_value::<ChannelRecord>(json.clone()).unwrap(),
        cloud
    );
    json["placement"] = serde_json::json!("orbit");
    assert!(
        serde_json::from_value::<ChannelRecord>(json).is_err(),
        "an unknown placement fails the read"
    );
}

#[test]
fn a_cloud_placement_is_stored_and_changed_on_its_own() {
    for (mode, store, _dir) in stores() {
        let channels = ChannelStore::for_store(&store);
        let new = NewChannel {
            placement: ChannelPlacement::Cloud,
            ..new_channel_in("zode", "proj-a")
        };
        let ch = channels.create(new, &agent("zns-coord", 0), ts(1)).unwrap();
        assert_eq!(ch.placement, ChannelPlacement::Cloud, "{mode}");
        assert_eq!(channels.get(&ch.id).unwrap(), Some(ch.clone()), "{mode}");

        let retitle = ChannelUpdate {
            title: Some("Zode 2".to_string()),
            ..ChannelUpdate::default()
        };
        let retitled = channels.update(&ch.id, retitle, ts(2)).unwrap();
        assert_eq!(
            retitled.placement,
            ChannelPlacement::Cloud,
            "{mode}: an update without a placement keeps it"
        );
        let to_local = ChannelUpdate {
            placement: Some(ChannelPlacement::Local),
            ..ChannelUpdate::default()
        };
        let local = channels.update(&ch.id, to_local, ts(3)).unwrap();
        assert_eq!(local.placement, ChannelPlacement::Local, "{mode}");
        assert_eq!(local.title, "Zode 2", "{mode}");
        assert_eq!(local.project_id.as_deref(), Some("proj-a"), "{mode}");
        assert_eq!(channels.get(&ch.id).unwrap(), Some(local), "{mode}");
    }

    let update: ChannelUpdate = serde_json::from_str(r#"{"placement":"cloud"}"#).unwrap();
    assert_eq!(update.placement, Some(ChannelPlacement::Cloud));
    assert_eq!(update.project_id, None);
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
