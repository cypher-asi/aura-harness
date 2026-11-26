//! A placed agent's machine: sleep, wake, exec, host stats and cost.

use std::sync::Arc;
use std::time::Duration;

use crate::cost::{latest_reading, CostEstimate, CostSample};
use crate::{BootReporter, CloudError, CloudResult};
use aura_core_protocol::cloud::HostStats;
use aura_core_types::{AgentId, VmPayload};

use super::{CloudHub, MAX_SAMPLES};

impl CloudHub {
    /// Put `agent`'s machine to sleep now (hibernate: disk-only billing).
    ///
    /// # Errors
    ///
    /// When it has no machine or boxd refuses.
    pub async fn sleep(&self, agent: AgentId) -> CloudResult<()> {
        let hex = agent.to_hex();
        let view = self.fleet.view(&hex).ok_or_else(|| CloudError::NotFound {
            what: "machine",
            name: hex.clone(),
        })?;
        if !view.state.is_asleep() {
            self.copy_before_sleep(agent).await;
        }
        if let Some(link) = self.links.lock().remove(&hex) {
            link.stop.cancel();
        }
        self.fleet.provider().hibernate(&view.machine.name).await?;
        self.fleet.poll().await;
        Ok(())
    }

    /// Wake `agent`'s machine and reopen its mesh socket, showing the
    /// wake on `boot`.
    ///
    /// # Errors
    ///
    /// When it has no machine or waking fails.
    pub async fn wake(self: &Arc<Self>, agent: AgentId, boot: &BootReporter) -> CloudResult<()> {
        let hex = agent.to_hex();
        self.fleet.ensure_awake_reported(&hex, boot).await?;
        self.open_link(&hex);
        Ok(())
    }

    /// Run a shell command in `agent`'s project folder on its machine.
    ///
    /// # Errors
    ///
    /// When the machine can't be woken or the call fails.
    pub async fn exec(&self, agent: AgentId, command: &str) -> CloudResult<crate::ExecOutput> {
        let hex = agent.to_hex();
        let view = self.fleet.ensure_awake(&hex).await?;
        let _guard = self.fleet.begin_work(&hex);
        self.fleet
            .provider()
            .exec(
                &view.machine.name,
                &format!("cd \"$AURA_WORKSPACE\" && {command}"),
                None,
                Duration::from_secs(600),
            )
            .await
    }

    /// The latest host sample of `agent`'s machine.
    #[must_use]
    pub fn host_stats(&self, agent: AgentId) -> Option<HostStats> {
        self.tracks
            .lock()
            .get(&agent.to_hex())
            .and_then(|t| t.latest.clone())
    }

    /// `agent`'s machine cost: boxd's meter plus an estimate since.
    #[must_use]
    pub fn cost(&self, agent: AgentId) -> Option<CostEstimate> {
        let hex = agent.to_hex();
        let view = self.fleet.view(&hex)?;
        let samples = self
            .tracks
            .lock()
            .get(&hex)
            .map(|t| t.samples.clone())
            .unwrap_or_default();
        let metered = view
            .usage
            .clone()
            .or_else(|| latest_reading(&self.ledger.rows(&view.ledger, 200)));
        Some(self.costs.estimate(metered.as_ref(), &samples))
    }

    /// The last `limit` rows of `agent`'s machine ledger.
    #[must_use]
    pub fn ledger_rows(&self, agent: AgentId, limit: usize) -> Vec<VmPayload> {
        self.fleet
            .view(&agent.to_hex())
            .map(|view| self.ledger.rows(&view.ledger, limit))
            .unwrap_or_default()
    }

    pub(super) fn track(&self, hex: &str, stats: HostStats) {
        let mut tracks = self.tracks.lock();
        let track = tracks.entry(hex.to_string()).or_default();
        track.samples.push(CostSample {
            at_ms: stats.sampled_ms,
            cpu_busy_secs: stats.cpu_busy_secs,
            mem_used_bytes: stats.mem_used_bytes,
            disk_used_bytes: stats.disk_used_bytes,
        });
        if track.samples.len() > MAX_SAMPLES {
            let excess = track.samples.len() - MAX_SAMPLES;
            track.samples.drain(..excess);
        }
        track.latest = Some(stats);
    }
}
