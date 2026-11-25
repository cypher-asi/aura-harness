//! The hub's view of the mesh: which node holds which agent, where to
//! reach it, mail ids and dedup.
//!
//! The hub is node `hub`. Each cloud machine is a node named after the
//! machine. [`Directory`] answers "where does mail for agent X go" and
//! builds the [`MeshFrame::Directory`] each node of a project receives,
//! so nodes can send mail to each other directly.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use aura_core_protocol::cloud::{MeshFrame, MeshMail, MeshPeer};
use parking_lot::Mutex;

/// Node id of the local app.
pub const HUB_NODE: &str = "hub";

/// Most mail ids remembered per sending node for dedup.
const DEDUP_WINDOW: usize = 4096;

/// Where an agent is held.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Holder {
    /// Node id ([`HUB_NODE`] or a machine name).
    pub node: String,
    /// The node's API base URL (`None` for the hub).
    pub url: Option<String>,
    /// Project the agent is in.
    pub project: String,
    /// Display name.
    pub name: String,
    /// Owner tag of the user the agent belongs to. Agents only see and
    /// mail agents of the same owner.
    pub owner: String,
}

/// Which node holds which agent.
#[derive(Debug, Default)]
pub struct Directory {
    agents: Mutex<BTreeMap<String, Holder>>,
}

impl Directory {
    /// An empty directory.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record that `agent` (hex) is held by `holder`.
    pub fn place(&self, agent: &str, holder: Holder) {
        self.agents.lock().insert(agent.to_string(), holder);
    }

    /// Forget `agent`.
    pub fn remove(&self, agent: &str) {
        self.agents.lock().remove(agent);
    }

    /// Where `agent` is held.
    #[must_use]
    pub fn holder(&self, agent: &str) -> Option<Holder> {
        self.agents.lock().get(agent).cloned()
    }

    /// Whether mail for `agent` must leave the hub.
    #[must_use]
    pub fn is_remote(&self, agent: &str) -> bool {
        self.holder(agent)
            .is_some_and(|holder| holder.node != HUB_NODE)
    }

    /// The nodes (machine names) holding agents of `project`.
    #[must_use]
    pub fn nodes_of(&self, project: &str) -> Vec<String> {
        let set: HashSet<String> = self
            .agents
            .lock()
            .values()
            .filter(|holder| holder.project == project && holder.node != HUB_NODE)
            .map(|holder| holder.node.clone())
            .collect();
        let mut nodes: Vec<String> = set.into_iter().collect();
        nodes.sort();
        nodes
    }

    /// The owner of the agents `node` holds.
    #[must_use]
    pub fn owner_of_node(&self, node: &str) -> Option<String> {
        self.agents
            .lock()
            .values()
            .find(|holder| holder.node == node)
            .map(|holder| holder.owner.clone())
    }

    /// The owner of `agent`, if a node holds it.
    #[must_use]
    pub fn owner_of(&self, agent: &str) -> Option<String> {
        self.holder(agent).map(|holder| holder.owner)
    }

    /// The directory frame for `node`: every agent of `project` with the
    /// same owner that it does not hold itself. Other users' agents are
    /// never listed.
    #[must_use]
    pub fn frame_for(&self, node: &str, project: &str) -> MeshFrame {
        let owner = self.owner_of_node(node);
        let peers = self
            .agents
            .lock()
            .iter()
            .filter(|(_, holder)| holder.project == project && holder.node != node)
            .filter(|(_, holder)| owner.as_deref() == Some(holder.owner.as_str()))
            .map(|(agent, holder)| MeshPeer {
                agent_id: agent.clone(),
                name: holder.name.clone(),
                node_id: holder.node.clone(),
                url: holder.url.clone(),
            })
            .collect();
        MeshFrame::Directory { peers }
    }
}

/// Ids seen from one node: a set for lookups, an order for eviction.
type SeenIds = (HashSet<u64>, VecDeque<u64>);

/// Mail ids the hub sends, and the ids it has seen from each node.
#[derive(Debug, Default)]
pub struct MailBook {
    next_id: Mutex<u64>,
    seen: Mutex<HashMap<String, SeenIds>>,
}

impl MailBook {
    /// An empty book. Ids start at the wall clock in microseconds, so a
    /// restarted sender never reuses an id its receivers still remember.
    #[must_use]
    pub fn new() -> Self {
        Self {
            next_id: Mutex::new(first_mail_id()),
            seen: Mutex::new(HashMap::new()),
        }
    }

    /// Forget `mail`, so a resend is handled again (its delivery failed).
    pub fn forget(&self, mail: &MeshMail) {
        if let Some((set, order)) = self.seen.lock().get_mut(&mail.from_node) {
            set.remove(&mail.id);
            order.retain(|id| *id != mail.id);
        }
    }

    /// A fresh id for mail the hub sends.
    pub fn next_id(&self) -> u64 {
        let mut next = self.next_id.lock();
        *next += 1;
        *next
    }

