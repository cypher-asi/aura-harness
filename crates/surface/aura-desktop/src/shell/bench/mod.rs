//! The window's timings on the scenes of [`super::snapshot`]:
//! `cargo test --release -p aura-desktop -- --ignored --nocapture timings`.
//!
//! The window applies what happens when it happens and renders once per
//! frame of the display, so updates are reported with the frames they
//! lead to.

mod history;
mod scroll;

use super::keys::Press;
use super::snapshot::{agent, report, seed, steps};
use super::tests::{stage, Stage};
use crate::demo::{LISTING, REPLY};
use crate::raster;
use crate::text;
use aura_ui::events::{AgentSummary, ToolData, UiCommand};
use std::time::{Duration, Instant};

/// Frames a display shows in a second.
const REFRESH: usize = 60;

fn sum(samples: &[Duration]) -> f64 {
    samples.iter().sum::<Duration>().as_secs_f64() * 1e3
}

/// The demo reply, `times` over, as characters to stream.
fn long_reply(times: usize) -> Vec<char> {
    REPLY
        .replace("{prompt}", "the crates")
        .repeat(times)
        .chars()
        .collect()
}

/// A key and its frame for each character of a question.
fn type_question(stage: &mut Stage) -> Vec<Duration> {
    let mut typing = Vec::new();
    let mut buffer = [0; 4];
    for c in "how are the crates tested?".chars() {
        let key = Press::typing(c.encode_utf8(&mut buffer));
        let t = Instant::now();
        stage.shell.key_pressed(&key, stage.now);
        stage.shell.frame(stage.now);
        typing.push(t.elapsed());
    }
    typing
}

/// What streaming a reply took.
#[derive(Default)]
struct Streamed {
    streaming: Vec<Duration>,
    frames: Vec<Duration>,
    rasters: Vec<Duration>,
    changed_frames: usize,
    changed_rows: usize,
}

/// Stream `text` in small chunks with a frame after each, and rasterize
/// every twenty-fifth frame on the CPU.
fn stream_reply(stage: &mut Stage, text: &[char]) -> Streamed {
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
    Streamed {
        streaming,
        frames,
        rasters,
        changed_frames,
        changed_rows,
    }
}

fn print_replay(boot: Duration, streamed: &mut Streamed, typing: &mut [Duration], rows: u16) {
    println!(
        "boot to first frame's quads: {:.2} ms",
        boot.as_secs_f64() * 1e3
    );
    report("commands while streaming", &mut streamed.streaming);
    report(
        "frame per streamed chunk (render + quads)",
        &mut streamed.frames,
    );
    report("key and its frame", typing);
    println!(
        "rows painted anew: {} over {} changed frames of {rows} rows",
        streamed.changed_rows, streamed.changed_frames,
    );
    report("whole frame rasterized on the CPU", &mut streamed.rasters);
}

/// A session replayed the way a model streams it: a long reply in small
/// chunks with a frame after each, typing, and whole frames rasterized
/// on the CPU as they are where there is no GPU.
#[test]
#[ignore = "benchmark"]
fn replay_timings() {
    let started = Instant::now();
    let mut stage = stage();
    stage.shell.frame(stage.now);
    let boot = started.elapsed();
    seed(&mut stage);
    steps(&mut stage);
    stage.apply([UiCommand::StartStreaming]);

    let text = long_reply(8);
    let mut streamed = stream_reply(&mut stage, &text);
    stage.apply([UiCommand::FinishStreaming, UiCommand::Complete]);
    let mut typing = type_question(&mut stage);
    print_replay(boot, &mut streamed, &mut typing, stage.shell.rows);
}

const SWARM_AGENTS: usize = 30;

/// What a helper of the swarm sends at `step`: tool calls, thinking and
/// reply text in turn.
fn helper_command<'a>(step: usize, chunks: &mut impl Iterator<Item = &'a [char]>) -> UiCommand {
    match step % 10 {
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
    }
}

/// A second of the swarm's commands, in the order they arrive.
fn swarm_second() -> Vec<UiCommand> {
    const COMMANDS_PER_AGENT: usize = 100;
    let reply: Vec<char> = REPLY.replace("{prompt}", "the crates").chars().collect();
    let mut chunks = reply.chunks(12).cycle();
    let mut second = Vec::new();
    for step in 0..COMMANDS_PER_AGENT {
        second.push(UiCommand::AppendText(
            chunks.next().unwrap().iter().collect(),
        ));
        for i in 1..SWARM_AGENTS {
            let command = helper_command(step, &mut chunks);
            second.push(UiCommand::ForAgent {
                agent_id: format!("b{i:02}"),
                command: Box::new(command),
            });
        }
    }
    second
}

fn swarm_agents() -> Vec<AgentSummary> {
    let mut agents = vec![agent("a1", "Agent01", 0, Some("aura-harness"))];
    agents.extend(
        (1..SWARM_AGENTS).map(|i| agent(&format!("b{i:02}"), &format!("Agent{i:02}"), 0, None)),
    );
    agents
}

/// Apply `second` `per_update` commands at a time, with the frames a
/// display takes in between.
fn swarm_updates(
    stage: &mut Stage,
    second: &[UiCommand],
    per_update: usize,
) -> (Vec<Duration>, Vec<Duration>) {
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
    (updates, frames)
}

/// One second of a swarm streaming at once: the agent in front writes a
/// reply while the others think, call tools and write theirs. The
/// commands arrive one at a time, about a frame's worth at a time, and
/// all at once, with the frames a display shows in that second.
#[test]
#[ignore = "benchmark"]
fn swarm_timings() {
    const AGENTS: usize = SWARM_AGENTS;
    let second = swarm_second();
    let agents = swarm_agents();
    for per_update in [1, 50, 1024] {
        let mut stage = stage();
        seed(&mut stage);
        stage.apply([UiCommand::SetAgents(agents.clone())]);
        steps(&mut stage);
        stage.apply([UiCommand::StartStreaming]);
        let (mut updates, mut frames) = swarm_updates(&mut stage, &second, per_update);
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
