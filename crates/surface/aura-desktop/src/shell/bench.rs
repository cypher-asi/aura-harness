//! The window's timings on the scenes of [`super::snapshot`]:
//! `cargo test --release -p aura-desktop -- --ignored --nocapture timings`.
//!
//! The window applies what happens when it happens and renders once per
//! frame of the display, so updates are reported with the frames they
//! lead to.

use super::keys::Press;
use super::snapshot::{agent, report, seed, steps};
use super::tests::stage;
use crate::demo::{LISTING, REPLY};
use crate::geom::Size;
use crate::raster;
use crate::text;
use aura_ui::events::{ActivityData, MessageData, MessageRole, ToolData, UiCommand};
use std::time::{Duration, Instant};

/// Frames a display shows in a second.
const REFRESH: usize = 60;

fn sum(samples: &[Duration]) -> f64 {
    samples.iter().sum::<Duration>().as_secs_f64() * 1e3
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

/// One second of a swarm streaming at once: the agent in front writes a
/// reply while the others think, call tools and write theirs. The
/// commands arrive one at a time, about a frame's worth at a time, and
/// all at once, with the frames a display shows in that second.
#[test]
#[ignore = "benchmark"]
fn swarm_timings() {
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

/// A conversation of `messages` messages: a prompt, two tool calls and a
/// reply, over and over.
fn history(messages: usize) -> Vec<MessageData> {
    let tool = |turn: usize, name: &str| MessageData {
        activity: Some(ActivityData::Tool {
            id: format!("t{turn}-{name}"),
            name: name.into(),
            args: r#"{"path":"crates"}"#.into(),
            result: Some(LISTING.into()),
            is_error: false,
            duration_ms: Some(120),
            policy: None,
        }),
        ..MessageData::new(MessageRole::Assistant, "")
    };
    let mut out = Vec::with_capacity(messages);
    while out.len() < messages {
        let turn = out.len() / 4;
        let prompt = format!("Summarise the crates and fix the README, pass {turn}");
        let mut reply = MessageData::new(
            MessageRole::Assistant,
            REPLY.replace("{prompt}", "the crates"),
        );
        reply.model = Some("local/qwen3-coder".into());
        out.extend([
            MessageData::new(MessageRole::User, prompt),
            tool(turn, "read_file"),
            tool(turn, "list_files"),
            reply,
        ]);
    }
    out.truncate(messages);
    out
}

/// Two hundred turns of a question and a long answer, no two rows alike
/// as in a real conversation.
fn long_chat() -> Vec<MessageData> {
    (0..200)
        .flat_map(|turn| {
            let reply: String = (0..12)
                .map(|k| {
                    format!(
                        "Turn {turn}, point {k}: the kernel records step {k} of turn {turn} \
                         and checks it against the policy before it runs.\n\n"
                    )
                })
                .collect();
            [
                MessageData::new(MessageRole::User, format!("Question number {turn}")),
                MessageData::new(MessageRole::Assistant, reply),
            ]
        })
        .collect()
}

/// The middle of `pane`'s body on the window, in pixels.
#[allow(clippy::cast_precision_loss)]
fn inside(stage: &super::tests::Stage, pane: aura_ui_terminal::Pane) -> crate::geom::Point {
    let shell = &stage.shell;
    let chrome = shell.tabs.front().tui.pane_chrome();
    let area = chrome
        .iter()
        .find(|c| c.pane == pane)
        .expect("the panel is drawn")
        .area;
    let origin = shell.metrics.origin(shell.grid_area(), shell.cols);
    let cell = shell.metrics.cell;
    let bar = crate::tab_bar::height(&shell.metrics);
    crate::geom::Point::new(
        origin.x + (f32::from(area.x) + f32::from(area.width) / 2.0) * cell.width,
        bar + origin.y + (f32::from(area.y) + f32::from(area.height) / 2.0) * cell.height,
    )
}

/// Scrolling with a lot on screen: a long conversation, and hundreds of
/// agents in the projects list. The chat scrolls a quarter of a row at
/// a time, as a touchpad does, and a wheel's notch at a time; the
/// projects list a notch at a time. A frame follows each.
#[test]
#[ignore = "benchmark"]
fn scroll_timings() {
    use aura_ui_terminal::Pane;
    use winit::dpi::PhysicalPosition;
    use winit::event::MouseScrollDelta;

    let mut stage = stage();
    seed(&mut stage);
    let mut agents = Vec::new();
    for project in 0..40 {
        let name = format!("project-{project:02}");
        agents.push(agent(
            &format!("r{project}"),
            &format!("Lead{project:02}"),
            0,
            Some(&name),
        ));
        for i in 0..9 {
            let id = format!("r{project}-{i}");
            agents.push(agent(
                &id,
                &format!("Helper{project:02}-{i}"),
                1 + i % 2,
                None,
            ));
        }
    }
    agents[0].id = "a1".into();
    stage.apply([
        UiCommand::SetAgents(agents),
        UiCommand::SetActiveAgent("a1".into()),
        UiCommand::LoadConversation(long_chat()),
    ]);

    let run = |stage: &mut super::tests::Stage, label: &str, delta: MouseScrollDelta| {
        let (mut frames, mut built) = (Vec::new(), 0);
        for _ in 0..160 {
            stage.now += Duration::from_millis(16);
            let t = Instant::now();
            stage.shell.wheel(delta, stage.now);
            stage.shell.frame(stage.now);
            frames.push(t.elapsed());
            built += stage.shell.painter.built();
        }
        report(label, &mut frames);
        println!(
            "  pieces painted anew: {built} over {} frames",
            frames.len()
        );
    };

    let chat = inside(&stage, Pane::Chat);
    stage.shell.pointer_moved(chat);
    stage.shell.frame(stage.now);
    let quarter = f64::from(stage.shell.metrics.cell.height / super::glide::TOUCHPAD_GAIN) / 4.0;
    let touchpad = MouseScrollDelta::PixelDelta(PhysicalPosition::new(0.0, quarter));
    run(&mut stage, "chat, a quarter row up and its frame", touchpad);
    run(
        &mut stage,
        "chat, a notch up and its frame",
        MouseScrollDelta::LineDelta(0.0, 1.0),
    );

    let projects = inside(&stage, Pane::Agents);
    stage.shell.pointer_moved(projects);
    stage.shell.frame(stage.now);
    run(
        &mut stage,
        "projects list, a notch down and its frame",
        MouseScrollDelta::LineDelta(0.0, -1.0),
    );
}

/// A second of the window's loop with thirty agents streaming behind a
/// long chat. What arrives is applied when it arrives, a frame is drawn
/// whenever the shell asks for one, and a frame unlike the one on screen
/// waits for the display's next refresh before the loop goes on, as the
/// window's does. Once while a touchpad scrolls the chat the whole
/// second, and once with the chat left alone.
#[test]
#[ignore = "benchmark"]
fn swarm_scroll_timings() {
    use aura_ui_terminal::Pane;
    use winit::dpi::PhysicalPosition;
    use winit::event::MouseScrollDelta;

    const AGENTS: u32 = 30;
    /// How often an agent's forwarder sends what it has.
    const FLUSH: Duration = Duration::from_millis(33);
    /// How often a touchpad reports scrolling.
    const TOUCHPAD: Duration = Duration::from_millis(8);
    const SECOND: Duration = Duration::from_secs(1);

    enum Arrival {
        Command(Box<UiCommand>),
        Scroll,
    }

    let refresh = SECOND / u32::try_from(REFRESH).unwrap_or(60);
    let reply: Vec<char> = REPLY.replace("{prompt}", "the crates").chars().collect();
    let mut chunks = reply.chunks(12).cycle();
    let mut agents = vec![agent("a1", "Agent01", 0, Some("aura-harness"))];
    agents
        .extend((1..=AGENTS).map(|i| agent(&format!("b{i:02}"), &format!("Agent{i:02}"), 1, None)));
    let conversation = long_chat();

    for scrolling in [true, false] {
        let mut arrivals = Vec::new();
        for i in 1..=AGENTS {
            let (mut at, mut step) = (FLUSH * i / AGENTS, 0);
            while at < SECOND {
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
                let command = UiCommand::ForAgent {
                    agent_id: format!("b{i:02}"),
                    command: Box::new(command),
                };
                arrivals.push((at, Arrival::Command(Box::new(command))));
                at += FLUSH;
                step += 1;
            }
        }
        if scrolling {
            let mut at = Duration::ZERO;
            while at < SECOND {
                arrivals.push((at, Arrival::Scroll));
                at += TOUCHPAD;
            }
        }
        arrivals.sort_by_key(|(at, _)| *at);

        let mut stage = stage();
        seed(&mut stage);
        stage.apply([
            UiCommand::SetAgents(agents.clone()),
            UiCommand::SetActiveAgent("a1".into()),
            UiCommand::LoadConversation(conversation.clone()),
        ]);
        stage.shell.pointer_moved(inside(&stage, Pane::Chat));
        let mut shown = stage.shell.frame(stage.now).clone();
        let quarter =
            f64::from(stage.shell.metrics.cell.height / super::glide::TOUCHPAD_GAIN) / 4.0;
        let touchpad = MouseScrollDelta::PixelDelta(PhysicalPosition::new(0.0, quarter));

        let start = stage.now;
        let (mut clock, mut next) = (Duration::ZERO, 0);
        let (mut busy, mut same, mut frames, mut shown_at) =
            (Duration::ZERO, 0, Vec::new(), Vec::new());
        while clock < SECOND {
            let now = start + clock;
            let t = Instant::now();
            let mut batch = Vec::new();
            while next < arrivals.len() && arrivals[next].0 <= clock {
                match &arrivals[next].1 {
                    Arrival::Command(command) => batch.push((**command).clone()),
                    Arrival::Scroll => stage.shell.wheel(touchpad, now),
                }
                next += 1;
            }
            if !batch.is_empty() {
                stage.shell.commands(batch);
            }
            stage.shell.advance(now);
            let mut spent = t.elapsed();
            let mut fresh = false;
            if stage.shell.dirty() {
                let t = Instant::now();
                let scene = stage.shell.frame(now);
                fresh = *scene != shown;
                if fresh {
                    shown.clone_from(scene);
                } else {
                    same += 1;
                }
                frames.push(t.elapsed());
                spent += t.elapsed();
            }
            busy += spent;
            clock += spent;
            if fresh {
                // Shown at the display's next refresh, which the loop
                // waits for.
                shown_at.push(clock);
                let refreshes = clock.as_nanos() / refresh.as_nanos() + 1;
                clock = refresh * u32::try_from(refreshes).unwrap_or(u32::MAX);
            } else if !stage.shell.dirty() {
                let arrival = arrivals.get(next).map_or(SECOND, |(at, _)| *at);
                let deadline = stage
                    .shell
                    .deadline(start + clock)
                    .saturating_duration_since(start);
                clock = clock.max(arrival.min(deadline));
            }
        }

        let longest = shown_at
            .windows(2)
            .map(|pair| pair[1] - pair[0])
            .max()
            .unwrap_or_default();
        let label = if scrolling {
            "chat scrolled by a touchpad"
        } else {
            "chat left alone"
        };
        println!(
            "{AGENTS} agents streaming, {label}: {} frames drawn ({same} unchanged, {} shown), \
             busy {:.1} ms of the second, longest wait between shown frames {:.1} ms",
            frames.len(),
            shown_at.len(),
            busy.as_secs_f64() * 1e3,
            longest.as_secs_f64() * 1e3,
        );
        report("  frame", &mut frames);
    }
}

/// The window with the most messages the UI keeps, and with a twentieth
/// of that: the first frame, which lays every message out, a key and
/// its frame, a frame after the window changed width, which lays them
/// all out again, and a reply streaming in small chunks with a frame
/// after each, as it grows.
#[test]
#[ignore = "benchmark"]
fn history_timings() {
    let ms = |d: Duration| d.as_secs_f64() * 1e3;
    for messages in [100, 2000] {
        let mut stage = stage();
        seed(&mut stage);
        stage.shell.frame(stage.now);
        stage
            .shell
            .commands([UiCommand::LoadConversation(history(messages))]);
        let t = Instant::now();
        stage.shell.frame(stage.now);
        println!("{messages} messages: first frame {:.2} ms", ms(t.elapsed()));

        let mut typing = Vec::new();
        let mut buffer = [0; 4];
        for c in "how are the crates tested?".chars() {
            let key = Press::typing(c.encode_utf8(&mut buffer));
            let t = Instant::now();
            stage.shell.key_pressed(&key, stage.now);
            stage.shell.frame(stage.now);
            typing.push(t.elapsed());
        }
        report("  key and its frame", &mut typing);

        let mut widths = Vec::new();
        for step in 1..=10_u8 {
            let wide = 1320.0 - 12.0 * f32::from(step);
            let t = Instant::now();
            stage.shell.resized(Size::new(wide, 860.0), 1.0);
            stage.shell.frame(stage.now);
            widths.push(t.elapsed());
        }
        report("  frame at a new width", &mut widths);

        stage.apply([UiCommand::StartStreaming]);
        let text: Vec<char> = REPLY
            .replace("{prompt}", "the crates")
            .repeat(160)
            .chars()
            .collect();
        let marks = [2_000, 10_000, 40_000, usize::MAX];
        let (mut mark, mut sent) = (0, 0);
        let mut frames = Vec::new();
        for chunk in text.chunks(12) {
            let t = Instant::now();
            stage
                .shell
                .commands([UiCommand::AppendText(chunk.iter().collect())]);
            stage.shell.frame(stage.now);
            frames.push(t.elapsed());
            sent += chunk.len();
            if sent >= marks[mark] {
                let label = format!(
                    "  streamed chunk and its frame, up to {} characters",
                    marks[mark]
                );
                report(&label, &mut frames);
                frames.clear();
                mark += 1;
            }
        }
        let label = format!("  streamed chunk and its frame, up to {sent} characters");
        report(&label, &mut frames);
    }
}
