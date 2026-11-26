//! Pausing, resuming, stopping and restarting agents, and removing subtrees.

use std::collections::HashMap;
use std::sync::Arc;

use aura_core_types::time::now_ms;
use aura_core_types::AgentId;
use tokio_util::sync::CancellationToken;
use tracing::info;

use crate::agents::inbox::MessageKind;
use crate::agents::mail::MailMessage;

use super::outcome::{cancelled, reply_text};
use super::{AgentEntry, Body, Envelope, HostInner, LifecycleAction};

impl HostInner {
    /// Cancel and remove `target`'s descendants, and `target` itself when
    /// `include_target`. Returns the queued messages that will never run.
    pub(super) fn remove_subtree(
        &self,
        agents: &mut HashMap<AgentId, AgentEntry>,
        target: AgentId,
        include_target: bool,
    ) -> Vec<Dropped> {
        let ids: Vec<AgentId> = agents
            .iter()
            .filter(|(id, e)| (include_target && **id == target) || e.chain.contains(&target))
            .map(|(id, _)| *id)
            .collect();
        let observer = self.observer.lock().clone();
        let mut dropped = Vec::new();
        for id in ids {
            let Some(entry) = agents.remove(&id) else {
                continue;
            };
            entry.stop.cancel();
            self.parent_locks.remove(&id);
            if let Body::Hosted { queue, .. } = entry.body {
                dropped.extend(queue.into_iter().map(|envelope| Dropped {
                    agent_id: id,
                    name: entry.name.clone(),
                    envelope,
                }));
                self.scheduler.identity_registry().unregister(id);
                if let Some(observer) = &observer {
                    observer.agent_removed(id);
                }
            }
        }
        dropped
    }

    /// Tell everyone waiting on a dropped message that it was cancelled,
    /// and answer each one an agent sent with a `was cancelled` reply in
    /// its thread. Call with no host lock held: the mail observer runs
    /// inline.
    pub(super) fn notify_dropped(&self, dropped: Vec<Dropped>) {
        for Dropped {
            agent_id,
            name,
            envelope,
        } in dropped
        {
            let result = cancelled(agent_id);
            if let Some(sender) = envelope.from {
                self.post_mail(MailMessage {
                    id: 0,
                    thread: envelope.thread,
                    subject: envelope.subject,
                    from: agent_id,
                    from_name: name,
                    to: sender,
                    to_name: envelope.from_name,
                    kind: MessageKind::Reply,
                    text: reply_text(&result),
                    sent_ms: now_ms(),
                    subject_from_text: false,
                });
            }
            if let Some(waiter) = envelope.waiter {
                let _ = waiter.send(result);
            }
        }
    }

    // ------------------------------------------------------------------
    // lifecycle
    // ------------------------------------------------------------------

    pub(in crate::agents) fn lifecycle(
        self: &Arc<Self>,
        target: AgentId,
        action: LifecycleAction,
    ) -> Result<(), String> {
        let dropped = {
            let mut agents = self.agents.lock();
            let entry = agents
                .get(&target)
                .ok_or_else(|| format!("unknown agent {target}"))?;
            if matches!(entry.body, Body::Root { .. }) {
                return Err("root agents are controlled by their surface".to_string());
            }
            let parent_stop = entry
                .parent()
                .and_then(|p| agents.get(&p))
                .map(|p| p.stop.clone());
            match action {
                LifecycleAction::Pause | LifecycleAction::Resume => {
                    let paused = action == LifecycleAction::Pause;
                    for (id, entry) in agents.iter_mut() {
                        if *id == target || entry.chain.contains(&target) {
                            entry.paused = paused;
                        }
                    }
                    Vec::new()
                }
                LifecycleAction::Stop => self.remove_subtree(&mut agents, target, true),
                LifecycleAction::Restart => {
                    let dropped = self.remove_subtree(&mut agents, target, false);
                    if let Some(entry) = agents.get_mut(&target) {
                        entry.paused = false;
                        entry.stop.cancel();
                        entry.stop = parent_stop
                            .as_ref()
                            .map_or_else(CancellationToken::new, CancellationToken::child_token);
                        if let Body::Hosted { history, .. } = &mut entry.body {
                            history.clear();
                        }
                    }
                    dropped
                }
            }
        };
        self.notify_dropped(dropped);
        info!(agent = %target, ?action, "agent lifecycle");
        if matches!(action, LifecycleAction::Resume | LifecycleAction::Restart) {
            let ids: Vec<AgentId> = self.agents.lock().keys().copied().collect();
            for id in ids {
                self.pump(id);
            }
        }
        Ok(())
    }

    /// True when `ancestor` spawned `agent`, directly or indirectly.
    pub(in crate::agents) fn is_descendant(&self, agent: AgentId, ancestor: AgentId) -> bool {
        self.agents
            .lock()
            .get(&agent)
            .is_some_and(|e| e.chain.contains(&ancestor))
    }
}

/// A queued message whose recipient was removed before handling it.
pub(super) struct Dropped {
    agent_id: AgentId,
    /// The recipient's display name.
    name: String,
    envelope: Envelope,
}
