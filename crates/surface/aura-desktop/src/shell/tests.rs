use super::*;
use crate::raster;
use aura_ui::events::AgentSummary;
use std::time::Duration;

pub(super) struct Stage {
    pub shell: Shell,
    pub events: mpsc::Receiver<UiEvent>,
    pub now: Instant,
}

pub(super) fn stage() -> Stage {
    let (events, events_rx) = mpsc::channel(64);
    let options = Options {
        theme: "cyber".into(),
        verbose: false,
        tabs_file: None,
    };
    let now = Instant::now();
    let mut shell = Shell::new(events, &options, now);
    shell.ticked = now;
    shell.blink_from = now;
    Stage {
        shell,
        events: events_rx,
        now,
    }
}

fn agent(id: &str, name: &str, running: bool) -> AgentSummary {
    AgentSummary {
        id: id.into(),
        name: name.into(),
        zns_id: format!("0://{name}"),
        running,
        ..AgentSummary::default()
    }
}

impl Stage {
    /// The frame at `self.now`, as pixels.
    pub fn pixels(&mut self) -> Vec<u8> {
        let scene = self.shell.frame(self.now).clone();
        let mut pixels = Vec::new();
        text::with(|text| raster::paint(&scene, text.atlas(), &mut pixels));
        pixels
    }

    pub fn seed(&mut self, running: bool) {
        self.shell.commands([
            UiCommand::SetAgents(vec![agent("a1", "agent01", running)]),
            UiCommand::SetActiveAgent("a1".into()),
        ]);
    }

    /// What the window asked of the session since the last call.
    pub fn asked(&mut self) -> Vec<String> {
        let mut out = Vec::new();
        while let Ok(event) = self.events.try_recv() {
            out.push(format!("{event:?}"));
        }
        out
    }

    /// The session's answer to creating the agent `id`, beside the
    /// one [`Self::seed`] lists.
    pub fn created(&mut self, id: &str, name: &str) {
        self.shell.commands([
            UiCommand::SetActiveAgent(id.into()),
            UiCommand::SetAgents(vec![agent("a1", "agent01", false), agent(id, name, false)]),
            UiCommand::SetActiveAgent(id.into()),
        ]);
    }

    /// Move the clock on and let the shell catch up.
    pub fn pass(&mut self, time: Duration) {
        self.now += time;
        self.shell.advance(self.now);
    }
}

#[test]
fn the_first_frame_fills_the_window_with_the_ui() {
    let mut stage = stage();
    assert!(stage.shell.dirty(), "nothing is drawn yet");
    let scene = stage.shell.frame(stage.now).clone();
    assert!(!stage.shell.dirty());
    assert_eq!((scene.width, scene.height), (1320, 860));
    // Two rows fewer than the window alone holds: the tab bar is above.
    assert_eq!((stage.shell.cols, stage.shell.rows), (155, 45));
    assert!(scene.quads.len() > 500, "{} quads", scene.quads.len());
    let text = stage.shell.text();
    assert_eq!(text.lines().count(), 45);
    assert!(text.contains("AURA"), "{text}");
}

#[test]
fn commands_reach_the_tabs_and_ask_for_a_frame() {
    let mut stage = stage();
    stage.seed(false);
    let before = stage.pixels();
    assert!(!stage.shell.dirty());

    stage.shell.commands([
        UiCommand::StartStreaming,
        UiCommand::AppendText("a reply from the agent".into()),
    ]);
    assert!(stage.shell.dirty());
    let after = stage.pixels();
    assert!(stage.shell.text().contains("a reply from the agent"));
    assert!(before != after, "the reply is on screen");
    assert!(stage.shell.take_effects().is_empty());
}

