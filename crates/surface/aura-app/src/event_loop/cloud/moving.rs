//! The mark an agent carries while it moves between machines.

use super::*;

/// Holds `agent` as moving for as long as it lives: its prompts are
/// refused, the local kernel leaves its record alone and its footer
/// switch shows the move. Taking and dropping it wake the HOST publisher,
/// so the switch changes as the move starts and as it ends, however it
/// ends.
pub(super) struct MoveMark {
    shared: Arc<Shared>,
    agent: AgentId,
}

impl MoveMark {
    pub(super) fn take(shared: &Arc<Shared>, agent: AgentId) -> Self {
        shared.moving.lock().insert(agent);
        shared.publish_now();
        Self {
            shared: Arc::clone(shared),
            agent,
        }
    }
}

impl Drop for MoveMark {
    fn drop(&mut self) {
        self.shared.moving.lock().remove(&self.agent);
        self.shared.publish_now();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A wake waits for a publisher that isn't listening yet, so this
    /// returns at once when one was sent.
    async fn woken(shared: &Shared) -> bool {
        tokio::time::timeout(Duration::from_secs(1), shared.changed.notified())
            .await
            .is_ok()
    }

    #[tokio::test]
    async fn a_mark_holds_the_move_and_wakes_the_publisher_as_it_starts_and_ends() {
        let shared = Arc::new(Shared::default());
        let agent = AgentId::new([3; 32]);
        let mark = MoveMark::take(&shared, agent);
        assert!(shared.moving.lock().contains(&agent));
        assert!(woken(&shared).await, "the switch shows the move");
        drop(mark);
        assert!(!shared.moving.lock().contains(&agent));
        assert!(woken(&shared).await, "and where the agent is after it");
    }
}
