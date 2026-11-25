//! Mail between agents: every message belongs to a thread, and a
//! [`MailObserver`] sees each one as it is delivered.
//!
//! [`Threads`] decides which thread a message goes in:
//!
//! - With a subject, the thread is keyed by the root of the sender's tree
//!   and the normalized subject ([`normalize_subject`]), so the same
//!   subject joins the same conversation, and a new subject starts one.
//! - Without a subject, the message continues the most recent thread
//!   between the two agents, whichever way it went. With none, it starts
//!   a new thread named after the first line of the text.
//! - Some messages always start a thread: a `spawn_agent` first prompt
//!   and a `task` prompt ([`Threads::new_thread`]).
//!
//! Host threads are named `t1`, `t2`, ... in the order they are created.
//!
//! A subject taken from the first line of the text is only a stand-in:
//! [`Resolved::from_text`] marks the call that created such a thread, and
//! the message that opened it carries [`MailMessage::subject_from_text`],
//! so an observer can write a better subject for it later.
//!
//! Both indexes are bounded ([`MAX_INDEX_ENTRIES`]); the entry written
//! longest ago is forgotten first. A forgotten subject or pair just
//! starts a new thread next time.

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::hash::Hash;

use aura_core_types::AgentId;

use super::inbox::MessageKind;

/// Identifies a conversation. Host threads look like `t17`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ThreadId(pub String);

impl fmt::Display for ThreadId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// One message between two agents, as a [`MailObserver`] sees it.
#[derive(Debug, Clone)]
pub struct MailMessage {
    /// Unique within the host, increasing in delivery order.
    pub id: u64,
    /// Conversation the message belongs to.
    pub thread: ThreadId,
    /// Subject of the thread, as first given.
    pub subject: String,
    /// Sender.
    pub from: AgentId,
    /// Sender's display name.
    pub from_name: String,
    /// Recipient.
    pub to: AgentId,
    /// Recipient's display name.
    pub to_name: String,
    /// Whether the message is a request or the answer to one.
    pub kind: MessageKind,
    /// Message body.
    pub text: String,
    /// When it was sent, in Unix milliseconds.
    pub sent_ms: u64,
    /// Whether this message opened its thread and the thread's subject
    /// was made from its text, not given; a better subject may be written
    /// for it.
    pub subject_from_text: bool,
}

/// Sees every message between agents as it is delivered. Called without
/// any host lock held; implementations must not block.
pub trait MailObserver: Send + Sync {
    /// `message` was just delivered.
    fn on_mail(&self, message: &MailMessage);
}

/// Longest subject derived from a message's text, in characters.
const DERIVED_SUBJECT_CHARS: usize = 60;

/// Subject used when a message has no text to name its thread after.
const EMPTY_SUBJECT: &str = "(no subject)";

/// Reply and forward markers stripped from the front of a subject.
const SUBJECT_PREFIXES: [&str; 3] = ["re:", "fwd:", "fw:"];

/// A thread's id and display subject.
#[derive(Debug, Clone)]
struct ThreadRef {
    id: ThreadId,
    subject: String,
}

impl ThreadRef {
    /// This thread as the place a message joins, not one it created.
    fn joined(&self) -> Resolved {
        Resolved {
            thread: self.id.clone(),
            subject: self.subject.clone(),
            from_text: false,
        }
    }
}

/// The thread [`Threads`] picked for a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Resolved {
    /// Conversation the message goes in.
    pub(crate) thread: ThreadId,
    /// Display subject of `thread`.
    pub(crate) subject: String,
    /// Whether this call created `thread` and named it after the
    /// message's text. Only the message that opens such a thread is
    /// marked, so a better subject is asked for once per thread.
    pub(crate) from_text: bool,
}

/// Most entries each [`Threads`] index keeps.
const MAX_INDEX_ENTRIES: usize = 4096;

/// A map that keeps at most `cap` entries, forgetting the one written
/// longest ago when a new key would go past it.
///
/// `order` holds one `(stamp, key)` per write; a write to a key already
/// present leaves its older stamp behind as stale. Stale stamps are
/// skipped on eviction and compacted away once `order` is twice `cap`,
/// so it stays bounded too.
#[derive(Debug)]
struct BoundedIndex<K, V> {
    entries: HashMap<K, (u64, V)>,
    order: VecDeque<(u64, K)>,
    next_stamp: u64,
    cap: usize,
}

