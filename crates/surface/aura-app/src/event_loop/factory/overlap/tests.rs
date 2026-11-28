use super::*;
use aura_exec_merge_queue::{MergeRequest, Origin, QueueState, QueuedPush};

fn hex(byte: u8) -> String {
    AgentId::new([byte; 32]).to_hex()
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| (*item).to_string()).collect()
}

fn work(entry: Option<u64>, branch: &str, byte: u8, name: &str, files: &[&str]) -> Work {
    Work {
        entry: entry.map(EntryId::from_seq),
        branch: branch.to_string(),
        agent: Some(hex(byte)),
        agent_name: Some(name.to_string()),
        files: strings(files),
    }
}

fn overlap_of(subject: &Work, other: &Work) -> Overlap {
    let mut found = overlap::find(subject, std::slice::from_ref(other));
    assert_eq!(found.len(), 1, "{found:?}");
    found.remove(0)
}

#[test]
fn both_owners_are_told_with_the_other_named() {
    let subject = work(Some(2), "aura/a", 1, "RUS1", &["src/x.rs", "src/y.rs"]);
    let other = work(None, "aura/b", 2, "RUS2", &["src/y.rs"]);
    let found = overlap_of(&subject, &other);
    let sent = notices(&subject, &found);
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[0].to, AgentId::new([1; 32]));
    assert!(sent[0].shown.contains("q2 aura/a"), "{}", sent[0].shown);
    assert!(sent[0].prompt.contains("RUS2"), "{}", sent[0].prompt);
    assert!(sent[0].prompt.contains("src/y.rs"), "{}", sent[0].prompt);
    assert!(
        sent[0].prompt.contains("send_to_agent"),
        "{}",
        sent[0].prompt
    );
    assert!(!sent[0].prompt.contains("src/x.rs"), "only shared files");
    assert_eq!(sent[1].to, AgentId::new([2; 32]));
    assert!(sent[1].prompt.contains("RUS1"), "{}", sent[1].prompt);
    assert!(
        sent[1].shown.contains("worktree aura/b"),
        "{}",
        sent[1].shown
    );
    assert!(sent.iter().all(|notice| notice.inbox.is_none()));
}

#[test]
fn an_owner_with_no_known_agent_is_not_told() {
    let subject = work(Some(1), "aura/a", 1, "RUS1", &["a.rs"]);
    let mut other = work(Some(2), "aura/b", 2, "RUS2", &["a.rs"]);
    other.agent = None;
    let sent = notices(&subject, &overlap_of(&subject, &other));
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].to, AgentId::new([1; 32]));
}

#[test]
fn a_pair_is_told_once_until_its_files_change() {
    let mut told = Told::default();
    let a = work(Some(1), "aura/a", 1, "RUS1", &["a.rs", "b.rs"]);
    let b = work(Some(2), "aura/b", 2, "RUS2", &["a.rs"]);
    assert!(told.first_time(&a, &overlap_of(&a, &b)));
    assert!(!told.first_time(&a, &overlap_of(&a, &b)), "a re-push");
    assert!(
        !told.first_time(&b, &overlap_of(&b, &a)),
        "the other way round"
    );
    let b_more = work(Some(2), "aura/b", 2, "RUS2", &["a.rs", "b.rs"]);
    assert!(told.first_time(&a, &overlap_of(&a, &b_more)));
}

fn queued(state: &mut QueueState, branch: &str, byte: u8, files: &[&str]) {
    let head = format!("{branch}-head");
    let request = MergeRequest {
        push: QueuedPush {
            branch: branch.to_string(),
            head: head.clone(),
            remote: None,
        },
        origin: Origin {
            agent: Some(hex(byte)),
            agent_name: Some(format!("RUS{byte}")),
            channel: None,
        },
    };
    let id = state.enqueue(request, Utc::now()).id().clone();
    state.record_files(&id, &head, strings(files));
}

#[test]
fn the_queue_tab_lists_each_open_entrys_overlaps() {
    let mut state = QueueState::default();
    queued(&mut state, "aura/a", 1, &["src/x.rs", "CHANGELOG.md"]);
    queued(&mut state, "aura/b", 2, &["src/x.rs", "CHANGELOG.md"]);
    queued(&mut state, "aura/c", 3, &["src/z.rs"]);
    let found = entry_overlaps(&state.entries, "/no/such/repo");
    assert_eq!(
        found.get(&EntryId::from_seq(1)),
        Some(&strings(&["q2 aura/b (RUS2): src/x.rs"]))
    );
    assert_eq!(
        found.get(&EntryId::from_seq(2)),
        Some(&strings(&["q1 aura/a (RUS1): src/x.rs"]))
    );
    assert!(!found.contains_key(&EntryId::from_seq(3)));
}

#[test]
fn long_file_lists_are_cut() {
    let files = strings(&["1", "2", "3", "4", "5", "6", "7"]);
    assert_eq!(file_list(&files), "1, 2, 3, 4, 5 and 2 more");
    assert_eq!(file_list(&files[..2]), "1, 2");
}
