//! What placed agents have under way on their machines, for the
//! PROCESSES and MEMBERS tabs here: their turns, whoever started them,
//! and the agents they spawned there.

use std::collections::{HashMap, HashSet};

use crate::fleet::WorkGuard;
use aura_core_protocol::cloud::MeshProcess;
use aura_core_types::AgentId;

use super::CloudHub;

/// Most rows kept of what one machine says it has under way.
const MAX_ROWS: usize = 256;

/// What one link's node last said it has under way for its agent, and
/// the hold keeping its machine awake while any of it runs.
#[derive(Default)]
pub(super) struct MachineWork {
    rows: Vec<MeshProcess>,
    /// Turns this hub streamed: the app shows each as its own turn.
    streamed: HashSet<String>,
    awake: Option<WorkGuard>,
}

/// Held while this hub streams an agent's turn from a chat run on its
/// node.
pub(super) struct StreamedTurn<'a> {
    hub: &'a CloudHub,
    hex: String,
}

impl Drop for StreamedTurn<'_> {
    fn drop(&mut self) {
        self.hub.streaming.lock().remove(&self.hex);
    }
}

impl CloudHub {
    /// What each placed agent's machine has under way, by agent hex:
    /// all but the turns this hub streamed.
    #[must_use]
    pub fn machine_processes(&self) -> Vec<(String, Vec<MeshProcess>)> {
        self.work
            .lock()
            .iter()
            .map(|(hex, work)| {
                let rows: Vec<MeshProcess> = work
                    .rows
                    .iter()
                    .filter(|row| !work.streamed.contains(&row.id))
                    .cloned()
                    .collect();
                (hex.clone(), rows)
            })
            .filter(|(_, rows)| !rows.is_empty())
            .collect()
    }

    /// The agents placed agents spawned on their machines that are
    /// working there, as the machines last said.
    #[must_use]
    pub fn working_spawns(&self) -> HashSet<AgentId> {
        self.work
            .lock()
            .values()
            .flat_map(|work| &work.rows)
            .filter(|row| row.is_agent() && row.is_running())
            .filter_map(|row| AgentId::from_hex(&row.agent_id).ok())
            .collect()
    }

    /// `hex`'s turn is streamed from `run` on its node until the guard
    /// drops.
    pub(super) fn streaming_turn(&self, hex: &str, run: String) -> StreamedTurn<'_> {
        self.streaming.lock().insert(hex.to_string(), run);
        StreamedTurn {
            hub: self,
            hex: hex.to_string(),
        }
    }

    /// Keep what `hex`'s node says it has under way, holding the machine
    /// awake while any of it runs: mail and spawned agents start work
    /// that no turn from here holds the machine for.
    pub(super) fn note_processes(&self, hex: &str, rows: Vec<MeshProcess>) {
        let rows = self.own_processes(hex, rows);
        let running = rows.iter().any(MeshProcess::is_running)
            && self
                .fleet
                .view(hex)
                .is_some_and(|view| !view.state.is_asleep());
        let streaming = self.streaming.lock().get(hex).cloned();
        let mut work = self.work.lock();
        let entry = work.entry(hex.to_string()).or_default();
        entry
            .streamed
            .retain(|id| rows.iter().any(|row| row.id == *id));
        if let Some(run) = streaming {
            entry.streamed.extend(
                rows.iter()
                    .filter(|row| !row.is_agent() && row.run_id.as_deref() == Some(run.as_str()))
                    .map(|row| row.id.clone()),
            );
        }
        entry.rows = rows;
        if !running {
            entry.awake = None;
        } else if entry.awake.is_none() {
            entry.awake = Some(self.fleet.begin_work(hex));
        }
    }

    /// Forget what `hex`'s node said, once its socket is gone.
    pub(super) fn forget_processes(&self, hex: &str) {
        self.work.lock().remove(hex);
    }

    /// The rows of `rows` that are `hex`'s: its own turns, and agents
    /// under it that are new here or already copied as spawned by it. A
    /// node speaks for nobody else.
    fn own_processes(&self, hex: &str, rows: Vec<MeshProcess>) -> Vec<MeshProcess> {
        let children: HashSet<String> = self.children_of(hex).iter().map(AgentId::to_hex).collect();
        let keep: Vec<bool> = {
            let parents: HashMap<&str, &str> = rows
                .iter()
                .filter(|row| row.is_agent())
                .filter_map(|row| Some((row.agent_id.as_str(), row.parent.as_deref()?)))
                .collect();
            rows.iter()
                .map(|row| {
                    if !row.is_agent() {
                        return row.agent_id == hex;
                    }
                    reaches(&row.agent_id, hex, &parents)
                        && (children.contains(&row.agent_id) || self.is_new_here(&row.agent_id))
                })
                .collect()
        };
        rows.into_iter()
            .zip(keep)
            .filter_map(|(row, keep)| keep.then_some(row))
            .take(MAX_ROWS)
            .collect()
    }

    /// Whether `hex` names an agent this computer holds nothing of: not
    /// placed, and without a record.
    fn is_new_here(&self, hex: &str) -> bool {
        let Ok(agent) = AgentId::from_hex(hex) else {
            return false;
        };
        !self.placements.lock().agents.contains_key(hex)
            && self.store.get_head_seq(agent).is_ok_and(|head| head == 0)
    }
}

/// Whether following `parents` up from `agent` reaches `root`.
fn reaches(agent: &str, root: &str, parents: &HashMap<&str, &str>) -> bool {
    let mut at = agent;
    for _ in 0..=parents.len() {
        match parents.get(at) {
            Some(&parent) if parent == root => return true,
            Some(&parent) => at = parent,
            None => return false,
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_spawned_agent_belongs_to_the_root_its_parents_lead_to() {
        let parents: HashMap<&str, &str> =
            [("b", "a"), ("c", "b"), ("x", "y"), ("p", "q"), ("q", "p")]
                .into_iter()
                .collect();
        assert!(reaches("b", "a", &parents));
        assert!(reaches("c", "a", &parents));
        assert!(!reaches("x", "a", &parents), "another root");
        assert!(!reaches("a", "a", &parents), "the root is not under itself");
        assert!(!reaches("p", "a", &parents), "a loop ends");
    }
}
