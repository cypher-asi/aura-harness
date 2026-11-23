//! What the user is asked to confirm before a comparison starts: the
//! work, the arms, the runs, the check and what it should cost.

use super::format::{headline, money, plural};
use aura_core_types::ExampleTaskInfo;
use aura_gate::compare::Estimate;
use aura_model::model_catalog::route_label;
use std::borrow::Cow;
use std::fmt::Write;

/// Characters an estimate row's label takes, the value column after it.
const LABEL_WIDTH: usize = 14;

/// Characters an estimate row's value wraps at.
pub(super) const VALUE_WIDTH: usize = 64;

/// What the user is asked to confirm before a comparison starts.
pub(crate) struct EstimateView<'a> {
    pub task: &'a str,
    /// Prompts each run sends
    pub prompts: usize,
    /// The replayed session's title, when the work is one
    pub session: Option<&'a str>,
    /// The example tasks, when the work is those
    pub examples: &'a [ExampleTaskInfo],
    /// Arm A as it reads, with Gate's likely pick when Gate routes it
    pub a: String,
    pub b: String,
    /// Model that classifies the runs' prompts
    pub classifier: &'a str,
    /// Whether the user named the classifier rather than leaving it to
    /// Gate
    pub classifier_named: bool,
    pub judge: &'a str,
    pub repeats: u32,
    pub check: Option<&'a str>,
    pub budget_usd: Option<f64>,
    pub concurrency: usize,
    pub estimate: Estimate,
    pub notes: &'a [String],
}

impl EstimateView<'_> {
    /// A heading, then one labelled row per fact, as the BENCH form lays
    /// out its own rows.
    pub(crate) fn render(&self) -> String {
        let mut out = self.heading();
        if !self.examples.is_empty() {
            let ids: Vec<&str> = self.examples.iter().map(|t| t.id.as_str()).collect();
            row(&mut out, "Tasks", &ids.join(", "));
        }
        row(&mut out, "A", &self.a);
        row(&mut out, "B", &self.b);
        row(&mut out, "Classifier", &self.classifier());
        row(&mut out, "Runs", &self.runs());
        row(&mut out, "Check", &self.check());
        row(&mut out, "Estimate", &self.cost());
        if let Some(budget) = self.budget_usd {
            row(&mut out, "Budget", &self.budget(budget));
        }
        for note in self.notes {
            row(&mut out, "Note", note);
        }
        out
    }

    /// What the comparison runs: example tasks, a replayed session or a
    /// prompt.
    fn heading(&self) -> String {
        let examples = self.examples.len();
        if examples > 0 {
            return format!(
                "Benchmark: {examples} example coding task{}",
                plural(examples)
            );
        }
        match self.session {
            Some(title) => format!(
                "Benchmark: replay the session \"{}\", {} prompt{} in order",
                headline(title, 72),
                self.prompts,
                plural(self.prompts),
            ),
            None => format!("Compare: \"{}\"", headline(self.task, 80)),
        }
    }

    /// The model that classifies the runs' prompts, and how to pick
    /// another when it is Gate's choice.
    fn classifier(&self) -> String {
        let model = route_label(self.classifier);
        if self.classifier_named {
            return model;
        }
        format!("{model} · Gate's choice of fast model; --classifier <model> picks another")
    }

    /// How many runs each arm gets, how many at once, and where they work.
    fn runs(&self) -> String {
        let repeats = self.repeats as usize;
        if self.examples.is_empty() {
            return format!(
                "{repeats} per arm, up to {} at a time,\neach in its own copy of the workspace",
                self.concurrency
            );
        }
        format!(
            "{repeats} of each task per arm ({} per arm), up to {} at a time,\n\
             each in a fresh copy of its task's files",
            repeats * self.examples.len(),
            self.concurrency
        )
    }

    /// What checks each run.
    fn check(&self) -> Cow<'_, str> {
        match self.check {
            _ if !self.examples.is_empty() => {
                "each task's own command; hidden checks go in only after the run".into()
            }
            Some(check) => format!("`{check}` after each run").into(),
            None => "none · --check <cmd> or AURA_CHECK_COMMAND counts passes".into(),
        }
    }

    /// What the budget of `budget_usd` stops, and, when an arm's price is
    /// unknown, that its calls are not counted toward it. The judge runs
    /// after every run has ended, so its price does not matter here.
    fn budget(&self, budget_usd: f64) -> String {
        let mut budget = format!(
            "{}: no new run starts once the finished ones cost that much",
            money(budget_usd)
        );
        let unpriced: Vec<&str> = [("A", self.estimate.a_usd), ("B", self.estimate.b_usd)]
            .into_iter()
            .filter_map(|(arm, usd)| usd.is_none().then_some(arm))
            .collect();
        let (arms, has, its) = match unpriced.as_slice() {
            [] => return budget,
            [arm] => ((*arm).to_string(), "has", "its"),
            arms => (arms.join(" and "), "have", "their"),
        };
        let _ = write!(
            budget,
            "\nit counts only priced calls: {arms} {has} no known price, \
             so {its} calls are not counted"
        );
        budget
    }

    /// What each arm and the judge should cost at list prices.
    fn cost(&self) -> String {
        let part = |usd: Option<f64>| usd.map_or_else(|| "unknown".to_string(), money);
        let mut cost = format!(
            "~{} at list prices\nA ~{} · B ~{}\njudge ~{} on {}\n\
             for turns of about 150k input and 6k output tokens",
            money(self.estimate.known_usd()),
            part(self.estimate.a_usd),
            part(self.estimate.b_usd),
            part(self.estimate.judge_usd),
            route_label(self.judge),
        );
        if !self.estimate.fully_priced() {
            cost.push_str("\nsome prices are unknown");
        }
        cost
    }
}

/// Append the row `label` reading `value` to `out`: each line of `value`
/// wrapped at [`VALUE_WIDTH`], under the value column.
pub(super) fn row(out: &mut String, label: &str, value: &str) {
    let mut label = label;
    for line in value.lines().flat_map(|line| wrap(line, VALUE_WIDTH)) {
        let _ = write!(out, "\n  {label:<LABEL_WIDTH$}{line}");
        label = "";
    }
}

/// `text` split at spaces into lines of at most `width` characters; a
/// word longer than that keeps a line of its own.
pub(super) fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = vec![String::new()];
    for word in text.split(' ') {
        let line = lines.last_mut().expect("there is always a line");
        if line.is_empty() {
            line.push_str(word);
        } else if line.chars().count() + 1 + word.chars().count() <= width {
            line.push(' ');
            line.push_str(word);
        } else {
            lines.push(word.to_string());
        }
    }
    lines
}
