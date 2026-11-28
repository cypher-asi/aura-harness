//! An app on a test terminal, and the frame and load timings taken on it.

use super::fixtures::{agents, id};
use aura_ui_terminal::events::MessageData;
use aura_ui_terminal::{render, App, Theme, UiCommand};
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use std::time::{Duration, Instant};

pub(crate) const WIDTH: u16 = 200;
pub(crate) const HEIGHT: u16 = 60;

pub(crate) struct Bench {
    pub(crate) app: App,
    theme: Theme,
    terminal: Terminal<TestBackend>,
}

impl Bench {
    pub(crate) fn new() -> Self {
        let mut app = App::new();
        app.process_command(UiCommand::SetAgents(agents()));
        app.process_command(UiCommand::SetActiveAgent(id(0)));
        Self {
            app,
            theme: Theme::cyber(),
            terminal: Terminal::new(TestBackend::new(WIDTH, HEIGHT)).unwrap(),
        }
    }

    pub(crate) fn frame(&mut self) -> Duration {
        let start = Instant::now();
        self.terminal
            .draw(|f| render(f, &mut self.app, &self.theme))
            .unwrap();
        start.elapsed()
    }

    pub(crate) fn load(&mut self, history: Vec<MessageData>) -> Duration {
        let start = Instant::now();
        self.app
            .process_command(UiCommand::LoadConversation(history));
        start.elapsed()
    }

    pub(crate) fn load_for(&mut self, agent: usize, history: Vec<MessageData>) {
        self.app.process_command(UiCommand::ForAgent {
            agent_id: id(agent),
            command: Box::new(UiCommand::LoadConversation(history)),
        });
    }
}

/// One chat's first-frame cost in a fresh app, the cheapest of `runs`: the
/// chat-line layout runs across the rayon pool, so a single run moves by
/// hundreds of milliseconds.
pub(crate) fn fresh_load(history: &[MessageData], runs: usize) -> (Duration, Duration, Duration) {
    let mut apply = Duration::MAX;
    let mut first = Duration::MAX;
    let mut after = Duration::MAX;
    for _ in 0..runs {
        let mut bench = Bench::new();
        apply = apply.min(bench.load(history.to_vec()));
        first = first.min(bench.frame());
        after = after.min(bench.frame());
    }
    (apply, first, after)
}

/// The first frame's cost of `history`, the cheapest of `runs` fresh apps.
pub(crate) fn first_frame(history: &[MessageData], runs: usize) -> Duration {
    let mut best = Duration::MAX;
    for _ in 0..runs {
        let mut bench = Bench::new();
        bench.load(history.to_vec());
        best = best.min(bench.frame());
    }
    best
}