impl<K: Hash + Eq + Clone, V> BoundedIndex<K, V> {
    fn new(cap: usize) -> Self {
        Self {
            entries: HashMap::new(),
            order: VecDeque::new(),
            next_stamp: 0,
            cap: cap.max(1),
        }
    }

    fn get(&self, key: &K) -> Option<&V> {
        self.entries.get(key).map(|(_, value)| value)
    }

    fn insert(&mut self, key: K, value: V) {
        self.next_stamp += 1;
        let stamp = self.next_stamp;
        self.order.push_back((stamp, key.clone()));
        self.entries.insert(key, (stamp, value));
        while self.entries.len() > self.cap {
            let Some((stamp, key)) = self.order.pop_front() else {
                break;
            };
            if self.is_current(stamp, &key) {
                self.entries.remove(&key);
            }
        }
        if self.order.len() > self.cap * 2 {
            let entries = &self.entries;
            self.order
                .retain(|(stamp, key)| entries.get(key).is_some_and(|(s, _)| s == stamp));
        }
    }

    fn is_current(&self, stamp: u64, key: &K) -> bool {
        self.entries.get(key).is_some_and(|(s, _)| *s == stamp)
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.len()
    }
}

/// Thread bookkeeping for one host: which subject maps to which thread,
/// the last thread each pair of agents used, and the id counters.
#[derive(Debug)]
pub(crate) struct Threads {
    /// (root of the sender's tree, normalized subject) to thread.
    by_subject: BoundedIndex<(AgentId, String), ThreadRef>,
    /// Unordered agent pair to the thread its latest message went in.
    by_pair: BoundedIndex<(AgentId, AgentId), ThreadRef>,
    next_thread: u64,
    next_message: u64,
}

impl Default for Threads {
    fn default() -> Self {
        Self::with_index_cap(MAX_INDEX_ENTRIES)
    }
}

impl Threads {
    /// Threads whose indexes each keep at most `cap` entries.
    fn with_index_cap(cap: usize) -> Self {
        Self {
            by_subject: BoundedIndex::new(cap),
            by_pair: BoundedIndex::new(cap),
            next_thread: 0,
            next_message: 0,
        }
    }

    /// The thread for a message from `from` to `to` in `root`'s tree.
    /// See the module docs for the rules; only the fallback that names a
    /// new thread after `text` is [`Resolved::from_text`].
    pub(crate) fn resolve(
        &mut self,
        root: AgentId,
        from: AgentId,
        to: AgentId,
        subject: Option<&str>,
        text: &str,
    ) -> Resolved {
        if let Some(subject) = subject {
            let key = normalize_subject(subject);
            if !key.is_empty() {
                if let Some(thread) = self.by_subject.get(&(root, key.clone())) {
                    return thread.joined();
                }
                return self.create(root, key, display_subject(subject), false);
            }
        }
        if let Some(thread) = self.by_pair.get(&pair_key(from, to)) {
            return thread.joined();
        }
        self.new_thread(root, text)
    }

    /// Start a new thread in `root`'s tree, named after the first line of
    /// `text`. The result is always [`Resolved::from_text`].
    pub(crate) fn new_thread(&mut self, root: AgentId, text: &str) -> Resolved {
        let subject = derived_subject(text);
        self.create(root, normalize_subject(&subject), subject, true)
    }

    /// Note that a message from `from` to `to` went in `thread`, and
    /// return the message's id.
    pub(crate) fn record(
        &mut self,
        from: AgentId,
        to: AgentId,
        thread: &ThreadId,
        subject: &str,
    ) -> u64 {
        self.by_pair.insert(
            pair_key(from, to),
            ThreadRef {
                id: thread.clone(),
                subject: subject.to_string(),
            },
        );
        self.next_message += 1;
        self.next_message
    }

    /// A new thread under `key`. A later explicit subject with the same
    /// key joins the newest thread created under it. `from_text` says
    /// whether `subject` was derived from the message rather than given.
    fn create(&mut self, root: AgentId, key: String, subject: String, from_text: bool) -> Resolved {
        self.next_thread += 1;
        let thread = ThreadRef {
            id: ThreadId(format!("t{}", self.next_thread)),
            subject,
        };
        let out = Resolved {
            from_text,
            ..thread.joined()
        };
        if !key.is_empty() {
            self.by_subject.insert((root, key), thread);
        }
        out
    }
}

/// The same key for `(a, b)` and `(b, a)`.
fn pair_key(a: AgentId, b: AgentId) -> (AgentId, AgentId) {
    if a.as_bytes() <= b.as_bytes() {
        (a, b)
    } else {
        (b, a)
    }
}

