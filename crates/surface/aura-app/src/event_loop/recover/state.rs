//! What the recovery holds, as plain state over times in milliseconds,
//! so every rule here is tested without a clock.
//!
//! # Invariants
//!
//! - An agent is held at most once: for a sign-in, a turn retry, or a
//!   cloud start retry ([`Hold`]).
//! - Every retry is capped ([`MAX_TURN_RETRIES`], [`MAX_START_RETRIES`],
//!   [`MAX_BLOCKED_START_RETRIES`]); past the cap the caller fails the
//!   agent as before.
//! - A sign-in pause ends only once it is cleared (a model call worked,
//!   or the user signed in) and its cooldown is over. The cooldown is
//!   0 for an agent's first pause, then 2 minutes doubling up to 30 for
//!   each pause straight after a resume, so a token that stays revoked
//!   costs one call per cooldown.

use aura_core_types::AgentId;
use std::collections::HashMap;
use std::sync::Mutex;

/// Most automatic retries of turns that failed on transient trouble while
/// the agent owes a report, before it fails.
pub(crate) const MAX_TURN_RETRIES: u32 = 3;
/// Most automatic retries of a cloud start that failed for its machine.
pub(crate) const MAX_START_RETRIES: u32 = 3;
/// Most automatic retries of a cloud start its project failed (it didn't
/// build), one every [`BLOCKED_RETRY_MS`].
pub(crate) const MAX_BLOCKED_START_RETRIES: u32 = 6;

const MINUTE_MS: u64 = 60_000;
/// Between retries of a start its project failed: a fix has to land on
/// the merge target first.
pub(crate) const BLOCKED_RETRY_MS: u64 = 10 * MINUTE_MS;
/// The first cooldown of a sign-in pause straight after a resume: about
/// how long a cloud machine takes to pick up a new sign-in.
const SIGN_IN_COOLDOWN_MS: u64 = 2 * MINUTE_MS;
const MAX_SIGN_IN_COOLDOWN_MS: u64 = 30 * MINUTE_MS;

/// Why an agent is held, and when it goes on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Hold {
    /// The model refused its sign-in: it waits for one to work again
    SignIn {
        why: String,
        /// Not resumed before this time
        not_before: u64,
        /// The pause's cooldown, which the next pause doubles
        cooldown: u64,
        /// A model call worked, or the user signed in, since it paused
        cleared: bool,
    },
    /// Its turn failed on transient trouble; retry `attempt` is due at `due`
    Turn { why: String, attempt: u32, due: u64 },
    /// Its cloud start failed; retry `attempt` is due at `due`
    Start {
        why: String,
        attempt: u32,
        due: u64,
        /// Its project failed the start, not its machine
        blocked: bool,
    },
}

impl Hold {
    /// Whether it goes on at `now`.
    fn is_due(&self, now: u64) -> bool {
        match self {
            Self::SignIn {
                not_before,
                cleared,
                ..
            } => *cleared && now >= *not_before,
            Self::Turn { due, .. } | Self::Start { due, .. } => now >= *due,
        }
    }
}

/// The agents held for recovery and what their retries have used.
#[derive(Debug, Default)]
pub(crate) struct Recovery {
    state: Mutex<State>,
}

#[derive(Debug, Default)]
struct State {
    held: HashMap<AgentId, Hold>,
    turn_retries: HashMap<AgentId, u32>,
    start_retries: HashMap<AgentId, u32>,
    /// The cooldown of each agent's last sign-in pause, while no model
    /// call of its own worked since it resumed
    resumed: HashMap<AgentId, u64>,
}

/// `MINUTE_MS` doubled for each retry after the first: 1, 2, 4 minutes.
fn backoff(attempt: u32) -> u64 {
    MINUTE_MS << attempt.saturating_sub(1).min(8)
}

impl Recovery {
    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Hold `agent`, whose model refused its sign-in for `why`, at `now`,
    /// until a sign-in works again.
    pub(crate) fn pause_for_sign_in(&self, agent: AgentId, why: &str, now: u64) {
        let mut state = self.state();
        let cooldown = match state.resumed.get(&agent) {
            None => 0,
            Some(0) => SIGN_IN_COOLDOWN_MS,
            Some(last) => last.saturating_mul(2).min(MAX_SIGN_IN_COOLDOWN_MS),
        };
        state.held.insert(
            agent,
            Hold::SignIn {
                why: why.to_string(),
                not_before: now.saturating_add(cooldown),
                cooldown,
                cleared: false,
            },
        );
    }

