//! The UI side of the load harness: the real terminal `App` and
//! renderer on a test screen, on a thread of their own, paced as
//! `aura_ui_terminal::Terminal::run` paces them: draw when asked, then
//! take the agents' commands every 5 ms until the next 50 ms frame.

use super::measure::ByKind;
use aura_ui::{UiCommand, UiEvent};
use aura_ui_terminal::{render, App, Theme};
use ratatui::{backend::TestBackend, Terminal};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// Time between frames, as the terminal's.
const FRAME: Duration = Duration::from_millis(50);
/// Time between takes of the agents' commands within a frame.
const COMMAND_POLL: Duration = Duration::from_millis(5);
/// The test screen.
const SCREEN: (u16, u16) = (200, 60);

/// What the UI thread measured.
#[derive(Default)]
pub(super) struct UiStats {
    /// Time to process each command, by kind
    pub(super) commands: ByKind,
    /// Time to draw each frame
    pub(super) frames: Vec<Duration>,
    /// Time of the app's per-frame tick
    pub(super) ticks: Vec<Duration>,
    /// Commands waiting each time the UI looked
    pub(super) depths: Vec<usize>,
    /// Longest the UI thread went without drawing or taking commands
    pub(super) longest_gap: Duration,
}

/// Run the UI until `stop`, taking `commands` and sending the UI's
/// events to `events`.
pub(super) fn spawn(
    mut commands: mpsc::Receiver<UiCommand>,
    events: mpsc::Sender<UiEvent>,
    stop: Arc<AtomicBool>,
    measuring: Arc<AtomicBool>,
) -> std::thread::JoinHandle<UiStats> {
    std::thread::spawn(move || {
        let mut app = App::new().with_event_sender(events);
        let mut terminal = Terminal::new(TestBackend::new(SCREEN.0, SCREEN.1)).unwrap();
        let theme = Theme::cyber();
        let mut stats = UiStats::default();
        let mut frame_no = 0u64;
        let mut last = Instant::now();
        while !stop.load(Ordering::Relaxed) {
            let on = measuring.load(Ordering::Relaxed);
            take(&mut app, &mut commands, &mut stats, on);
            let started = Instant::now();
            app.tick();
            if on {
                stats.ticks.push(started.elapsed());
            }
            frame_no += 1;
            // The user scrolls the chat now and then.
            if frame_no % 20 == 0 {
                app.scroll_up(3);
            } else if frame_no % 20 == 10 {
                app.scroll_down(3);
            }
            if app.take_redraw() || frame_no % 20 == 0 {
                let started = Instant::now();
                terminal
                    .draw(|frame| render(frame, &mut app, &theme))
                    .unwrap();
                if on {
                    stats.frames.push(started.elapsed());
                }
            }
            let deadline = Instant::now() + FRAME;
            while Instant::now() < deadline && !stop.load(Ordering::Relaxed) {
                if on {
                    stats.longest_gap = stats.longest_gap.max(last.elapsed());
                }
                std::thread::sleep(COMMAND_POLL);
                last = Instant::now();
                take(&mut app, &mut commands, &mut stats, on);
            }
            if on {
                stats.longest_gap = stats.longest_gap.max(last.elapsed());
            }
            last = Instant::now();
        }
        stats
    })
}

/// Process every command waiting, timing each when `on`.
fn take(app: &mut App, commands: &mut mpsc::Receiver<UiCommand>, stats: &mut UiStats, on: bool) {
    if on {
        stats.depths.push(commands.len());
    }
    while let Ok(command) = commands.try_recv() {
        let kind = kind_of(&command);
        let started = Instant::now();
        app.process_command(command);
        if on {
            stats.commands.add(kind, started.elapsed());
        }
    }
}

/// A short name for `command`'s kind: its variant, and the variant
/// within for a command for one agent.
fn kind_of(command: &UiCommand) -> &'static str {
    if let UiCommand::ForAgent { command, .. } = command {
        return intern(&format!("agent:{}", variant(command)));
    }
    intern(&variant(command))
}

/// The variant name `command`'s `Debug` starts with, read no further
/// than its first 40 characters.
fn variant(command: &UiCommand) -> String {
    struct Head(String);
    impl std::fmt::Write for Head {
        fn write_str(&mut self, s: &str) -> std::fmt::Result {
            self.0.push_str(s);
            if self.0.len() >= 40 {
                return Err(std::fmt::Error);
            }
            Ok(())
        }
    }
    let mut head = Head(String::new());
    let _ = std::fmt::write(&mut head, format_args!("{command:?}"));
    head.0
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// `name` as a `'static` string, each name leaked once.
fn intern(name: &str) -> &'static str {
    use std::collections::HashSet;
    use std::sync::{Mutex, OnceLock};
    static NAMES: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
    let mut names = NAMES.get_or_init(Mutex::default).lock().unwrap();
    if let Some(known) = names.get(name) {
        return known;
    }
    let leaked: &'static str = Box::leak(name.to_string().into_boxed_str());
    names.insert(leaked);
    leaked
}
