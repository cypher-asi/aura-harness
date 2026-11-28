//! The UI with many agents streaming at once: how long applying a frame's
//! commands and drawing it take, and how often it redraws once they stop
//! streaming but keep running. Ignored by default; run with
//! `cargo test --release -p aura-ui-terminal --test it ui_load:: -- --ignored --nocapture`.
//!
//! `AURA_LOAD_AGENTS` lists the agent counts to try (default `1,8,32,64`)
//! and `AURA_LOAD_FRAMES` the frames each streams for (default `300`).

use aura_ui_terminal::events::{AgentSummary, ToolData};
use aura_ui_terminal::{render, App, Theme, UiCommand};
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use std::time::{Duration, Instant};

const WIDTH: u16 = 160;
const HEIGHT: u16 = 48;

fn env_list(name: &str, default: &[usize]) -> Vec<usize> {
    std::env::var(name).ok().map_or_else(
        || default.to_vec(),
        |v| v.split(',').filter_map(|n| n.trim().parse().ok()).collect(),
    )
}

fn env_num(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(default)
}

fn id(i: usize) -> String {
    format!("{i:032x}")
}

fn agents(count: usize) -> Vec<AgentSummary> {
    (0..count)
        .map(|i| AgentSummary {
            id: id(i),
            name: format!("Agent {i}"),
            is_active: i == 0,
            running: true,
            ..Default::default()
        })
        .collect()
}

/// `command` for agent `i`: as is for the agent on screen, else wrapped.
fn to(i: usize, command: UiCommand) -> UiCommand {
    if i == 0 {
        command
    } else {
        UiCommand::ForAgent {
            agent_id: id(i),
            command: Box::new(command),
        }
    }
}

/// What agent `i` sends in `frame`: a chunk of prose with a code fence now
/// and then, and every 40 frames a tool call whose arguments stream in.
fn frame_commands(i: usize, frame: usize) -> Vec<UiCommand> {
    let mut out = Vec::new();
    match frame % 40 {
        0 => out.push(to(i, UiCommand::StartStreaming)),
        30 => {
            out.push(to(i, UiCommand::FinishStreaming));
            out.push(to(
                i,
                UiCommand::ShowTool(ToolData {
                    id: format!("tool-{i}-{frame}"),
                    name: "write_file".to_string(),
                    args: String::new(),
                }),
            ));
        }
        31..=37 => {
            let args = format!(
                "{{\"path\":\"src/lib.rs\",\"content\":\"{}\"",
                "fn main() {} ".repeat((frame % 40 - 30) * 40)
            );
            out.push(to(
                i,
                UiCommand::UpdateToolArgs {
                    id: format!("tool-{i}-{}", frame - frame % 40 + 30),
                    args,
                },
            ));
        }
        38 => out.push(to(
            i,
            UiCommand::CompleteTool {
                id: format!("tool-{i}-{}", frame - frame % 40 + 30),
                result: "wrote 1 file".to_string(),
                success: true,
            },
        )),
        _ => {
            let text = if frame % 40 == 10 {
                "\n```rust\nfn handle(request: Request) -> Response {\n    let body = parse(request);\n"
                    .to_string()
            } else if frame % 40 == 20 {
                "    respond(body)\n}\n```\n".to_string()
            } else {
                format!("Some words about step {frame} of the work, and why it matters. ")
            };
            out.push(to(i, UiCommand::AppendText(text)));
        }
    }
    out
}

fn percentile(sorted: &[Duration], p: f64) -> Duration {
    if sorted.is_empty() {
        return Duration::ZERO;
    }
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    let index = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[index]
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn run(count: usize, frames: usize) {
    let mut terminal = Terminal::new(TestBackend::new(WIDTH, HEIGHT)).unwrap();
    let theme = Theme::cyber();
    let mut app = App::new();
    app.process_command(UiCommand::SetAgents(agents(count)));
    app.process_command(UiCommand::SetActiveAgent(id(0)));

    let mut apply = Vec::with_capacity(frames);
    let mut draw = Vec::with_capacity(frames);
    for frame in 0..frames {
        let start = Instant::now();
        for i in 0..count {
            for command in frame_commands(i, frame) {
                app.process_command(command);
            }
        }
        app.tick();
        apply.push(start.elapsed());
        if app.take_redraw() {
            let start = Instant::now();
            terminal.draw(|f| render(f, &mut app, &theme)).unwrap();
            draw.push(start.elapsed());
        }
    }

    // The agents keep running but send nothing: what the spinners cost.
    let idle_ticks = 100;
    let mut idle_draws = 0;
    let idle_start = Instant::now();
    for _ in 0..idle_ticks {
        app.tick();
        if app.take_redraw() {
            idle_draws += 1;
            terminal.draw(|f| render(f, &mut app, &theme)).unwrap();
        }
    }
    let idle = idle_start.elapsed();

    apply.sort();
    draw.sort();
    println!(
        "{count:>6} {:>10.3} {:>10.3} {:>10.3} {:>10.3} {:>8}/{idle_ticks} {:>10.1}",
        ms(percentile(&apply, 0.5)),
        ms(percentile(&apply, 0.99)),
        ms(percentile(&draw, 0.5)),
        ms(percentile(&draw, 0.99)),
        idle_draws,
        ms(idle),
    );
}

#[test]
#[ignore = "load test; run with --ignored --nocapture"]
fn ui_load() {
    let counts = env_list("AURA_LOAD_AGENTS", &[1, 8, 32, 64]);
    let frames = env_num("AURA_LOAD_FRAMES", 300);
    println!("{frames} frames, {WIDTH}x{HEIGHT}");
    println!(
        "{:>6} {:>10} {:>10} {:>10} {:>10} {:>12} {:>10}",
        "agents", "apply p50", "apply p99", "draw p50", "draw p99", "idle draws", "idle ms"
    );
    for count in counts {
        run(count, frames);
    }
}
