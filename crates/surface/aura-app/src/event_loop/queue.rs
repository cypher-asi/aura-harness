//! Work waiting for busy agents.
//!
//! Every entry gets an id when it is queued, stable while it waits, so the
//! UI can name the entry it removes, edits, moves or sends now even after
//! the queue changed under it. Ids come from one counter shared by every
//! agent and are never reused within a run.
//!
//! An agent with an empty queue has no entry in the map, so "has queued
//! work" is a map lookup.

use super::standing::QueueCounts;
use super::turns::Work;
use aura_core_types::AgentId;
use aura_ui::events::QueuedItemView;
use std::collections::{HashMap, VecDeque};

/// One piece of queued work and its id.
pub(super) struct Queued {
    pub id: u64,
    pub work: Work,
}

/// What became of an edit of a queued entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EditOutcome {
    /// The message has the new text
    Applied,
    /// No entry has that id: it started, or was removed
    Gone,
    /// The entry is work other than a message, which has no text to edit
    NotAMessage,
}

impl EditOutcome {
    /// What to tell the user about an edit that was not applied.
    pub(super) const fn warning(self) -> Option<&'static str> {
        match self {
            Self::Applied => None,
            Self::Gone => Some("That message already started; your edit wasn't applied"),
            Self::NotAMessage => Some("Only queued messages can be edited"),
        }
    }
}

/// Queues of the agents, oldest first.
#[derive(Default)]
pub(super) struct WorkQueues {
    by_agent: HashMap<AgentId, VecDeque<Queued>>,
    next_id: u64,
    /// How long each queue is, kept for the agents' peers as every
    /// change happens ([`super::standing`]), so no path that changes a
    /// queue leaves its count behind
    counts: QueueCounts,
}

impl WorkQueues {
    /// Empty queues whose lengths are kept in `counts`.
    pub(super) fn counted(counts: QueueCounts) -> Self {
        Self {
            counts,
            ..Self::default()
        }
    }

    fn fresh_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    /// Keep `agent_id`'s count as long as its queue now is.
    fn count(&self, agent_id: AgentId) {
        let mut counts = self
            .counts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match self.len(agent_id) {
            0 => counts.remove(&agent_id),
            len => counts.insert(agent_id, len),
        };
    }

    /// Queue `work` behind what `agent_id` already waits for.
    pub(super) fn push_back(&mut self, agent_id: AgentId, work: Work) {
        let id = self.fresh_id();
        self.by_agent
            .entry(agent_id)
            .or_default()
            .push_back(Queued { id, work });
        self.count(agent_id);
    }

    /// Queue `work` ahead of what `agent_id` already waits for.
    pub(super) fn push_front(&mut self, agent_id: AgentId, work: Work) {
        let id = self.fresh_id();
        self.by_agent
            .entry(agent_id)
            .or_default()
            .push_front(Queued { id, work });
        self.count(agent_id);
    }

    /// Take the oldest work of `agent_id`.
    pub(super) fn pop_front(&mut self, agent_id: AgentId) -> Option<Work> {
        let queue = self.by_agent.get_mut(&agent_id)?;
        let next = queue.pop_front();
        if queue.is_empty() {
            self.by_agent.remove(&agent_id);
        }
        self.count(agent_id);
        next.map(|queued| queued.work)
    }

    /// How much work waits for `agent_id`.
    pub(super) fn len(&self, agent_id: AgentId) -> usize {
        self.by_agent.get(&agent_id).map_or(0, VecDeque::len)
    }

    /// Whether no agent has queued work.
    #[cfg(test)]
    pub(super) fn is_empty(&self) -> bool {
        self.by_agent.is_empty()
    }

    /// Drop the work waiting for `agent_id`.
    pub(super) fn drop_all(&mut self, agent_id: AgentId) {
        self.by_agent.remove(&agent_id);
        self.count(agent_id);
    }

    /// Drop every agent's queue.
    pub(super) fn clear(&mut self) {
        self.by_agent.clear();
        self.counts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }

    fn position(&self, agent_id: AgentId, id: u64) -> Option<usize> {
        self.by_agent
            .get(&agent_id)?
            .iter()
            .position(|queued| queued.id == id)
    }

    /// Take entry `id` of `agent_id` out of its queue, if it is there.
    pub(super) fn remove(&mut self, agent_id: AgentId, id: u64) -> Option<Work> {
        let at = self.position(agent_id, id)?;
        let queue = self.by_agent.get_mut(&agent_id)?;
        let removed = queue.remove(at);
        if queue.is_empty() {
            self.by_agent.remove(&agent_id);
        }
        self.count(agent_id);
        removed.map(|queued| queued.work)
    }