#[test]
fn commands_for_a_chat_off_screen_ask_for_no_frame() {
    let mut stage = stage();
    stage.shell.commands([
        UiCommand::SetAgents(vec![
            agent("a1", "agent01", false),
            agent("a2", "agent02", false),
        ]),
        UiCommand::SetActiveAgent("a1".into()),
    ]);
    stage.shell.frame(stage.now);
    let to = |agent: &str, command| UiCommand::ForAgent {
        agent_id: agent.into(),
        command: Box::new(command),
    };

    stage
        .shell
        .commands([to("a2", UiCommand::TurnStarted { prompt: None })]);
    assert!(stage.shell.dirty(), "the tab bar marks the tab at work");
    let before = stage.shell.frame(stage.now).clone();
    stage.shell.commands([
        to("a2", UiCommand::StartStreaming),
        to("a2", UiCommand::AppendText("off screen".into())),
    ]);
    assert!(!stage.shell.dirty(), "its chat is not on screen");
    assert!(before == *stage.shell.frame(stage.now));

    stage.shell.commands([
        to("a1", UiCommand::StartStreaming),
        to("a1", UiCommand::AppendText("on screen".into())),
    ]);
    assert!(stage.shell.dirty());
}

/// What `shell` last drew, painted again with nothing kept.
pub(super) fn afresh(shell: &Shell) -> Vec<u8> {
    let view = View {
        size: shell.window,
        cells: &shell.cells,
        cols: shell.cols,
        rows: shell.rows,
        prose: &shell.prose,
        panes: &shell.panes,
        cursor: shell.cursor.filter(|_| shell.cursor_shown),
        metrics: shell.metrics,
        palette: shell.palette,
        bar: &shell.bar,
        chrome: &shell.chrome,
        slide: shell.glide.slide(&shell.tabs.front().tui),
        canvas: super::refresh::canvas_of(shell.tabs.front(), &shell.tui_theme, shell.palette),
    };
    let mut pixels = Vec::new();
    text::with(|text| {
        let mut painter = Painter::default();
        let scene = painter.paint_with(&view, text);
        raster::paint(scene, text.atlas(), &mut pixels);
    });
    pixels
}

#[test]
fn a_streaming_reply_is_drawn_from_kept_rows_as_it_would_be_afresh() {
    let mut stage = stage();
    // A small window: every frame is rasterized twice.
    stage.shell.resized(Size::new(700.0, 420.0), 1.0);
    stage.seed(true);
    stage.shell.commands([UiCommand::StartStreaming]);
    let reply = "The **kernel** records every step, then `cargo test` runs.\n\n\
                 - one thing that wraps over more than a single line of the chat\n\
                 - another\n\n```rust\nfn main() {}\n```\n\n";
    let text: Vec<char> = reply.repeat(6).chars().collect();
    let mut kept = 0;
    for (i, chunk) in text.chunks(9).enumerate() {
        stage
            .shell
            .commands([UiCommand::AppendText(chunk.iter().collect())]);
        // Spinners turn and the cursor blinks as it streams.
        stage.pass(Duration::from_millis(50));
        let drawn = stage.pixels();
        kept += usize::from(stage.shell.painter.built() < usize::from(stage.shell.rows));
        assert!(drawn == afresh(&stage.shell), "chunk {i} left a stale row");
    }
    assert!(kept > 50, "rows were kept from frame to frame: {kept}");
}

#[test]
fn an_idle_window_wakes_once_a_second_and_looks_the_same() {
    let mut stage = stage();
    stage.seed(false);
    // No cursor blinks in a window without the keyboard.
    stage.shell.focused(false, stage.now);
    let before = stage.shell.frame(stage.now).clone();
    assert_eq!(
        stage.shell.deadline(stage.now),
        stage.now + Duration::from_secs(1)
    );

    stage.pass(Duration::from_millis(400));
    assert!(!stage.shell.dirty(), "nothing is due yet");
    stage.pass(Duration::from_millis(600));
    assert!(stage.shell.dirty(), "the second's ticks were made up");
    assert_eq!(
        stage.shell.deadline(stage.now),
        stage.now + Duration::from_secs(1)
    );
    let after = stage.shell.frame(stage.now).clone();
    assert!(before == after, "and nothing moved: nothing to show");
}

