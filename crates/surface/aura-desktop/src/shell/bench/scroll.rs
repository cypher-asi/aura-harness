//! Scrolling with a lot on screen, alone and with a swarm streaming.

use super::{agent, helper_command, report, seed, stage, Stage, REFRESH};
use crate::demo::REPLY;
use crate::scene::Scene;
use aura_ui::events::{AgentSummary, MessageData, MessageRole, UiCommand};
use aura_ui_terminal::Pane;
use std::time::{Duration, Instant};
use winit::dpi::PhysicalPosition;
use winit::event::MouseScrollDelta;

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
fn inside(stage: &Stage, pane: Pane) -> crate::geom::Point {
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

/// Forty projects of a lead and nine helpers each.
fn project_agents() -> Vec<AgentSummary> {
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
    agents
}

/// A hundred and sixty scrolls by `delta`, a frame after each.
fn scroll_run(stage: &mut Stage, label: &str, delta: MouseScrollDelta) {
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
}

/// A touchpad's report of a quarter of a row.
fn quarter_row(stage: &Stage) -> MouseScrollDelta {
    let quarter =
        f64::from(stage.shell.metrics.cell.height / crate::shell::glide::TOUCHPAD_GAIN) / 4.0;
    MouseScrollDelta::PixelDelta(PhysicalPosition::new(0.0, quarter))
}

/// Scrolling with a lot on screen: a long conversation, and hundreds of
/// agents in the projects list. The chat scrolls a quarter of a row at
/// a time, as a touchpad does, and a wheel's notch at a time; the
/// projects list a notch at a time. A frame follows each.
#[test]
#[ignore = "benchmark"]
fn scroll_timings() {
    let mut stage = stage();
    seed(&mut stage);
    stage.apply([
        UiCommand::SetAgents(project_agents()),
        UiCommand::SetActiveAgent("a1".into()),
        UiCommand::LoadConversation(long_chat()),
    ]);

    let chat = inside(&stage, Pane::Chat);
    stage.shell.pointer_moved(chat);
    stage.shell.frame(stage.now);
    let touchpad = quarter_row(&stage);
    scroll_run(&mut stage, "chat, a quarter row up and its frame", touchpad);
    scroll_run(
        &mut stage,
        "chat, a notch up and its frame",
        MouseScrollDelta::LineDelta(0.0, 1.0),
    );

    let projects = inside(&stage, Pane::Agents);
    stage.shell.pointer_moved(projects);
    stage.shell.frame(stage.now);
    scroll_run(
        &mut stage,
        "projects list, a notch down and its frame",
        MouseScrollDelta::LineDelta(0.0, -1.0),
    );
}

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

/// What arrives in the second, in order: each agent's commands every
/// flush, and a touchpad's reports when `scrolling`.
fn arrivals<'a>(
    scrolling: bool,
    chunks: &mut impl Iterator<Item = &'a [char]>,
) -> Vec<(Duration, Arrival)> {
    let mut arrivals = Vec::new();
    for i in 1..=AGENTS {
        let (mut at, mut step) = (FLUSH * i / AGENTS, 0);
        while at < SECOND {
            let command = helper_command(step, chunks);
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
    arrivals
}

/// What the second of the loop came to.
struct Second {
    busy: Duration,
    same: usize,
    frames: Vec<Duration>,
    shown_at: Vec<Duration>,
}

/// The window's loop over a second of `arrivals`, `shown` on screen.
fn run_second(
    stage: &mut Stage,
    arrivals: &[(Duration, Arrival)],
    mut shown: Scene,
    touchpad: MouseScrollDelta,
) -> Second {
    let refresh = SECOND / u32::try_from(REFRESH).unwrap_or(60);
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
            clock = shown_at_refresh(clock, refresh, &mut shown_at);
        } else if !stage.shell.dirty() {
            clock = idle_until(stage, arrivals.get(next), start, clock);
        }
    }
    Second {
        busy,
        same,
        frames,
        shown_at,
    }
}

/// A fresh frame is shown at the display's next refresh, which the loop
/// waits for.
fn shown_at_refresh(clock: Duration, refresh: Duration, shown_at: &mut Vec<Duration>) -> Duration {
    shown_at.push(clock);
    let refreshes = clock.as_nanos() / refresh.as_nanos() + 1;
    refresh * u32::try_from(refreshes).unwrap_or(u32::MAX)
}

/// An idle loop sleeps until the next arrival or the shell's deadline.
fn idle_until(
    stage: &Stage,
    arrival: Option<&(Duration, Arrival)>,
    start: Instant,
    clock: Duration,
) -> Duration {
    let arrival = arrival.map_or(SECOND, |(at, _)| *at);
    let deadline = stage
        .shell
        .deadline(start + clock)
        .saturating_duration_since(start);
    clock.max(arrival.min(deadline))
}

fn print_second(scrolling: bool, second: &mut Second) {
    let longest = second
        .shown_at
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
        "{AGENTS} agents streaming, {label}: {} frames drawn ({} unchanged, {} shown), \
         busy {:.1} ms of the second, longest wait between shown frames {:.1} ms",
        second.frames.len(),
        second.same,
        second.shown_at.len(),
        second.busy.as_secs_f64() * 1e3,
        longest.as_secs_f64() * 1e3,
    );
    report("  frame", &mut second.frames);
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
    let reply: Vec<char> = REPLY.replace("{prompt}", "the crates").chars().collect();
    let mut chunks = reply.chunks(12).cycle();
    let mut agents = vec![agent("a1", "Agent01", 0, Some("aura-harness"))];
    agents
        .extend((1..=AGENTS).map(|i| agent(&format!("b{i:02}"), &format!("Agent{i:02}"), 1, None)));
    let conversation = long_chat();

    for scrolling in [true, false] {
        let arrivals = arrivals(scrolling, &mut chunks);
        let mut stage = stage();
        seed(&mut stage);
        stage.apply([
            UiCommand::SetAgents(agents.clone()),
            UiCommand::SetActiveAgent("a1".into()),
            UiCommand::LoadConversation(conversation.clone()),
        ]);
        stage.shell.pointer_moved(inside(&stage, Pane::Chat));
        let shown = stage.shell.frame(stage.now).clone();
        let touchpad = quarter_row(&stage);
        let mut second = run_second(&mut stage, &arrivals, shown, touchpad);
        print_second(scrolling, &mut second);
    }
}