    /// The text of queued message `id` of `agent_id`, if it is one.
    pub(super) fn text_of(&self, agent_id: AgentId, id: u64) -> Option<String> {
        let queued = self
            .by_agent
            .get(&agent_id)?
            .iter()
            .find(|queued| queued.id == id)?;
        match &queued.work {
            Work::Prompt { prompt, .. } => Some(prompt.text.clone()),
            Work::Command { .. } | Work::Compact { .. } | Work::Build { .. } => None,
        }
    }

    /// Move entry `id` of `agent_id` to index `to`, clamped to the queue.
    /// Returns whether it was there.
    pub(super) fn move_to(&mut self, agent_id: AgentId, id: u64, to: usize) -> bool {
        let Some(at) = self.position(agent_id, id) else {
            return false;
        };
        let Some(queue) = self.by_agent.get_mut(&agent_id) else {
            return false;
        };
        let Some(entry) = queue.remove(at) else {
            return false;
        };
        let to = to.min(queue.len());
        queue.insert(to, entry);
        true
    }

    /// Replace the text of queued message `id` of `agent_id`, keeping its
    /// images and mentions. Other work is left as it is.
    pub(super) fn edit(&mut self, agent_id: AgentId, id: u64, text: String) -> EditOutcome {
        let Some(queued) = self
            .by_agent
            .get_mut(&agent_id)
            .and_then(|queue| queue.iter_mut().find(|queued| queued.id == id))
        else {
            return EditOutcome::Gone;
        };
        match &mut queued.work {
            Work::Prompt { prompt, .. } => {
                prompt.text = text;
                EditOutcome::Applied
            }
            Work::Command { .. } | Work::Compact { .. } | Work::Build { .. } => {
                EditOutcome::NotAMessage
            }
        }
    }

    /// The queue of `agent_id` as the UI shows it.
    pub(super) fn views(&self, agent_id: AgentId) -> Vec<QueuedItemView> {
        self.by_agent
            .get(&agent_id)
            .map(|queue| queue.iter().map(view).collect())
            .unwrap_or_default()
    }

    /// Every queued item with its agent, the agents in hex order and each
    /// queue oldest first: what survives a restart ([`super::inbox`]).
    pub(super) fn items(&self) -> Vec<(AgentId, &Work)> {
        let mut agents: Vec<(&AgentId, &VecDeque<Queued>)> = self.by_agent.iter().collect();
        agents.sort_by_cached_key(|(agent, _)| agent.to_hex());
        agents
            .into_iter()
            .flat_map(|(agent, queue)| queue.iter().map(move |queued| (*agent, &queued.work)))
            .collect()
    }
}

/// `queued` as the UI shows it: a message's text, or a label naming the
/// command that typed the other work.
fn view(queued: &Queued) -> QueuedItemView {
    let (text, editable) = match &queued.work {
        Work::Prompt { prompt, .. } => (prompt.text.clone(), true),
        Work::Command { command, .. } => (format!("! {command}"), false),
        Work::Compact { focus } => (with_argument("/compact", focus.as_deref()), false),
        Work::Build { path } => (with_argument("/build", path.as_deref()), false),
    };
    QueuedItemView {
        id: queued.id,
        text,
        editable,
    }
}

