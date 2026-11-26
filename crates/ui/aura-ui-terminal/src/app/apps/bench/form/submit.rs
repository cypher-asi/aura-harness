//! Enter on the form's last row: the estimate first, then the start.

use super::{BenchForm, WorkChoice, DEFAULT_AGAINST, DIFFICULTIES};
use crate::app::{App, FormField, NotificationType};
use crate::events::{BenchRequest, BenchSpec, BenchWork};

impl App {
    /// Enter on the form's last row: ask what the benchmark should cost,
    /// or start it once that came back.
    pub(super) fn submit_bench_form(&mut self) {
        if self.bench.form.waiting {
            return;
        }
        if self.bench.form.from.is_none() {
            // The agents were listed after the form opened.
            self.settle_bench_from();
        }
        if self.bench.form.ready() {
            self.bench.start_unanswered = true;
            self.bench.show_started = true;
            self.send_form_bench(BenchRequest::Start);
            return;
        }
        if self.bench.form.b == DEFAULT_AGAINST {
            // `opus-5-5` itself matches only a Claude plan's id; with no
            // runnable Opus 5.5 (or no model list yet) the server's
            // strongest Opus stands in.
            self.bench.form.b = self.default_bench_against();
        }
        let Some(spec) = self.bench_form_spec() else {
            return;
        };
        self.bench.form.waiting = true;
        self.send_form_bench(BenchRequest::Prepare(spec));
    }

    /// The benchmark the form sets up; `None`, with a warning, while a row
    /// it needs is missing or wrong.
    fn bench_form_spec(&mut self) -> Option<BenchSpec> {
        let form = self.bench.form.clone();
        let work = self.bench_form_work(&form)?;
        let budget_usd = self.bench_form_budget(&form)?;
        let [a, b] = self.bench_form_arms(&form)?;
        Some(BenchSpec {
            work,
            a,
            b,
            repeats: form.repeats,
            check: Some(form.check.clone()).filter(|c| !c.is_empty()),
            budget_usd,
            classifier: form.classifier_model(),
        })
    }

    /// The work the runs do; `None` for a prompt not typed yet, which the
    /// form goes to.
    fn bench_form_work(&mut self, form: &BenchForm) -> Option<BenchWork> {
        let work = match self.bench.work() {
            WorkChoice::LastPrompt => Some(BenchWork::LastPrompt),
            WorkChoice::Prompt if form.prompt.trim().is_empty() => None,
            WorkChoice::Prompt => Some(BenchWork::Prompt(form.prompt.clone())),
            WorkChoice::Examples(suite) => Some(BenchWork::Examples {
                suite: suite.map(str::to_string),
                tasks: form.tasks.clamp(1, self.bench.example_count().max(1)),
            }),
            WorkChoice::Session(session) if session.current => Some(BenchWork::CurrentSession),
            WorkChoice::Session(session) => Some(BenchWork::Session(session.started_ms)),
        };
        if work.is_none() {
            self.bench.form.field = FormField::Prompt;
            self.notifications.current = Some((
                "Type the prompt in the input line and press Enter".to_string(),
                NotificationType::Warning,
            ));
        }
        work
    }

    /// The budget in USD, `Some(None)` for none; `None` for one that is
    /// not an amount.
    fn bench_form_budget(&mut self, form: &BenchForm) -> Option<Option<f64>> {
        match form.budget.trim() {
            "" => Some(None),
            text => match text.parse::<f64>() {
                Ok(usd) if usd > 0.0 => Some(Some(usd)),
                _ => {
                    self.notifications.current = Some((
                        format!("Budget {text:?} is not an amount in USD"),
                        NotificationType::Warning,
                    ));
                    None
                }
            },
        }
    }

    /// What arms A and B are sent as; `None` for a by-difficulty arm
    /// missing a model, whose row the form goes to.
    fn bench_form_arms(&mut self, form: &BenchForm) -> Option<[String; 2]> {
        let mut arms = [String::new(), String::new()];
        for (arm, value) in arms.iter_mut().enumerate() {
            match form.arm_value(arm) {
                Ok(arm_value) => *value = arm_value,
                Err(missing) => {
                    let difficulty = missing
                        .difficulty()
                        .map_or("", |(_, difficulty)| DIFFICULTIES[difficulty]);
                    self.bench.form.field = missing;
                    self.notifications.current = Some((
                        format!(
                            "Pick a model for {difficulty} work of arm {}",
                            if arm == 0 { "A" } else { "B" }
                        ),
                        NotificationType::Warning,
                    ));
                    return None;
                }
            }
        }
        Some(arms)
    }
}
