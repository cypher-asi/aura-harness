//! Gate's part in a `task` spawn: when the user selected gate and nothing
//! pins the subtask's model, the subtask's prompt is classified and Gate
//! picks the model from its subtask rows. The child records the prompt at
//! level `subtask` with its category, route and kind, and the `Outcome`
//! written when the subtask ends. A prompt the classifier could not label
//! fails the subtask.
//!
//! The model stays pinned for a permanent agent that names its own model,
//! a council member, a model given in the `task` call, and every subtask
//! when the user pinned the main model. A pinned subtask's prompt is
//! recorded with its route only: Gate neither classifies it nor records
//! how it went.
//!
//! A subtask dispatched by an agent working on a frontier model is first
//! routed below that tier ([`aura_gate::route::below_lead`]), so the most
//! expensive model plans and a cheaper one does the work.
//!
//! The work of a subtask Gate routed that nothing checked ends with the
//! verdict of Gate's judge, and work the judge finds unfinished fails the
//! attempt. A subtask Gate routed whose attempt failed is retried once on
//! the next stronger model, the lead's tier included; the retry is
//! recorded with route reason `escalate`, which Gate's table does not
//! count. Gate's judge reviews the failed attempt first (see
//! [`aura_gate::judge::turn`]): a failure it does not put down to the
//! model is not retried and counts for nothing.
//!
//! Under the by-difficulty arm of `/compare` a subtask nothing pins is
//! classified as Gate's are, then runs on the model the arm gives its
//! difficulty, with no exploring and no retry.
//!
//! - `route`: where the subtask goes, and the label its prompt gets
//! - `label`: what the subtask's prompt is recorded with ([`SubtaskLabel`])
//! - `end`: what Gate judges and records as an attempt ends

mod end;
mod label;
mod route;

pub(crate) use end::{Judgement, SubtaskEnd};
pub(crate) use label::SubtaskLabel;

use crate::model_selection::{ModelLimits, ModelSelection};
use aura_core_types::{PromptCategory, SubagentDispatchRequest};
use aura_gate::compare::DifficultyRoutes;
use aura_gate::live::GateLive;
use aura_gate::route::{Candidate, Decision};
use aura_model::model_catalog::GATE_MODEL_ID;
use std::sync::Arc;

/// What Gate does with the subtasks turns delegate, as the selected
/// model stands.
#[derive(Debug, Clone, Default)]
pub struct GateBase {
    /// Model that classifies subtask prompts; empty when none can, which
    /// fails the subtasks Gate would route.
    pub classifier: String,
    /// What Gate routes subtasks among when the user selected gate;
    /// `None` when they pinned the main model.
    pub candidates: Option<Vec<Candidate>>,
}

impl GateBase {
    /// Gate's part in subtasks under `models`.
    #[must_use]
    pub fn of(models: &ModelSelection) -> Self {
        Self {
            classifier: models.classifier_model(),
            candidates: models.is_gate().then(|| models.gate_candidates()),
        }
    }
}

/// The model a `task` call names, unless it names Gate itself.
pub(crate) fn explicit_model(request: &SubagentDispatchRequest) -> Option<&str> {
    request
        .model_override
        .as_deref()
        .map(str::trim)
        .filter(|model| !model.is_empty() && *model != GATE_MODEL_ID)
}

/// How one subtask is classified and routed, and judged as it ends.
pub(crate) struct SubtaskGate {
    /// The permanent agent the `task` call named
    pub kind: String,
    pub classifier: String,
    /// What Gate picks the subtask's model among; `None` when it is pinned
    pub candidates: Option<Vec<Candidate>>,
    /// The model Gate sent the dispatching agent's work to, if it did
    pub lead_model: Option<String>,
    pub limits: ModelLimits,
    pub live: Arc<GateLive>,
    /// Whether Gate may explore; off under the Gate arm of `/compare`
    pub explore: bool,
    /// Set for the retry of a failed attempt: where it goes, and the
    /// first attempt's category
    pub escalation: Option<(Decision, PromptCategory)>,
    /// Set under the by-difficulty arm of `/compare`: the model each
    /// difficulty goes to, instead of Gate's pick
    pub table: Option<DifficultyRoutes>,
}