fn with_argument(command: &str, argument: Option<&str>) -> String {
    match argument {
        Some(argument) if !argument.trim().is_empty() => format!("{command} {argument}"),
        _ => command.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::super::turns::Prompt;
    use super::*;

    fn agent() -> AgentId {
        AgentId::from_hex(&"ab".repeat(32)).unwrap()
    }

    fn prompt(text: &str) -> Work {
        Work::Prompt {
            prompt: Prompt::typed(text.to_string(), Vec::new()),
            shown: false,
        }
    }

    fn queue_of(texts: &[&str]) -> WorkQueues {
        let mut queues = WorkQueues::default();
        for text in texts {
            queues.push_back(agent(), prompt(text));
        }
        queues
    }

    fn texts(queues: &WorkQueues) -> Vec<String> {
        queues.views(agent()).into_iter().map(|v| v.text).collect()
    }

    fn id_at(queues: &WorkQueues, at: usize) -> u64 {
        queues.views(agent())[at].id
    }

    #[test]
    fn entries_get_distinct_ids_in_run_order() {
        let mut queues = queue_of(&["a", "b"]);
        queues.push_front(agent(), prompt("unseen"));
        let views = queues.views(agent());
        assert_eq!(texts(&queues), ["unseen", "a", "b"]);
        let mut ids: Vec<u64> = views.iter().map(|v| v.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 3, "every entry has its own id");
    }

    #[test]
    fn views_label_work_that_is_not_a_message() {
        let mut queues = WorkQueues::default();
        let work = [
            Work::Command {
                command: "ls -la".to_string(),
                cols: 80,
            },
            Work::Compact { focus: None },
            Work::Compact {
                focus: Some("tests".to_string()),
            },
            Work::Build { path: None },
            Work::Build {
                path: Some("plan.md".to_string()),
            },
            prompt("hi"),
        ];
        for work in work {
            queues.push_back(agent(), work);
        }
        let views = queues.views(agent());
        let shown: Vec<(&str, bool)> = views
            .iter()
            .map(|v| (v.text.as_str(), v.editable))
            .collect();
        assert_eq!(
            shown,
            [
                ("! ls -la", false),
                ("/compact", false),
                ("/compact tests", false),
                ("/build", false),
                ("/build plan.md", false),
                ("hi", true),
            ]
        );
    }

    #[test]
    fn removing_the_last_entry_drops_the_agents_queue() {
        let mut queues = queue_of(&["only"]);
        let id = id_at(&queues, 0);
        assert!(queues.remove(agent(), id).is_some());
        assert!(queues.is_empty());
        assert_eq!(queues.len(agent()), 0);
        assert!(queues.remove(agent(), id).is_none(), "already gone");
    }

    #[test]
    fn remove_takes_only_the_named_entry() {
        let mut queues = queue_of(&["a", "b", "c"]);
        let id = id_at(&queues, 1);
        assert!(queues.remove(agent(), id).is_some());
        assert_eq!(texts(&queues), ["a", "c"]);
    }

    #[test]
    fn move_to_reorders_and_clamps_the_index() {
        let mut queues = queue_of(&["a", "b", "c"]);
        let c = id_at(&queues, 2);
        assert!(queues.move_to(agent(), c, 0));
        assert_eq!(texts(&queues), ["c", "a", "b"]);
        let a = id_at(&queues, 1);
        assert!(queues.move_to(agent(), a, 99));
        assert_eq!(texts(&queues), ["c", "b", "a"]);
        assert!(!queues.move_to(agent(), 12_345, 0), "unknown id");
        assert_eq!(texts(&queues), ["c", "b", "a"]);
    }

    #[test]
    fn edit_replaces_a_message_and_keeps_its_images() {
        let mut queues = WorkQueues::default();
        let image = aura_ui::events::ImageAttachment {
            media_type: "image/png".to_string(),
            data: "AAAA".to_string(),
            bytes: 3,
        };
        queues.push_back(
            agent(),
            Work::Prompt {
                prompt: Prompt::typed("old [image 1]".to_string(), vec![image]),
                shown: false,
            },
        );
        let id = id_at(&queues, 0);
        assert_eq!(
            queues.edit(agent(), id, "new [image 1]".to_string()),
            EditOutcome::Applied
        );
        assert_eq!(texts(&queues), ["new [image 1]"]);
        let Some(Work::Prompt { prompt, .. }) = queues.pop_front(agent()) else {
            panic!("the message is still queued");
        };
        assert_eq!(prompt.images.len(), 1);
        assert!(prompt.mentions);
    }

    #[test]
    fn edit_of_work_that_is_not_a_message_is_ignored() {
        let mut queues = WorkQueues::default();
        queues.push_back(
            agent(),
            Work::Command {
                command: "make".to_string(),
                cols: 80,
            },
        );
        let id = id_at(&queues, 0);
        assert_eq!(
            queues.edit(agent(), id, "rm -rf /".to_string()),
            EditOutcome::NotAMessage
        );
        assert_eq!(texts(&queues), ["! make"]);
        assert_eq!(
            queues.edit(agent(), id + 1, "x".to_string()),
            EditOutcome::Gone
        );
    }

    #[test]
    fn pop_front_takes_the_oldest_and_empties_the_map() {
        let mut queues = queue_of(&["a", "b"]);
        assert!(
            matches!(queues.pop_front(agent()), Some(Work::Prompt { prompt, .. }) if prompt.text == "a")
        );
        assert!(
            matches!(queues.pop_front(agent()), Some(Work::Prompt { prompt, .. }) if prompt.text == "b")
        );
        assert!(queues.pop_front(agent()).is_none());
        assert!(queues.is_empty());
    }
}
