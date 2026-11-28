//! The saved inbox's file: round trips, bad files, limits, and entries
//! for agents that are gone.

use super::file::{
    self, SavedKind, SavedNotice, SavedOwed, SavedQueued, SavedRequest, MAX_BYTES, MAX_QUEUED,
    VERSION,
};
use super::snapshot::live_entries;
use super::*;
use crate::event_loop::subagents::tests::fixture;
use tempfile::TempDir;

fn owed(agent: &str, from: &str, id: &str, withdrawn: bool) -> SavedOwed {
    SavedOwed {
        agent: agent.to_string(),
        id: id.to_string(),
        kind: SavedKind::Assignment,
        from: from.to_string(),
        thread: "inbox:0123456789abcdef".to_string(),
        subject: "Footer".to_string(),
        todo: Some("3".to_string()),
        withdrawn,
    }
}

fn queued(agent: &str, text: &str) -> SavedQueued {
    SavedQueued {
        agent: agent.to_string(),
        text: text.to_string(),
        request: SavedRequest::New,
        ..SavedQueued::default()
    }
}

fn sample() -> Snapshot {
    let (a, b) = ("ab".repeat(32), "cd".repeat(32));
    Snapshot {
        owed: vec![owed(&a, &b, "1", false), owed(&a, &b, "2", true)],
        queued: vec![queued(
            &a,
            "PRC1: Footer\n(inbox assignment 1 from PRC1, x)",
        )],
        outbox: vec![SavedNotice {
            to: b,
            shown: "✉ REN4 replied: Footer".to_string(),
            prompt: "REN4 replied: Footer\n(inbox reply 9 from REN4, y)\n\nDone.".to_string(),
            kind: SavedKind::Reply,
        }],
        ..Snapshot::default()
    }
}

#[test]
fn a_saved_inbox_reads_back_as_it_was_written() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join(SAVED_FILE);
    file::save(&path, &sample()).unwrap();
    assert_eq!(file::load(&path), sample());
    assert!(
        !path.with_extension("json.tmp").exists(),
        "the temporary file is renamed away"
    );
}

#[test]
fn a_missing_file_is_an_empty_inbox() {
    let dir = TempDir::new().unwrap();
    assert_eq!(
        file::load(&dir.path().join(SAVED_FILE)),
        Snapshot::default()
    );
}

#[test]
fn a_bad_file_is_an_empty_inbox_and_the_next_save_replaces_it() {
    let wrong_version = serde_json::to_vec(&Snapshot {
        version: VERSION + 1,
        ..sample()
    })
    .unwrap();
    let too_large = vec![b' '; usize::try_from(MAX_BYTES).unwrap() + 1];
    let cases: [(&str, &[u8]); 4] = [
        ("corrupt", b"{\"version\": 1, \"owed\": [tr"),
        ("not this shape", b"[1, 2, 3]"),
        ("another version", &wrong_version),
        ("too large", &too_large),
    ];
    for (case, bytes) in cases {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join(SAVED_FILE);
        std::fs::write(&path, bytes).unwrap();
        assert_eq!(file::load(&path), Snapshot::default(), "{case}");
        file::save(&path, &sample()).unwrap();
        assert_eq!(file::load(&path), sample(), "{case}: replaced");
    }
}

#[test]
fn a_saved_inbox_keeps_within_its_limits_oldest_first() {
    let agent = "ab".repeat(32);
    let snapshot = Snapshot {
        queued: (0..MAX_QUEUED + 5)
            .map(|at| queued(&agent, &at.to_string()))
            .collect(),
        ..Snapshot::default()
    }
    .capped();
    assert_eq!(snapshot.queued.len(), MAX_QUEUED);
    assert_eq!(snapshot.queued[0].text, "0");
    assert_eq!(
        snapshot.queued[MAX_QUEUED - 1].text,
        (MAX_QUEUED - 1).to_string()
    );
}

#[tokio::test]
async fn entries_for_agents_gone_or_archived_are_dropped_on_restore() {
    let fixture = fixture("unused");
    let hub = &fixture.hub;
    let (member, coordinator, archived) = {
        let mut registry = lock(&hub.registry);
        let member = registry.active().agent_id();
        let coordinator = registry.create(Some("Lead".into())).unwrap().agent_id();
        let archived = registry.create(Some("Old".into())).unwrap().agent_id();
        registry.archive(archived).unwrap();
        (member, coordinator, archived)
    };
    let (member, coordinator, archived) =
        (member.to_hex(), coordinator.to_hex(), archived.to_hex());
    let unknown = "ef".repeat(32);
    let snapshot = Snapshot {
        owed: vec![
            owed(&member, &coordinator, "kept", true),
            owed(&member, &unknown, "unknown sender", false),
            owed(&archived, &coordinator, "archived member", false),
            owed("not hex", &coordinator, "bad id", false),
        ],
        queued: vec![queued(&member, "kept"), queued(&archived, "dropped")],
        ..Snapshot::default()
    };

    let restored = live_entries(hub, snapshot);
    let ids: Vec<&str> = restored.owed.iter().map(|(_, o)| o.id.as_str()).collect();
    assert_eq!(ids, ["kept"]);
    let (_, kept) = &restored.owed[0];
    assert!(kept.withdrawn, "taken back stays taken back");
    assert_eq!(kept.todo.as_deref(), Some("3"));
    assert_eq!(kept.from.to_hex(), coordinator);
    let texts: Vec<&str> = restored.queued.iter().map(|q| q.1.text.as_str()).collect();
    assert_eq!(texts, ["kept"]);
}
