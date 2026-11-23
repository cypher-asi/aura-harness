//! `/gate`: what Gate has learned per level, category and model, the
//! model it picks for each category now, how other ways of routing turns
//! would have done on the recorded ones, how much of the active agent's
//! work went to subtasks, and Gate's latest decisions.
//!
//! The view is drawn with Gate's table locked, so routing and the record
//! observers wait while it is drawn; the records it reads for the
//! delegation figures are read before that.
//!
//! - `table`: per level and category, Gate's picks and what each model did
//! - `replay`: how the recorded turns went, and how other ways of routing
//!   them would have done
//! - `delegation`: how much of the active agent's work went to subtasks,
//!   read from the records
//! - `recent`: Gate's latest decisions

mod delegation;
mod recent;
mod replay;
mod table;
#[cfg(test)]
mod tests;

pub(crate) use delegation::delegation_of;

use crate::event_loop::handlers::show_system_message;
use crate::event_loop::LoopState;
use aura_config::GateConfig;
use aura_core_types::RouteLevel;
use aura_gate::lead::delegation::Delegation;
use aura_gate::live::RecentRoute;
use aura_gate::route::stats::GateStats;
use aura_gate::route::{Candidate, Decision, Work};
use std::fmt::Write;

/// Show `/gate` in the active agent's chat.
pub(crate) async fn show(state: &LoopState<'_>) {
    let live = &state.hub.gate;
    let candidates = state.models.gate_candidates();
    let recent = live.recent();
    let pinned = (!state.models.is_gate()).then(|| state.models.current());
    let delegation = delegation_of(&state.hub, state.active);
    let text = live.with_stats(|stats| {
        GateView {
            stats,
            candidates: &candidates,
            settings: live.settings(),
            recent: &recent,
            pinned,
            delegation: &delegation,
        }
        .render()
    });
    show_system_message(state, text).await;
}

/// Everything `/gate` shows.
struct GateView<'a> {
    stats: &'a GateStats,
    candidates: &'a [Candidate],
    settings: &'a GateConfig,
    /// Oldest first
    recent: &'a [RecentRoute],
    /// The model the user pinned, when Gate isn't routing their turns
    pinned: Option<&'a str>,
    /// How much of the active agent's work went to subtasks
    delegation: &'a Delegation,
}

impl GateView<'_> {
    fn render(&self) -> String {
        let mut out = format!(
            "Gate: a model becomes a category's pick after {} counted outcomes at {} \
             success or better; {} of routes try the next cheaper model.",
            self.settings.min_tries,
            percent(self.settings.target),
            percent(self.settings.explore_rate),
        );
        if let Some(model) = self.pinned {
            let _ = write!(
                out,
                "\nYour turns run on {model}, which you chose; pick gate in /model to let \
                 Gate route them."
            );
        }
        if self.candidates.is_empty() {
            out.push_str("\nNothing to route to yet: /login, link a plan, or start a local model.");
        }
        for level in [RouteLevel::Turn, RouteLevel::Subtask] {
            self.render_level(&mut out, level);
        }
        self.render_replay(&mut out);
        self.render_delegation(&mut out);
        self.render_recent(&mut out);
        out
    }

    /// Gate's pick for `work` now. It never explores, so it takes no
    /// draw and depends on nothing but the work.
    fn pick(&self, work: &Work<'_>) -> Option<Decision> {
        aura_gate::route::pick(
            self.stats,
            work,
            self.candidates,
            &self.settings.without_exploring(),
            &mut || 1.0,
        )
    }
}

/// `share` as a whole percentage, e.g. `67%`.
fn percent(share: f64) -> String {
    format!("{:.0}%", share * 100.0)
}

/// `$0.004`, or `free` for models that cost nothing.
fn money(usd: f64) -> String {
    if usd <= 0.0 {
        "free".to_string()
    } else if usd < 0.01 {
        format!("${usd:.4}")
    } else {
        format!("${usd:.3}")
    }
}
