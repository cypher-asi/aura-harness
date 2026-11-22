//! Reading the table: the rows Gate routes by, and the counted prompts
//! a replay weighs.

use aura_core_types::{Difficulty, PromptCategory, RouteLevel};

use super::{GateStats, Row, RowKey};
use crate::replay::Logged;

impl GateStats {
    /// The row of `model` on `category` at `level`, if it has turns.
    #[must_use]
    pub fn row(&self, level: RouteLevel, category: PromptCategory, model: &str) -> Option<&Row> {
        self.table.get(level, category, model).map(|rows| &rows.all)
    }

    /// The row of `model` on `category` for subtasks that went to the
    /// permanent agent `kind`, if it has turns.
    pub(crate) fn kind_row(
        &self,
        category: PromptCategory,
        kind: &str,
        model: &str,
    ) -> Option<&Row> {
        self.table
            .get(RouteLevel::Subtask, category, model)?
            .kinds
            .get(kind)
    }

    /// The row of `model` on turns asking for `category` that were led
    /// (`led`) or done by the model itself, if it has turns. Turns whose
    /// prompt does not say are in neither.
    #[must_use]
    pub fn way_row(&self, category: PromptCategory, model: &str, led: bool) -> Option<&Row> {
        self.table
            .get(RouteLevel::Turn, category, model)?
            .ways
            .get(&led)
    }

    /// [`Self::way_row`] over the turns the classifier judged of
    /// `difficulty` only.
    #[must_use]
    pub fn way_row_of(
        &self,
        category: PromptCategory,
        difficulty: Difficulty,
        model: &str,
        led: bool,
    ) -> Option<&Row> {
        self.table
            .get(RouteLevel::Turn, category, model)?
            .works
            .get(&(difficulty, Some(led)))
    }

    /// [`Self::unled_row`] over the turns the classifier judged of
    /// `difficulty` only. `None` when there are none.
    pub(crate) fn unled_work_row(
        &self,
        category: PromptCategory,
        difficulty: Difficulty,
        model: &str,
    ) -> Option<Row> {
        let works = &self.table.get(RouteLevel::Turn, category, model)?.works;
        let row = [None, Some(false)]
            .into_iter()
            .filter_map(|led| works.get(&(difficulty, led)))
            .fold(Row::default(), |sum, row| sum.with(row));
        (row.turns > 0).then_some(row)
    }

    /// The row of `model` on turns asking for `category` that it did
    /// itself: every turn but those it led, whose outcome says as much
    /// about the subtasks as about the lead. `None` when there are none.
    pub(crate) fn unled_row(&self, category: PromptCategory, model: &str) -> Option<Row> {
        let rows = self.table.get(RouteLevel::Turn, category, model)?;
        let row = rows
            .ways
            .get(&true)
            .map_or(rows.all, |led| rows.all.without(led));
        (row.turns > 0).then_some(row)
    }

    /// Mean cost per turn of `model` at `level`, its subtasks' included,
    /// over every category; `None` before its first turn there.
    pub(crate) fn mean_cost_usd(&self, level: RouteLevel, model: &str) -> Option<f64> {
        self.table
            .of_model(level, model)
            .fold(Row::default(), |sum, rows| sum.with(&rows.all))
            .mean_total_cost_usd()
    }

    /// Every row with turns, by level, category and model.
    #[must_use]
    pub fn rows(&self) -> Vec<(&RowKey, &Row)> {
        self.table
            .all()
            .map(|rows| (&rows.key, &rows.all))
            .collect()
    }

    /// Every counted prompt, for [`crate::replay`].
    #[must_use]
    pub fn logged(&self) -> Vec<Logged> {
        self.prompts
            .values()
            .filter_map(|state| {
                let shape = state.shape.as_ref()?;
                let counted = state.counted.as_ref()?;
                let success = counted.success?;
                Some(Logged {
                    level: shape.level,
                    category: counted.category,
                    difficulty: shape.difficulty,
                    kind: shape.kind.clone(),
                    model: counted.model.clone(),
                    propensity: shape.propensity,
                    success,
                    cost_usd: counted.cost_usd + counted.subtask_cost_usd,
                })
            })
            .collect()
    }
}
