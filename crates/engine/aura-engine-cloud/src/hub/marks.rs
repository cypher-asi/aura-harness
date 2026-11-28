//! What each placement keeps for the next hub, so one started after a
//! crash or a quit takes up where this one left off: the turn its machine
//! was asked for and no hub saw end, and whether everything the machine
//! made was copied here before it went to sleep.

use aura_core_protocol::RuntimeRequest;
use aura_core_types::time::now_ms;
use aura_core_types::AgentId;
use serde::{Deserialize, Serialize};

use super::CloudHub;

/// Longest label kept, in characters.
const LABEL_CHARS: usize = 120;
/// How much later than a hub's sleep boxd may say a client last
/// connected to the machine, for clocks that differ, before the machine
/// counts as reached since.
const CLOCK_SLACK_MS: u64 = 30_000;

/// A turn a hub asked a node for and has not seen end, kept on its
/// agent's placement: the next hub follows it to its end
/// ([`CloudHub::follow_turn`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnMark {
    /// The node's chat run that carries it.
    pub run_id: String,
    /// When it was asked for, Unix milliseconds.
    pub started_ms: u64,
    /// The head of the agent's record here when it was asked for: what
    /// the turn wrote comes after it.
    pub head: u64,
    /// Its prompt's first line.
    pub label: String,
    /// The model it asked for; `None` for the node's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Whether its machine titles the chat from its prompt.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub titled: bool,
}

impl TurnMark {
    /// The mark of `prompt`, asked for now as `request` on run `run_id`
    /// with the record here at `head`.
    pub(super) fn new(run_id: &str, head: u64, prompt: &str, request: &RuntimeRequest) -> Self {
        let line = prompt
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("Turn");
        Self {
            run_id: run_id.to_string(),
            started_ms: now_ms(),
            head,
            label: line.chars().take(LABEL_CHARS).collect(),
            model: request.model.id.clone(),
            titled: request.model.title_model.is_some(),
        }
    }
}

/// Whether a machine a hub put to sleep at `slept_ms` with everything
/// copied stayed as it was, boxd having last seen a client connect to it
/// at `last_connected_ms`: whatever reaches a sleeping machine wakes it.
pub(super) fn slept_unchanged(slept_ms: Option<u64>, last_connected_ms: u64) -> bool {
    slept_ms.is_some_and(|slept| last_connected_ms <= slept.saturating_add(CLOCK_SLACK_MS))
}

impl CloudHub {
    /// Keep `mark` as `agent`'s turn under way.
    pub(super) fn mark_turn(&self, agent: AgentId, mark: TurnMark) {
        let marked = self
            .placements
            .lock()
            .agents
            .get_mut(&agent.to_hex())
            .map(|placed| placed.turn = Some(mark))
            .is_some();
        if marked {
            self.save_placements();
        }
    }

    /// `agent`'s turn on run `run_id` was seen to end. The mark of a turn
    /// asked for since stays.
    pub(super) fn end_turn_mark(&self, agent: AgentId, run_id: &str) {
        let cleared = self
            .placements
            .lock()
            .agents
            .get_mut(&agent.to_hex())
            .filter(|placed| {
                placed
                    .turn
                    .as_ref()
                    .is_some_and(|mark| mark.run_id == run_id)
            })
            .map(|placed| placed.turn = None)
            .is_some();
        if cleared {
            self.save_placements();
        }
    }

    /// Whether `hex` has a turn marked under way: one no hub saw end,
    /// which may still run on its machine.
    pub(super) fn turn_marked(&self, hex: &str) -> bool {
        self.placements
            .lock()
            .agents
            .get(hex)
            .is_some_and(|placed| placed.turn.is_some())
    }

    /// The turns an earlier hub asked machines for and never saw end, by
    /// agent: each still runs on its machine or ended while no hub
    /// watched. Follow each with [`Self::follow_turn`].
    #[must_use]
    pub fn unfinished_turns(&self) -> Vec<(AgentId, TurnMark)> {
        let streaming = self.streaming.lock().clone();
        self.placements
            .lock()
            .agents
            .iter()
            .filter(|(hex, _)| !streaming.contains_key(*hex))
            .filter_map(|(hex, placed)| Some((AgentId::from_hex(hex).ok()?, placed.turn.clone()?)))
            .collect()
    }

    /// `hex`'s machine went to sleep; `copied` says whether everything on
    /// it was copied here first.
    pub(super) fn note_slept(&self, hex: &str, copied: bool) {
        let slept_ms = copied.then(now_ms);
        let changed = self
            .placements
            .lock()
            .agents
            .get_mut(hex)
            .filter(|placed| placed.slept_ms != slept_ms)
            .map(|placed| placed.slept_ms = slept_ms)
            .is_some();
        if changed {
            self.save_placements();
        }
    }

    /// `hex`'s machine is awake: what it makes from now on is not copied
    /// yet.
    pub(super) fn note_awake(&self, hex: &str) {
        let changed = self
            .placements
            .lock()
            .agents
            .get_mut(hex)
            .and_then(|placed| placed.slept_ms.take())
            .is_some();
        if changed {
            self.save_placements();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::request::{chat_request, TurnAsk};
    use super::*;

    #[test]
    fn a_machine_stays_as_it_slept_until_something_connects() {
        assert!(!slept_unchanged(None, 0), "slept with nothing copied");
        assert!(slept_unchanged(Some(1_000_000), 0));
        assert!(slept_unchanged(Some(1_000_000), 1_000_000 + CLOCK_SLACK_MS));
        assert!(!slept_unchanged(
            Some(1_000_000),
            1_000_001 + CLOCK_SLACK_MS
        ));
    }

    #[test]
    fn a_mark_keeps_the_first_line_and_what_was_asked() {
        let agent = AgentId::new([1; 32]);
        let ask = TurnAsk {
            model: Some("claude/opus".into()),
            title_model: Some("claude/haiku".into()),
            ..TurnAsk::default()
        };
        let request = chat_request(agent, &ask);
        let mark = TurnMark::new("run-1", 7, "\n  fix the build  \nthen test", &request);
        assert_eq!(mark.label, "fix the build");
        assert_eq!(mark.head, 7);
        assert_eq!(mark.model.as_deref(), Some("claude/opus"));
        assert!(mark.titled);
        let long = "x".repeat(LABEL_CHARS + 10);
        let plain = TurnMark::new("run-2", 0, &long, &chat_request(agent, &TurnAsk::default()));
        assert_eq!(plain.label.chars().count(), LABEL_CHARS);
        assert!(!plain.titled && plain.model.is_none());
    }

    #[test]
    fn placements_from_before_marks_still_load() {
        let old = r#"{"zns":"0://a","name":"a","project":"p","project_dir":"/p","machine":"m"}"#;
        let placed: super::super::PlacedAgent = serde_json::from_str(old).expect("old placement");
        assert!(placed.turn.is_none() && placed.slept_ms.is_none());
        let saved = serde_json::to_string(&placed).expect("save");
        assert!(!saved.contains("turn") && !saved.contains("slept_ms"));
    }
}