    /// Hold `agent`, whose turn failed on transient trouble `why`, for a
    /// retry. The attempt it is and the wait before it, or `None` past
    /// [`MAX_TURN_RETRIES`]: then it fails, and so does its next such turn,
    /// until one of its turns works ([`Self::worked`]).
    pub(crate) fn hold_turn(&self, agent: AgentId, why: &str, now: u64) -> Option<(u32, u64)> {
        let mut state = self.state();
        let used = state.turn_retries.get(&agent).copied().unwrap_or(0);
        if used >= MAX_TURN_RETRIES {
            return None;
        }
        let attempt = used + 1;
        let wait = backoff(attempt);
        state.turn_retries.insert(agent, attempt);
        let due = now.saturating_add(wait);
        state.held.insert(
            agent,
            Hold::Turn {
                why: why.to_string(),
                attempt,
                due,
            },
        );
        Some((attempt, wait))
    }

    /// Hold `agent`, whose cloud start failed because of `why` (its
    /// project's doing when `blocked`), for a retry. The attempt and the
    /// wait before it, or `None` past the cap: then it fails, and so does
    /// each start after, as before, until one works ([`Self::started`]).
    pub(crate) fn hold_start(
        &self,
        agent: AgentId,
        why: &str,
        blocked: bool,
        now: u64,
    ) -> Option<(u32, u64)> {
        let mut state = self.state();
        let cap = if blocked {
            MAX_BLOCKED_START_RETRIES
        } else {
            MAX_START_RETRIES
        };
        let used = state.start_retries.get(&agent).copied().unwrap_or(0);
        if used >= cap {
            return None;
        }
        let attempt = used + 1;
        let wait = if blocked {
            BLOCKED_RETRY_MS
        } else {
            backoff(attempt)
        };
        state.start_retries.insert(agent, attempt);
        state.held.insert(
            agent,
            Hold::Start {
                why: why.to_string(),
                attempt,
                due: now.saturating_add(wait),
                blocked,
            },
        );
        Some((attempt, wait))
    }

    /// A model call of `agent` worked: its sign-in works, and so does
    /// everyone's. Its retries start over, and every sign-in pause is
    /// cleared.
    pub(crate) fn worked(&self, agent: AgentId) {
        let mut state = self.state();
        state.turn_retries.remove(&agent);
        state.resumed.remove(&agent);
        clear_sign_ins(&mut state);
    }

    /// The user signed in: every sign-in pause is cleared.
    pub(crate) fn signed_in(&self) {
        clear_sign_ins(&mut self.state());
    }

    /// `agent`'s cloud start worked, or the user asked for one: its start
    /// retries start over.
    pub(crate) fn started(&self, agent: AgentId) {
        self.state().start_retries.remove(&agent);
    }

    /// Take the holds due at `now`, in no particular order. A sign-in
    /// pause taken leaves its cooldown for the next one.
    pub(crate) fn take_due(&self, now: u64) -> Vec<(AgentId, Hold)> {
        let mut state = self.state();
        let due: Vec<AgentId> = state
            .held
            .iter()
            .filter(|(_, hold)| hold.is_due(now))
            .map(|(agent, _)| *agent)
            .collect();
        let mut taken = Vec::with_capacity(due.len());
        for agent in due {
            if let Some(hold) = state.held.remove(&agent) {
                if let Hold::SignIn { cooldown, .. } = &hold {
                    state.resumed.insert(agent, *cooldown);
                }
                taken.push((agent, hold));
            }
        }
        taken
    }

    /// Let `agent` go at once, as when the user sends it work: whatever
    /// held it, if anything.
    pub(crate) fn release(&self, agent: AgentId) -> Option<Hold> {
        self.state().held.remove(&agent)
    }

    /// Whether `agent` is held.
    pub(crate) fn is_held(&self, agent: AgentId) -> bool {
        self.state().held.contains_key(&agent)
    }

    /// Why `agent` waits for a sign-in, while it does.
    pub(crate) fn sign_in_wait(&self, agent: AgentId) -> Option<String> {
        match self.state().held.get(&agent) {
            Some(Hold::SignIn { why, .. }) => Some(why.clone()),
            _ => None,
        }
    }

    /// Use up `agent`'s start retries, as failed starts would have: the
    /// tests of what happens past them.
    #[cfg(test)]
    pub(crate) fn exhaust_start_retries(&self, agent: AgentId) {
        self.state()
            .start_retries
            .insert(agent, MAX_BLOCKED_START_RETRIES.max(MAX_START_RETRIES));
    }

    /// The agents waiting for a sign-in.
    #[cfg(test)]
    pub(crate) fn waiting_for_sign_in(&self) -> Vec<AgentId> {
        let state = self.state();
        state
            .held
            .iter()
            .filter(|(_, hold)| matches!(hold, Hold::SignIn { .. }))
            .map(|(agent, _)| *agent)
            .collect()
    }
}

fn clear_sign_ins(state: &mut State) {
    for hold in state.held.values_mut() {
        if let Hold::SignIn { cleared, .. } = hold {
            *cleared = true;
        }
    }
}

#[cfg(test)]
mod tests;
