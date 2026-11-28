//! Destroying machines: one agent's, and those no agent owns.

use crate::error::CloudResult;
use crate::names::{self, LABEL_AGENT, LABEL_ROLE, ROLE_AGENT};
use aura_core_types::{VmEvent, VmPayload};
use std::collections::HashSet;

use super::{VmFleet, VmState};

impl VmFleet {
    /// Destroy `agent`'s machine for `reason` and write its `destroyed`
    /// row. Returns the machine id, for [`Self::settle_usage`].
    ///
    /// # Errors
    ///
    /// When boxd refuses the destroy (the machine is kept and listed).
    pub async fn destroy(&self, agent: &str, reason: &str) -> CloudResult<Option<String>> {
        let Some((machine, ledger, usage)) = self.machines.lock().get_mut(agent).map(|entry| {
            entry.state = VmState::Destroying;
            (
                entry.machine.clone(),
                entry.ledger.clone(),
                entry.usage.clone(),
            )
        }) else {
            return Ok(None);
        };
        if let Err(err) = self.provider.destroy(&machine.name).await {
            self.set_state(agent, VmState::Error(err.to_string()));
            return Err(err);
        }
        self.machines.lock().remove(agent);
        self.row(
            &ledger,
            &VmPayload {
                reason: Some(reason.to_string()),
                usage: usage.as_ref().map(|u| u.usage),
                currency: usage.map(|u| u.currency),
                ..Self::base_row(VmEvent::Destroyed, &machine)
            },
        );
        Ok(Some(machine.id))
    }

    /// The install this fleet's machines are labeled with.
    #[cfg(test)]
    pub(crate) fn install(&self) -> &str {
        &self.settings.install
    }

    /// The name of a machine of this install boxd lists for `agent`, if
    /// any: one already made for it.
    ///
    /// # Errors
    ///
    /// When the machines can't be listed.
    pub async fn machine_listed_for(&self, agent: &str) -> CloudResult<Option<String>> {
        let selector = names::install_selector(&self.settings.install);
        let machines = self.provider.list(&[selector]).await?;
        Ok(machines
            .into_iter()
            .find(|machine| {
                machine.labels.get(LABEL_ROLE).map(String::as_str) == Some(ROLE_AGENT)
                    && machine.labels.get(LABEL_AGENT).map(String::as_str) == Some(agent)
            })
            .map(|machine| machine.name))
    }

    /// Destroy machines of this install whose agent is not in `keep`
    /// (archived, removed or unknown agents). Returns their names.
    ///
    /// # Errors
    ///
    /// When the machines can't be listed.
    pub async fn sweep_orphans(&self, keep: &HashSet<String>) -> CloudResult<Vec<String>> {
        let selector = names::install_selector(&self.settings.install);
        let machines = self.provider.list(&[selector]).await?;
        let mut destroyed = Vec::new();
        for machine in machines {
            if machine.labels.get(LABEL_ROLE).map(String::as_str) != Some(ROLE_AGENT) {
                // Builders delete themselves (delete-after).
                continue;
            }
            let agent = machine.labels.get(LABEL_AGENT).cloned().unwrap_or_default();
            if keep.contains(&agent) {
                continue;
            }
            match self.provider.destroy(&machine.name).await {
                Ok(()) => {
                    tracing::info!(machine = %machine.name, agent = %agent, "destroyed an orphaned cloud machine");
                    destroyed.push(machine.name);
                }
                Err(err) => {
                    tracing::warn!(machine = %machine.name, error = %err, "could not destroy an orphaned machine");
                }
            }
        }
        Ok(destroyed)
    }
}
