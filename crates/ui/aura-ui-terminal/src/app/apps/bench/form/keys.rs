//! Keys on the form: moving between its rows, cycling their values and
//! clearing the typed ones.

use super::{cycle, FormField, AUTO_CLASSIFIER, MAX_REPEATS};
use crate::app::App;
use crate::events::BenchRequest;
use crossterm::event::{KeyCode, KeyEvent};

impl App {
    pub(in crate::app::apps::bench) fn bench_form_key(&mut self, key: KeyEvent) -> bool {
        let field = self.bench.form.field;
        match key.code {
            KeyCode::Esc => {
                if self.bench.form.ready() {
                    self.send_form_bench(BenchRequest::Cancel);
                }
                self.bench.form.estimate = None;
                self.drop_bench_form();
            }
            KeyCode::Up | KeyCode::Char('k') | KeyCode::BackTab => self.move_form_field(false),
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => self.move_form_field(true),
            KeyCode::Left | KeyCode::Char('h') => self.change_form_value(false),
            KeyCode::Right | KeyCode::Char('l') => self.change_form_value(true),
            KeyCode::Delete | KeyCode::Backspace if field.is_text() => self.clear_form_field(field),
            KeyCode::Enter if field == FormField::Go => self.submit_bench_form(),
            KeyCode::Enter if field.holds_model() => self.open_bench_model_picker(field, ""),
            KeyCode::Enter => self.move_form_field(true),
            _ => return false,
        }
        true
    }

    /// Delete on a typed row: clear its value, or put the classifier back
    /// on its default.
    fn clear_form_field(&mut self, field: FormField) {
        let default_classifier = self.default_bench_classifier();
        let form = &mut self.bench.form;
        match field {
            FormField::Prompt => form.prompt.clear(),
            FormField::Check => form.check.clear(),
            FormField::Budget => form.budget.clear(),
            FormField::Classifier => form.classifier = default_classifier,
            _ => {
                if let Some(model) = form.row_model_mut(field) {
                    model.clear();
                }
            }
        }
        form.estimate = None;
    }

    /// Move to the next row on the form, down or up, past those not shown.
    pub(super) fn move_form_field(&mut self, down: bool) {
        let form = &mut self.bench.form;
        let mut at = form.field.index();
        loop {
            let next = if down {
                (at + 1).min(FormField::ALL.len() - 1)
            } else {
                at.saturating_sub(1)
            };
            if next == at {
                return;
            }
            at = next;
            if form.shown(FormField::ALL[at]) {
                form.field = FormField::ALL[at];
                return;
            }
        }
    }

    pub(super) fn change_form_value(&mut self, forward: bool) {
        let field = self.bench.form.field;
        if field.holds_model() {
            self.cycle_form_model(field, forward);
            return;
        }
        let changed = match field {
            FormField::From => self.cycle_form_from(forward),
            FormField::Work => {
                self.cycle_form_work(forward);
                true
            }
            FormField::A | FormField::B => {
                self.cycle_form_arm(field, forward);
                true
            }
            FormField::Tasks => self.step_form_tasks(forward),
            FormField::Repeats => {
                self.step_form_repeats(forward);
                true
            }
            _ => false,
        };
        if changed {
            self.bench.form.estimate = None;
        }
    }

    /// Cycle the model of a row that holds one.
    fn cycle_form_model(&mut self, field: FormField, forward: bool) {
        let mut choices = self.bench_model_choices();
        if field == FormField::Classifier {
            choices.insert(0, AUTO_CLASSIFIER.to_string());
        }
        if let Some(value) = self.bench.form.row_model_mut(field) {
            *value = cycle(&choices, value, forward);
        }
        self.bench.form.estimate = None;
    }

    /// Cycle the agent it is run from, asking for its sessions. Returns
    /// whether it changed.
    fn cycle_form_from(&mut self, forward: bool) -> bool {
        let choices: Vec<String> = self
            .bench_from_choices()
            .iter()
            .map(|a| a.id.clone())
            .collect();
        let current = self.bench.form.from.clone().unwrap_or_default();
        let next = cycle(&choices, &current, forward);
        if next == current {
            return false;
        }
        self.set_bench_from(Some(next));
        self.send_form_bench(BenchRequest::Sessions);
        true
    }

    fn cycle_form_work(&mut self, forward: bool) {
        let count = self.bench.work_choices().len();
        let current = self.bench.work_index();
        let form = &mut self.bench.form;
        form.work = if forward {
            (current + 1) % count
        } else {
            (current + count - 1) % count
        };
        let most = self.bench.example_count();
        if most > 0 {
            self.bench.form.tasks = self.bench.form.tasks.clamp(1, most);
        }
    }

    fn cycle_form_arm(&mut self, field: FormField, forward: bool) {
        self.settle_bench_against();
        let choices = self.bench_arm_choices(field == FormField::B);
        let form = &mut self.bench.form;
        let value = if field == FormField::A {
            &mut form.a
        } else {
            &mut form.b
        };
        *value = cycle(&choices, value, forward);
    }

    /// Step the task count; returns `false` when the work has no example
    /// tasks.
    fn step_form_tasks(&mut self, forward: bool) -> bool {
        let most = self.bench.example_count();
        if most == 0 {
            return false;
        }
        let form = &mut self.bench.form;
        form.tasks = if forward {
            (form.tasks + 1).min(most)
        } else {
            form.tasks.saturating_sub(1).max(1)
        };
        true
    }

    fn step_form_repeats(&mut self, forward: bool) {
        let form = &mut self.bench.form;
        form.repeats = if forward {
            (form.repeats + 1).min(MAX_REPEATS)
        } else {
            form.repeats.saturating_sub(1).max(1)
        };
    }
}
