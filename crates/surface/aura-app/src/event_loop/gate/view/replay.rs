//! The replay: how the recorded turns went, and how Gate's picks now and
//! each model on its own would have done on them.

use super::{money, percent, GateView};
use aura_core_types::RouteLevel;
use aura_gate::route::replay::{self, Estimate, Logged};
use aura_gate::route::Work;
use aura_model::model_catalog::route_label;
use std::fmt::Write;

impl GateView<'_> {
    /// How the recorded turns went, and how Gate's picks now and each
    /// model on its own would have done on them.
    pub(super) fn render_replay(&self, out: &mut String) {
        let logged: Vec<Logged> = self
            .stats
            .logged()
            .into_iter()
            .filter(|prompt| prompt.level == RouteLevel::Turn)
            .collect();
        if logged.is_empty() {
            return;
        }
        let policies = self.policies(&logged);
        let width = policies
            .iter()
            .map(|(name, _)| name.chars().count())
            .max()
            .unwrap_or(0);
        let _ = write!(
            out,
            "\n\nReplay of {} recorded turns, each weighted by how unlikely its route was; \
             a policy is judged on the turns it would have routed the same way\n  \
             {:<width$}  {:>7}  {:>7}  {:>9}",
            logged.len(),
            "policy",
            "matched",
            "success",
            "avg cost"
        );
        for (name, estimate) in policies {
            let [matched, success, cost] = estimate.map_or_else(
                || ["0", "-", "-"].map(str::to_string),
                |e| {
                    [
                        e.matched.to_string(),
                        percent(e.success_rate),
                        money(e.mean_cost_usd),
                    ]
                },
            );
            let _ = write!(
                out,
                "\n  {name:<width$}  {matched:>7}  {success:>7}  {cost:>9}"
            );
        }
    }

    /// Each way of routing the replay weighs, and how it would have done
    /// on `logged`: as recorded, Gate's picks now, then each model that did
    /// any of the work on its own, by id.
    fn policies(&self, logged: &[Logged]) -> Vec<(String, Option<Estimate>)> {
        let mut models: Vec<&str> = logged.iter().map(|p| p.model.as_str()).collect();
        models.sort_unstable();
        models.dedup();
        let mut policies = vec![
            ("as recorded".to_string(), replay::as_recorded(logged)),
            ("Gate's picks now".to_string(), self.gate_now(logged)),
        ];
        policies.extend(models.into_iter().map(|model| {
            (
                format!("all on {}", route_label(model)),
                replay::estimate(logged, |prompt| prompt.model == model),
            )
        }));
        policies
    }

    /// How Gate's picks now would have done on `logged`. Gate picks once
    /// for each piece of work there, as its pick depends on nothing else.
    fn gate_now(&self, logged: &[Logged]) -> Option<Estimate> {
        let mut picks: Vec<(Work<'_>, Option<String>)> = Vec::new();
        for prompt in logged {
            let work = prompt.work();
            if !picks.iter().any(|(seen, _)| *seen == work) {
                picks.push((work, self.pick(&work).map(|decision| decision.model.id)));
            }
        }
        replay::estimate(logged, |prompt| {
            let work = prompt.work();
            picks.iter().any(|(seen, model)| {
                *seen == work && model.as_deref() == Some(prompt.model.as_str())
            })
        })
    }
}
