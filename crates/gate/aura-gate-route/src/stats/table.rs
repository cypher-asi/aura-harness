//! Where the table keeps its rows: by level and category, then by model,
//! so that a row is found by a model's id without building a key.

use std::borrow::Borrow;
use std::collections::BTreeMap;

use aura_core_types::{Difficulty, PromptCategory, RouteLevel};

use super::observe::{Counted, Shape};
use super::Row;

/// Which row an outcome counts in.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RowKey {
    /// Turn or subtask.
    pub level: RouteLevel,
    /// What the prompt asked for.
    pub category: PromptCategory,
    /// Model that did the work.
    pub model: String,
}

/// The rows of every model, by level and category, then by model.
#[derive(Debug, Clone, Default)]
pub(super) struct Table(BTreeMap<(RouteLevel, PromptCategory), BTreeMap<String, ModelRows>>);

/// What one model has done on one category at one level: all of it, and
/// the parts of it Gate asks about.
#[derive(Debug, Clone)]
pub(super) struct ModelRows {
    /// Where the rows are, for [`super::GateStats::rows`] to lend out.
    pub(super) key: RowKey,
    /// All of the model's work there.
    pub(super) all: Row,
    /// The subtasks, by the permanent agent they went to.
    pub(super) kinds: BTreeMap<String, Row>,
    /// The turns whose prompt says whether they were led, by that.
    pub(super) ways: BTreeMap<bool, Row>,
    /// The work of known difficulty, by that and whether a turn was led
    /// (`None` where its prompt does not say, and for subtasks).
    pub(super) works: BTreeMap<(Difficulty, Option<bool>), Row>,
}

impl Table {
    /// The rows of `model` on `category` at `level`, if it has turns there.
    pub(super) fn get(
        &self,
        level: RouteLevel,
        category: PromptCategory,
        model: &str,
    ) -> Option<&ModelRows> {
        self.0.get(&(level, category))?.get(model)
    }

    /// The rows of `model` at `level`, one for each category it has turns on.
    pub(super) fn of_model<'a>(
        &'a self,
        level: RouteLevel,
        model: &'a str,
    ) -> impl Iterator<Item = &'a ModelRows> + 'a {
        self.0
            .iter()
            .filter(move |((at, _), _)| *at == level)
            .filter_map(move |(_, models)| models.get(model))
    }

    /// Every model's rows, by level, category and model.
    pub(super) fn all(&self) -> impl Iterator<Item = &ModelRows> {
        self.0.values().flat_map(BTreeMap::values)
    }

    /// Count `counted` in the rows its prompt's `shape` puts it in. An
    /// outcome that takes the earlier ones back counts in none.
    pub(super) fn add(&mut self, counted: &Counted, shape: &Shape) {
        if counted.void {
            return;
        }
        let key = (shape.level, counted.category);
        let rows = self
            .0
            .entry(key)
            .or_default()
            .entry(counted.model.clone())
            .or_insert_with(|| ModelRows::new(key, &counted.model));
        rows.all.add(counted);
        if let Some(kind) = &shape.kind {
            rows.kinds.entry(kind.clone()).or_default().add(counted);
        }
        if let Some(led) = shape.led {
            rows.ways.entry(led).or_default().add(counted);
        }
        if let Some(difficulty) = shape.difficulty {
            let at = (difficulty, shape.led);
            rows.works.entry(at).or_default().add(counted);
        }
    }

    /// Take `counted` back out of the rows [`Self::add`] counted it in,
    /// dropping each row it leaves with no turns.
    pub(super) fn remove(&mut self, counted: &Counted, shape: &Shape) {
        if counted.void {
            return;
        }
        let key = (shape.level, counted.category);
        let Some(models) = self.0.get_mut(&key) else {
            return;
        };
        let Some(rows) = models.get_mut(counted.model.as_str()) else {
            return;
        };
        rows.remove(counted, shape);
        if rows.all.turns == 0 {
            models.remove(counted.model.as_str());
        }
        if models.is_empty() {
            self.0.remove(&key);
        }
    }
}

impl ModelRows {
    fn new((level, category): (RouteLevel, PromptCategory), model: &str) -> Self {
        Self {
            key: RowKey {
                level,
                category,
                model: model.to_string(),
            },
            all: Row::default(),
            kinds: BTreeMap::new(),
            ways: BTreeMap::new(),
            works: BTreeMap::new(),
        }
    }

    fn remove(&mut self, counted: &Counted, shape: &Shape) {
        self.all.remove(counted);
        if let Some(kind) = &shape.kind {
            take_out(&mut self.kinds, kind.as_str(), counted);
        }
        if let Some(led) = shape.led {
            take_out(&mut self.ways, &led, counted);
        }
        if let Some(difficulty) = shape.difficulty {
            take_out(&mut self.works, &(difficulty, shape.led), counted);
        }
    }
}

/// Take `counted` out of the row at `key` in `rows`, dropping the row
/// once it has no turns left.
fn take_out<K, Q>(rows: &mut BTreeMap<K, Row>, key: &Q, counted: &Counted)
where
    K: Ord + Borrow<Q>,
    Q: Ord + ?Sized,
{
    if let Some(row) = rows.get_mut(key) {
        row.remove(counted);
        if row.turns == 0 {
            rows.remove(key);
        }
    }
}