/// `subject` without leading reply/forward markers and with its
/// whitespace collapsed, in its original case.
fn display_subject(subject: &str) -> String {
    let mut rest = subject.trim();
    loop {
        let stripped = SUBJECT_PREFIXES.iter().find_map(|prefix| {
            rest.get(..prefix.len())
                .filter(|head| head.eq_ignore_ascii_case(prefix))
                .and_then(|_| rest.get(prefix.len()..))
        });
        match stripped {
            Some(next) => rest = next.trim_start(),
            None => break,
        }
    }
    rest.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The key two subjects share when they name the same thread: trimmed,
/// without leading `re:`/`fwd:`/`fw:` markers, whitespace collapsed,
/// lowercased.
pub(crate) fn normalize_subject(subject: &str) -> String {
    display_subject(subject).to_lowercase()
}

/// A subject for a thread started by `text`: its first non-empty line,
/// cut to [`DERIVED_SUBJECT_CHARS`] characters with `…` when cut.
fn derived_subject(text: &str) -> String {
    let Some(line) = text.lines().map(str::trim).find(|line| !line.is_empty()) else {
        return EMPTY_SUBJECT.to_string();
    };
    let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = line.chars();
    let head: String = chars.by_ref().take(DERIVED_SUBJECT_CHARS).collect();
    if chars.next().is_some() {
        format!("{}…", head.trim_end())
    } else {
        head
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization_strips_markers_case_and_spacing() {
        let cases = [
            ("Re: RE:  Foo", "foo"),
            ("  fwd: Re:fw: Foo   Bar ", "foo bar"),
            ("FOO", "foo"),
            ("Reply needed", "reply needed"),
            ("re:", ""),
            ("   ", ""),
        ];
        for (input, expected) in cases {
            assert_eq!(normalize_subject(input), expected, "{input:?}");
        }
    }

    #[test]
    fn same_subject_in_the_same_tree_joins_the_thread() {
        let mut threads = Threads::default();
        let (root, a, b, c) = ids();
        let first = threads.resolve(root, a, b, Some("Parser review"), "hi");
        let again = threads.resolve(root, c, a, Some("Re: RE:  parser REVIEW"), "x");
        assert_eq!(first.thread, again.thread);
        assert_eq!(first.subject, "Parser review");
        assert_eq!(again.subject, "Parser review");
        assert_eq!(first.thread, ThreadId("t1".into()));
    }

    #[test]
    fn different_subject_or_tree_starts_a_new_thread() {
        let mut threads = Threads::default();
        let (root, a, b, _) = ids();
        let other_root = AgentId::generate();
        let first = threads.resolve(root, a, b, Some("Parser"), "hi").thread;
        let other_subject = threads.resolve(root, a, b, Some("Benchmarks"), "hi").thread;
        let other_tree = threads
            .resolve(other_root, a, b, Some("Parser"), "hi")
            .thread;
        assert_ne!(first, other_subject);
        assert_ne!(first, other_tree);
        assert_ne!(other_subject, other_tree);
    }

    #[test]
    fn no_subject_continues_the_last_thread_between_the_pair() {
        let mut threads = Threads::default();
        let (root, a, b, c) = ids();
        let first = threads.resolve(root, a, b, Some("Parser"), "hi");
        threads.record(a, b, &first.thread, &first.subject);

        let reply = threads.resolve(root, b, a, None, "answer");
        assert_eq!(reply.thread, first.thread);
        assert_eq!(reply.subject, "Parser");

        let fresh = threads.resolve(root, a, c, None, "\n  Check the lexer\nmore");
        assert_ne!(fresh.thread, first.thread);
        assert_eq!(fresh.subject, "Check the lexer");
    }

    #[test]
    fn the_latest_recorded_thread_wins_for_the_pair() {
        let mut threads = Threads::default();
        let (root, a, b, _) = ids();
        let first = threads.resolve(root, a, b, Some("One"), "x");
        threads.record(a, b, &first.thread, &first.subject);
        let second = threads.resolve(root, a, b, Some("Two"), "x");
        threads.record(b, a, &second.thread, &second.subject);
        assert_eq!(threads.resolve(root, a, b, None, "x").thread, second.thread);
    }

    #[test]
    fn new_thread_always_creates_and_can_be_joined_by_subject() {
        let mut threads = Threads::default();
        let (root, a, b, _) = ids();
        let first = threads.new_thread(root, "Look at the parser");
        let second = threads.new_thread(root, "Look at the parser");
        assert_ne!(first.thread, second.thread);
        assert_eq!(first.subject, "Look at the parser");
        let joined = threads.resolve(root, b, a, Some("re: look at the parser"), "x");
        assert_eq!(
            joined.thread, second.thread,
            "the newest thread with that subject"
        );
    }

    #[test]
    fn only_a_thread_created_from_the_text_is_marked() {
        let mut threads = Threads::default();
        let (root, a, b, c) = ids();

        let started = threads.new_thread(root, "Look at the parser");
        assert!(
            started.from_text,
            "new_thread names the thread after the text"
        );

        let fallback = threads.resolve(root, a, b, None, "Check the lexer");
        assert!(
            fallback.from_text,
            "no pair and no subject: named from text"
        );
        threads.record(a, b, &fallback.thread, &fallback.subject);

        let given = threads.resolve(root, a, c, Some("Benchmarks"), "x");
        assert!(!given.from_text, "an explicit subject is not derived");

        let continued = threads.resolve(root, b, a, None, "answer");
        assert_eq!(continued.thread, fallback.thread);
        assert!(!continued.from_text, "continuing the pair creates nothing");

        let joined = threads.resolve(root, c, b, Some("re: look at the parser"), "x");
        assert_eq!(joined.thread, started.thread);
        assert!(
            !joined.from_text,
            "joining an existing subject creates nothing"
        );
    }

    #[test]
    fn derived_subjects_are_cut_on_char_boundaries() {
        let long = "é".repeat(80);
        let subject = derived_subject(&long);
        assert_eq!(subject.chars().count(), DERIVED_SUBJECT_CHARS + 1);
        assert!(subject.ends_with('…'));
        assert_eq!(derived_subject(&"a".repeat(60)), "a".repeat(60));
        assert_eq!(derived_subject(" \n\t\n"), EMPTY_SUBJECT);
    }

    #[test]
    fn message_ids_increase() {
        let mut threads = Threads::default();
        let (root, a, b, _) = ids();
        let Resolved {
            thread, subject, ..
        } = threads.new_thread(root, "x");
        let first = threads.record(a, b, &thread, &subject);
        let second = threads.record(b, a, &thread, &subject);
        assert!(second > first);
    }

    #[test]
    fn index_cap_evicts_the_entry_written_longest_ago() {
        let mut index = BoundedIndex::new(2);
        index.insert("a", 1);
        index.insert("b", 2);
        index.insert("a", 3);
        index.insert("c", 4);
        assert_eq!(index.len(), 2);
        assert_eq!(index.get(&"b"), None, "b was written longest ago");
        assert_eq!(index.get(&"a"), Some(&3), "rewriting a kept it fresh");
        assert_eq!(index.get(&"c"), Some(&4));

        for round in 0..100 {
            index.insert("a", round);
        }
        assert!(index.order.len() <= 2 * 2 + 1, "stale stamps are compacted");
        assert_eq!(index.get(&"c"), Some(&4));
    }

    #[test]
    fn capped_threads_forget_the_oldest_pair_and_subject() {
        let mut threads = Threads::with_index_cap(2);
        let (root, a, b, c) = ids();
        let d = AgentId::generate();
        let ab = threads.resolve(root, a, b, Some("First"), "x");
        threads.record(a, b, &ab.thread, &ab.subject);
        let ac = threads.resolve(root, a, c, Some("Second"), "x");
        threads.record(a, c, &ac.thread, &ac.subject);
        let ad = threads.resolve(root, a, d, Some("Third"), "x");
        threads.record(a, d, &ad.thread, &ad.subject);

        assert_eq!(
            threads.resolve(root, a, b, Some("Third"), "x").thread,
            ad.thread
        );
        assert_ne!(
            threads.resolve(root, a, b, Some("First"), "x").thread,
            ab.thread,
            "the oldest subject was evicted"
        );
        assert_ne!(
            threads.resolve(root, b, a, None, "x").thread,
            ab.thread,
            "a↔b evicted"
        );
        assert_eq!(threads.resolve(root, d, a, None, "x").thread, ad.thread);
    }

    fn ids() -> (AgentId, AgentId, AgentId, AgentId) {
        (
            AgentId::generate(),
            AgentId::generate(),
            AgentId::generate(),
            AgentId::generate(),
        )
    }
}
