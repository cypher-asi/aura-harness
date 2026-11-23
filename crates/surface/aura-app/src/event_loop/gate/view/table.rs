//! Gate's table for one level: for each category, Gate's pick now and
//! its pick for easy work, then every other model with outcomes there,
//! each with how many tries it had, how many succeeded and what its work
//! cost on average.

use super::{money, percent, GateView};
use aura_core_types::{Difficulty, PromptCategory, RouteLevel, RouteReason};
use aura_gate::route::stats::Row;
use aura_gate::route::{by_expected_cost, Candidate, Work};
use aura_model::model_catalog::route_label;
use std::fmt::Write;

/// One model's line under a category.
struct Line<'a> {
    model: String,
    row: Option<&'a Row>,
    /// What Gate picks the model for and why, e.g. `pick` and `proven`
    pick: Option<(&'static str, RouteReason)>,
}

impl GateView<'_> {
    /// The table for `level`. Turns list every category; subtasks only
    /// those with outcomes, until there are any.
    pub(super) fn render_level(&self, out: &mut String, level: RouteLevel) {
        let order = by_expected_cost(self.stats, level, self.candidates);
        let categories: Vec<(PromptCategory, Vec<Line<'_>>)> = PromptCategory::CLASSES
            .into_iter()
            .map(|category| (category, self.lines(level, category, &order)))
            .filter(|(_, lines)| {
                level == RouteLevel::Turn || lines.iter().any(|line| line.row.is_some())
            })
            .collect();
        if categories.is_empty() {
            return;
        }
        let width = categories
            .iter()
            .flat_map(|(_, lines)| lines.iter().map(|line| line.model.chars().count()))
            .max()
            .unwrap_or(0);
        let title = match level {
            RouteLevel::Turn => "Turns",
            RouteLevel::Subtask => "Subtasks",
        };
        let _ = write!(
            out,
            "\n\n{title}\n  {:<20}{:<width$}  {:>5}  {:>7}  {:>9}",
            "category", "  model", "tries", "success", "avg cost"
        );
        for (category, lines) in categories {
            let mut name = category.as_str();
            for line in lines {
                line.render(out, name, width);
                name = "";
            }
        }
    }

    /// Lines for `category` at `level`: Gate's pick now, then its pick for
    /// easy work when that is another model, then every other model with
    /// outcomes there, cheapest first as `order` has the candidates.
    fn lines(
        &self,
        level: RouteLevel,
        category: PromptCategory,
        order: &[&Candidate],
    ) -> Vec<Line<'_>> {
        let work = Work::new(level, category, None);
        let picked = self.pick(&work);
        let easy = self
            .pick(&work.of(Some(Difficulty::Easy)))
            .filter(|easy| picked.as_ref().is_none_or(|p| p.model.id != easy.model.id));
        let picks = [(picked, "pick"), (easy, "pick for easy work")];
        let shown = |model: &str| {
            picks
                .iter()
                .any(|(decision, _)| decision.as_ref().is_some_and(|d| d.model.id == model))
        };
        let mut tried: Vec<(Option<usize>, &str, &Row)> = self
            .stats
            .rows()
            .into_iter()
            .filter(|(key, _)| key.level == level && key.category == category)
            .filter(|(key, _)| !shown(&key.model))
            .map(|(key, row)| {
                let rank = order.iter().position(|c| c.id == key.model);
                (rank, key.model.as_str(), row)
            })
            .collect();
        // Models that can't run now (signed out, dropped) come last.
        tried.sort_unstable_by_key(|&(rank, model, _)| (rank.is_none(), rank, model));
        let picked = picks.into_iter().filter_map(|(decision, what)| {
            let decision = decision?;
            Some(Line {
                model: decision.model.route_label(),
                row: self.stats.row(level, category, &decision.model.id),
                pick: Some((what, decision.reason)),
            })
        });
        let tried = tried.into_iter().map(|(_, model, row)| Line {
            model: route_label(model),
            row: Some(row),
            pick: None,
        });
        picked.chain(tried).collect()
    }
}

impl Line<'_> {
    /// Write the line under `name`, the category's on its first line and
    /// blank after that, with the model padded to `width`.
    fn render(&self, out: &mut String, name: &str, width: usize) {
        let [tries, success, cost] = self.row.map_or_else(
            || ["-", "-", "-"].map(str::to_string),
            |row| {
                [
                    row.tries.to_string(),
                    row.success_rate().map_or_else(String::new, percent),
                    row.mean_cost_usd().map_or_else(String::new, money),
                ]
            },
        );
        let marker = if self.pick.is_some() { "→ " } else { "  " };
        let _ = write!(
            out,
            "\n  {name:<20}{marker}{:<width$}  {tries:>5}  {success:>7}  {cost:>9}",
            self.model
        );
        if let Some((what, reason)) = self.pick {
            let _ = write!(out, "  {what} ({reason})");
        }
    }
}
