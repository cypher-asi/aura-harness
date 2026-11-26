//! Keys in BENCH's chat panel: the benchmark's detail view's, the form's,
//! and Esc on an agent's chat.

use super::BenchMode;
use crate::app::{AgentsTab, App, ChatTab, NotificationType};
use crate::events::{BenchRequest, BenchStatus};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// What the keys of the detail view act on, from the benchmark it shows.
struct DetailTarget {
    /// Id of the benchmark
    id: String,
    runs: usize,
    /// Whether its runs' copies are kept, for diff, apply and dismiss
    has_copies: bool,
    /// Whether it ran example tasks
    examples: bool,
    /// The agent it was run from
    root: String,
    /// Name of the picked run
    run: Option<String>,
    /// The agent whose log `L` shows
    log_of: Option<String>,
}

impl App {
    /// A key in BENCH's chat panel with nothing typed: the benchmark's,
    /// or the form's. An agent's chat keeps its keys but Esc on CHAT,
    /// which hands them back to the list. Returns whether it used the key.
    pub(in crate::app) fn handle_bench_key(&mut self, key: KeyEvent) -> bool {
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return false;
        }
        match self.bench.mode.clone() {
            Some(BenchMode::Detail(id)) => self.bench_detail_key(key, &id),
            Some(BenchMode::Form) => self.bench_form_key(key),
            Some(BenchMode::Agent { .. }) | None
                if key.code == KeyCode::Esc && self.chat_tab() == ChatTab::Chat =>
            {
                self.focus_bench_list();
                true
            }
            Some(BenchMode::Agent { .. }) | None => false,
        }
    }

    /// What the detail view's keys act on; `None` with no benchmark shown.
    fn detail_target(&self, id: &str) -> Option<DetailTarget> {
        let view = self.bench.shown()?;
        let run = self
            .bench
            .run
            .and_then(|i| view.runs.get(i))
            .map(|r| r.name.clone());
        // The picked run, else the first one under way.
        let log_of = match self.bench.run.and_then(|i| view.runs.get(i)) {
            Some(picked) => Some(picked.agent_id.clone()).filter(|id| !id.is_empty()),
            None => view.live.iter().find_map(|live| live.agent_id.clone()),
        };
        Some(DetailTarget {
            id: id.to_string(),
            runs: view.runs.len(),
            has_copies: view.status.has_copies() && view.status != BenchStatus::Running,
            examples: !view.tasks.is_empty(),
            root: view.root.clone(),
            run,
            log_of,
        })
    }

    fn bench_detail_key(&mut self, key: KeyEvent, id: &str) -> bool {
        let Some(DetailTarget {
            id,
            runs,
            has_copies,
            examples,
            root,
            run,
            log_of,
        }) = self.detail_target(id)
        else {
            return false;
        };
        match key.code {
            KeyCode::Esc | KeyCode::Backspace => self.focus_bench_list(),
            KeyCode::Up | KeyCode::Char('k') => self.bench.scroll_by(-1),
            KeyCode::Down | KeyCode::Char('j') => self.bench.scroll_by(1),
            KeyCode::PageUp => self.bench.scroll_by(-10),
            KeyCode::PageDown => self.bench.scroll_by(10),
            KeyCode::Home | KeyCode::Char('g') => self.bench.scroll = 0,
            KeyCode::Left | KeyCode::Char('h') if runs > 0 => {
                self.bench.run = Some(self.bench.run.map_or(runs - 1, |i| i.saturating_sub(1)));
            }
            KeyCode::Right | KeyCode::Char('l') if runs > 0 => {
                self.bench.run = Some(self.bench.run.map_or(0, |i| (i + 1).min(runs - 1)));
            }
            KeyCode::Char('d') if has_copies => self.show_bench_diff(&root, id, run),
            KeyCode::Char('y') if has_copies && examples => {
                self.notifications.current = Some((
                    "Example tasks ran in their own fixtures; there is nothing to apply"
                        .to_string(),
                    NotificationType::Warning,
                ));
            }
            KeyCode::Char('y') if has_copies => self.apply_bench_run(id, run),
            KeyCode::Char('x') if has_copies => self.send_bench(BenchRequest::Dismiss { id }),
            KeyCode::Char('c') => self.send_bench(BenchRequest::Cancel),
            KeyCode::Char('L') => self.show_bench_run_log(id, log_of),
            _ => return false,
        }
        true
    }

    /// `y` on a benchmark: apply the picked run, or say to pick one.
    fn apply_bench_run(&mut self, id: String, run: Option<String>) {
        match run {
            Some(run) => self.send_bench(BenchRequest::Apply { id, run }),
            None => {
                self.notifications.current = Some((
                    "Pick a run with ←/→ first, then y applies it".to_string(),
                    NotificationType::Warning,
                ));
            }
        }
    }

    /// `L` on a benchmark: show the log of `log_of`, or say there is none.
    fn show_bench_run_log(&mut self, id: String, log_of: Option<String>) {
        match log_of {
            Some(agent) => self.show_bench_log(id, agent),
            None => {
                self.notifications.current = Some((
                    "This run has no log; pick a run with ←/→ first".to_string(),
                    NotificationType::Warning,
                ));
            }
        }
    }

    /// Show the diff of benchmark `id`'s run `run`, or of every run, in
    /// the chat of `root`, the agent it was run from: PROJECTS comes to
    /// the front with that agent on CHAT.
    fn show_bench_diff(&mut self, root: &str, id: String, run: Option<String>) {
        self.switch_app(AgentsTab::Projects);
        if root.is_empty() || root == self.active_agent_id {
            self.show_chat_tab(ChatTab::Chat);
        } else {
            self.give_chat_tab_back(root, ChatTab::Chat);
            self.request_agent_switch(root.to_string());
        }
        self.send_bench_for(root, BenchRequest::Diff { id, run });
    }

    /// Show the LOG tab of `agent`, a run of benchmark `bench`, here under
    /// the benchmark; in PROJECTS when it is neither listed nor archived,
    /// for BENCH to show it under.
    fn show_bench_log(&mut self, bench: String, agent: String) {
        if !self.is_listed(&agent) && !self.is_archived_agent(&agent) {
            self.switch_app(AgentsTab::Projects);
            self.open_log_of(agent);
            return;
        }
        let on_screen = agent == self.active_agent_id;
        if !on_screen {
            self.open_log_on_switch(agent.clone());
        }
        self.show_bench(BenchMode::Agent { bench, agent });
        if on_screen {
            self.show_chat_tab(ChatTab::Log);
        }
    }
}