#[test]
fn a_working_agent_ticks_twenty_times_a_second() {
    let mut stage = stage();
    stage.seed(true);
    stage.shell.focused(false, stage.now);
    stage.shell.frame(stage.now);
    assert_eq!(
        stage.shell.deadline(stage.now),
        stage.now + Duration::from_millis(50)
    );
    stage.pass(Duration::from_millis(49));
    assert!(!stage.shell.dirty());
    stage.pass(Duration::from_millis(1));
    assert!(stage.shell.dirty(), "a tick turns the spinners");
    stage.shell.frame(stage.now);

    // Held up for a while, it makes the ticks up and is on time again.
    stage.pass(Duration::from_millis(730));
    assert_eq!(
        stage.shell.deadline(stage.now),
        stage.now + Duration::from_millis(20)
    );
    // After a night asleep it does not spin through the night.
    stage.pass(Duration::from_secs(8 * 60 * 60));
    assert_eq!(
        stage.shell.deadline(stage.now),
        stage.now + Duration::from_millis(50)
    );
}

#[test]
fn the_cursor_blinks_only_while_the_window_has_the_keyboard() {
    let mut stage = stage();
    stage.seed(false);
    let on = stage.pixels();
    assert!(stage.shell.cursor.is_some(), "the input line has a cursor");
    assert_eq!(stage.shell.deadline(stage.now), stage.now + BLINK);

    stage.pass(BLINK - Duration::from_millis(1));
    assert!(!stage.shell.dirty());
    stage.pass(Duration::from_millis(1));
    assert!(stage.shell.dirty(), "half a blink on, the cursor goes");
    let off = stage.pixels();
    assert!(on != off);
    // The second's ticks come due before the cursor is back.
    assert_eq!(
        stage.shell.deadline(stage.now),
        stage.now + Duration::from_secs(1) - BLINK
    );

    stage.shell.focused(false, stage.now);
    assert!(on == stage.pixels(), "solid without the keyboard");
    // Nothing left to wake for but the second's ticks.
    assert_eq!(
        stage.shell.deadline(stage.now),
        stage.now + Duration::from_secs(1) - BLINK
    );
    stage.pass(Duration::from_millis(400));
    assert!(!stage.shell.dirty());
}

#[test]
fn a_denser_display_draws_the_same_grid_with_more_pixels() {
    let mut stage = stage();
    stage.shell.frame(stage.now);
    let grid = (stage.shell.cols, stage.shell.rows);
    let cell = stage.shell.metrics.cell;

    stage.shell.resized(Size::new(2640.0, 1720.0), 2.0);
    assert!(stage.shell.dirty());
    let scene = stage.shell.frame(stage.now).clone();
    assert_eq!((scene.width, scene.height), (2640, 1720));
    assert_eq!(stage.shell.metrics.cell.height, cell.height * 2.0);
    assert_eq!(stage.shell.metrics.pad, 12.0, "and the space around it");
    let (cols, rows) = (stage.shell.cols, stage.shell.rows);
    assert!(cols.abs_diff(grid.0) <= 1 && rows.abs_diff(grid.1) <= 1);
}

#[test]
fn a_bell_asks_for_attention() {
    let mut stage = stage();
    stage.seed(false);
    stage.shell.frame(stage.now);
    assert!(stage.shell.take_effects().is_empty());
    let alerts = stage.shell.tabs.settings().alerts;
    if alerts.desktop {
        // Not from a test: it would pop up a notification.
        return;
    }
    stage.shell.commands([UiCommand::ForAgent {
        agent_id: "a1".into(),
        command: Box::new(UiCommand::RequestApproval {
            id: "r1".into(),
            tool: "run_command".into(),
            description: "rm -rf target/".into(),
            always: "`rm` commands in this project".into(),
        }),
    }]);
    let effects = stage.shell.take_effects();
    assert_eq!(
        effects.contains(&Effect::Attention),
        alerts.bell,
        "{effects:?}"
    );
}
