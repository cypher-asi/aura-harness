//! The bare window's timings, on the scenes the canvas renderer's
//! benchmarks in [`crate::snapshot`] time, to set the two side by side
//! on one build:
//! `cargo test --release -p aura-desktop -- --ignored --nocapture timings`.
//!
//! The canvas window renders the terminal UI in every update. This one
//! applies what happens when it happens and renders once per frame of
//! the display, so its updates are reported with the frames they lead
//! to.

use super::keys::Press;
use super::tests::{stage, Stage};
use crate::demo::{LISTING, REPLY};
use crate::raster;
use crate::snapshot::{agent, report, seed, steps, Driven};
use crate::text;
use aura_ui::events::{ToolData, UiCommand};
use std::time::{Duration, Instant};
use winit::keyboard::NamedKey;

/// Frames a display shows in a second.
const REFRESH: usize = 60;

impl Driven for Stage {
    fn apply(&mut self, commands: impl IntoIterator<Item = UiCommand>) {
        self.shell.commands(commands);
        self.shell.frame(self.now);
    }

    fn submit(&mut self, text: &str) {
        let mut buffer = [0; 4];
        for ch in text.chars() {
            let key = Press::typing(ch.encode_utf8(&mut buffer));
            self.shell.key_pressed(&key, self.now);
        }
        self.shell
            .key_pressed(&Press::of(NamedKey::Enter), self.now);
        self.shell.frame(self.now);
    }
}

fn sum(samples: &[Duration]) -> f64 {
    samples.iter().sum::<Duration>().as_secs_f64() * 1e3
}

/// As `snapshot::replay_timings`: a long reply streaming in small
/// chunks with a frame after each, typing, and whole frames rasterized
/// on the CPU as they are where there is no GPU.
#[test]
#[ignore = "benchmark"]
fn bare_replay_timings() {
    let started = Instant::now();
    let mut stage = stage();
    stage.shell.frame(stage.now);
    let boot = started.elapsed();
    seed(&mut stage);
    steps(&mut stage);
    stage.apply([UiCommand::StartStreaming]);

    let text: Vec<char> = REPLY
        .replace("{prompt}", "the crates")
        .repeat(8)
        .chars()
        .collect();
    let (mut streaming, mut frames, mut rasters) = (Vec::new(), Vec::new(), Vec::new());
    let (mut changed_frames, mut changed_rows) = (0, 0);
    let mut pixels = Vec::new();
    for (i, chunk) in text.chunks(12).enumerate() {
        let chunk: String = chunk.iter().collect();
        let t = Instant::now();
        stage.shell.commands([UiCommand::AppendText(chunk)]);
        streaming.push(t.elapsed());
        let t = Instant::now();
        stage.shell.frame(stage.now);
        frames.push(t.elapsed());
        let rows = stage.shell.painter.built();
        changed_frames += usize::from(rows > 0);
        changed_rows += rows;
        if i % 25 == 0 {
            let scene = stage.shell.painter.scene();
            let t = Instant::now();
            text::with(|text| raster::paint(scene, text.atlas(), &mut pixels));
            rasters.push(t.elapsed());
        }
    }
    stage.apply([UiCommand::FinishStreaming, UiCommand::Complete]);

    let mut typing = Vec::new();
    let mut buffer = [0; 4];
    for c in "how are the crates tested?".chars() {
        let key = Press::typing(c.encode_utf8(&mut buffer));
        let t = Instant::now();
        stage.shell.key_pressed(&key, stage.now);
        stage.shell.frame(stage.now);
        typing.push(t.elapsed());
    }

    println!(
        "boot to first frame's quads: {:.2} ms",
        boot.as_secs_f64() * 1e3
    );
    report("commands while streaming", &mut streaming);
    report("frame per streamed chunk (render + quads)", &mut frames);
    report("key and its frame", &mut typing);
    println!(
        "rows painted anew: {changed_rows} over {changed_frames} changed frames of {} rows",
        stage.shell.rows
    );
    report("whole frame rasterized on the CPU", &mut rasters);
}

/// As `snapshot::swarm_timings`: one second of thirty agents streaming
/// at once, arriving a command at a time, about a frame's worth at a
/// time, and all at once, with the frames a display shows in that
/// second.
#[test]
#[ignore = "benchmark"]
fn bare_swarm_timings() {
    const AGENTS: usize = 30;
    const COMMANDS_PER_AGENT: usize = 100;
    let reply: Vec<char> = REPLY.replace("{prompt}", "the crates").chars().collect();
    let mut chunks = reply.chunks(12).cycle();
    let mut second = Vec::new();
    for step in 0..COMMANDS_PER_AGENT {
        second.push(UiCommand::AppendText(
            chunks.next().unwrap().iter().collect(),
        ));
        for i in 1..AGENTS {
            let command = match step % 10 {
                0 => UiCommand::ShowTool(ToolData {
                    id: format!("t{step}"),
                    name: "read_file".into(),
                    args: r#"{"path":"src/lib.rs"}"#.into(),
                }),
                1 => UiCommand::CompleteTool {
                    id: format!("t{}", step - 1),
                    result: LISTING.into(),
                    success: true,
                },
                2..=4 => UiCommand::AppendThinking("weighing the options ".into()),
                _ => UiCommand::AppendText(chunks.next().unwrap().iter().collect()),
            };
            second.push(UiCommand::ForAgent {
                agent_id: format!("b{i:02}"),
                command: Box::new(command),
            });
        }
    }

    let mut agents = vec![agent("a1", "Agent01", 0, Some("aura-harness"))];
    agents
        .extend((1..AGENTS).map(|i| agent(&format!("b{i:02}"), &format!("Agent{i:02}"), 0, None)));
    for per_update in [1, 50, 1024] {
        let mut stage = stage();
        seed(&mut stage);
        stage.apply([UiCommand::SetAgents(agents.clone())]);
        steps(&mut stage);
        stage.apply([UiCommand::StartStreaming]);
        let batches: Vec<&[UiCommand]> = second.chunks(per_update).collect();
        // The display takes a frame every so many updates; fewer than
        // its sixty when the second has fewer updates than that.
        let every = batches.len().div_ceil(REFRESH).max(1);
        let (mut updates, mut frames) = (Vec::new(), Vec::new());
        for (i, batch) in batches.iter().enumerate() {
            let t = Instant::now();
            stage.shell.commands(batch.to_vec());
            updates.push(t.elapsed());
            if (i + 1) % every == 0 || i + 1 == batches.len() {
                let t = Instant::now();
                stage.shell.frame(stage.now);
                frames.push(t.elapsed());
            }
        }
        println!(
            "{} commands from {AGENTS} agents, {per_update} per update: busy {:.1} ms of the \
             second ({:.1} applying, {:.1} in {} frames)",
            second.len(),
            sum(&updates) + sum(&frames),
            sum(&updates),
            sum(&frames),
            frames.len(),
        );
        report("  update", &mut updates);
        report("  frame", &mut frames);
    }
}