    /// Whether `mail` is new; remembers it either way. Mail is delivered
    /// at least once, so a resent copy must be dropped here.
    pub fn first_sight(&self, mail: &MeshMail) -> bool {
        let mut seen = self.seen.lock();
        let (set, order) = seen.entry(mail.from_node.clone()).or_default();
        if !set.insert(mail.id) {
            return false;
        }
        order.push_back(mail.id);
        if order.len() > DEDUP_WINDOW {
            if let Some(old) = order.pop_front() {
                set.remove(&old);
            }
        }
        true
    }
}

/// The first id a sender hands out: microseconds since 1970, so ids keep
/// growing across restarts.
#[must_use]
pub fn first_mail_id() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(1, |since| {
            u64::try_from(since.as_micros()).unwrap_or(u64::MAX / 2)
        })
}

/// A thread id as it travels between nodes: `<node>:<local id>`. Ids
/// that already carry a node prefix are kept.
#[must_use]
pub fn global_thread(node: &str, local: &str) -> String {
    if local.contains(':') {
        local.to_string()
    } else {
        format!("{node}:{local}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aura_core_protocol::cloud::MeshMailKind;

    fn holder(node: &str, project: &str) -> Holder {
        Holder {
            owner: "a11ce000".into(),
            node: node.into(),
            url: (node != HUB_NODE).then(|| format!("https://{node}.boxd.sh")),
            project: project.into(),
            name: format!("agent on {node}"),
        }
    }

    #[test]
    fn the_directory_routes_and_lists_peers() {
        let dir = Directory::new();
        dir.place("aa", holder(HUB_NODE, "p1"));
        dir.place("bb", holder("aura-re36-3f9c", "p1"));
        dir.place("cc", holder("aura-ex2-1111", "p1"));
        dir.place("dd", holder("aura-other-2222", "p2"));
        assert!(!dir.is_remote("aa"));
        assert!(dir.is_remote("bb"));
        assert!(!dir.is_remote("zz"));
        assert_eq!(dir.nodes_of("p1"), vec!["aura-ex2-1111", "aura-re36-3f9c"]);

        let MeshFrame::Directory { peers } = dir.frame_for("aura-re36-3f9c", "p1") else {
            panic!("not a directory frame");
        };
        let ids: Vec<&str> = peers.iter().map(|p| p.agent_id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["aa", "cc"],
            "own agents and other projects left out"
        );
        assert_eq!(
            peers[0].url, None,
            "local agents are reached through the hub"
        );
        assert_eq!(
            peers[1].url.as_deref(),
            Some("https://aura-ex2-1111.boxd.sh")
        );
    }

    #[test]
    fn directories_never_list_another_owners_agents() {
        let dir = Directory::new();
        dir.place("aa", holder("aura-a-1111", "p1"));
        dir.place(
            "bb",
            Holder {
                owner: "b0b00000".into(),
                ..holder("aura-b-2222", "p1")
            },
        );
        dir.place("cc", holder("aura-c-3333", "p1"));
        assert_eq!(dir.owner_of("bb").as_deref(), Some("b0b00000"));
        let MeshFrame::Directory { peers } = dir.frame_for("aura-a-1111", "p1") else {
            panic!("not a directory frame");
        };
        let ids: Vec<&str> = peers.iter().map(|p| p.agent_id.as_str()).collect();
        assert_eq!(ids, vec!["cc"]);
        let MeshFrame::Directory { peers } = dir.frame_for("aura-b-2222", "p1") else {
            panic!("not a directory frame");
        };
        assert!(peers.is_empty(), "Bob's node sees none of Alice's agents");
    }

    #[test]
    fn resent_mail_is_dropped() {
        let book = MailBook::new();
        let mail = MeshMail {
            id: 1,
            from_node: "n1".into(),
            thread: "n1:t1".into(),
            subject: "s".into(),
            from: "aa".into(),
            from_name: "a".into(),
            to: "bb".into(),
            kind: MeshMailKind::Request,
            text: "hi".into(),
            sent_ms: 0,
        };
        assert!(book.first_sight(&mail));
        assert!(!book.first_sight(&mail));
        let other_node = MeshMail {
            from_node: "n2".into(),
            ..mail
        };
        assert!(book.first_sight(&other_node));
        book.forget(&other_node);
        assert!(
            book.first_sight(&other_node),
            "a forgotten id is handled again"
        );
        let first = book.next_id();
        assert!(first > 1_000_000, "ids start at the clock: {first}");
        assert_eq!(book.next_id(), first + 1);
    }

    #[test]
    fn threads_get_a_node_prefix_once() {
        assert_eq!(global_thread(HUB_NODE, "t17"), "hub:t17");
        assert_eq!(global_thread(HUB_NODE, "aura-x:t2"), "aura-x:t2");
    }
}
