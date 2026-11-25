//! Where the time goes when a chat is loaded: applying the history the
//! session sends, laying out its first frame, the frames after it, and
//! switching between two chats already held. Ignored by default; run with
//!
//! ```text
//! cargo test --release -p aura-ui-terminal --test chat_load_perf -- --ignored --nocapture
//! ```
//!
//! `AURA_PERF_ENTRIES` lists the chat sizes to try (default `300,1000`).
//! The session sends at most `HISTORY_MESSAGE_LIMIT` (300) messages, so
//! that is the realistic load and 1000 the worst a caller could send.
//! Everything here goes through the public API, so it measures the same
//! paths the app does.

use aura_ui_terminal::components::{highlight_code_line, CodeBlock};
use aura_ui_terminal::events::{ActivityData, AgentSummary, CallUsage, MessageData, MessageRole};
use aura_ui_terminal::{render, App, Theme, UiCommand};
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use std::time::{Duration, Instant};

const WIDTH: u16 = 200;
const HEIGHT: u16 = 60;

fn env_list(name: &str, default: &[usize]) -> Vec<usize> {
    std::env::var(name).ok().map_or_else(
        || default.to_vec(),
        |v| v.split(',').filter_map(|n| n.trim().parse().ok()).collect(),
    )
}

fn id(i: usize) -> String {
    format!("{i:032x}")
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn print_row(stage: &str, d: Duration) {
    println!("{stage:<46} {:>10.2} ms", ms(d));
}

/// `bytes` in kilobytes, for the report lines.
#[allow(clippy::cast_precision_loss)] // a chat is far below 2^52 bytes
fn kb(bytes: usize) -> f64 {
    bytes as f64 / 1024.0
}

/// `a / b`, for the report lines.
#[allow(clippy::cast_precision_loss)] // both counts are small
fn ratio(a: usize, b: usize) -> f64 {
    a as f64 / b.max(1) as f64
}

/// How many threads the chat lays blocks out on: `MOST_LAYOUT_THREADS` (8)
/// at most.
#[allow(clippy::cast_possible_truncation)] // clamped to 8 just below
fn threads() -> u32 {
    std::thread::available_parallelism()
        .map_or(1, std::num::NonZeroUsize::get)
        .min(8) as u32
}

/// A long markdown reply: prose, three fenced code blocks and two tables.
fn reply(turn: usize) -> String {
    let mut text = format!(
        "## Turn {turn}: what changed\n\n\
         I moved the loader off the hot path and bounded the scan. The table \
         below is what each stage costs now, measured on this machine.\n\n\
         | stage | before | after |\n|---|---|---|\n\
         | scan_tail | 41 ms | 12 ms |\n\
         | markdown | 180 ms | 22 ms |\n\
         | wrap | 60 ms | 9 ms |\n\n\
         The first block is the scan itself, with the context kept at the \
         boundary so the error says which agent's record failed:\n\n"
    );
    for block in 0..3 {
        text.push_str("```rust\n");
        for line in 0..10 {
            text.push_str(&format!(
                "fn stage_{block}_{line}(input: &[u8], limits: &Limits) -> Result<usize, Error> {{\n    \
                 let parsed = parse(input, limits).context(\"stage read\")?;\n    \
                 let kept = parsed.iter().filter(|row| row.is_live()).count();\n    \
                 Ok(kept)\n}}\n"
            ));
        }
        text.push_str("```\n\n");
        text.push_str(&format!(
            "The block above is stage {block}. Its tests, and the one that \
             caught the off-by-one:\n\n\
             - `stage_replays_the_same_bytes`\n\
             - `stage_rejects_a_short_buffer`, the one that caught the off-by-one\n\
             - `stage_reports_the_offset`\n\n\
             | test | wall time |\n|---|---|\n\
             | replays | 0.4 ms |\n\
             | short buffer | 0.1 ms |\n\n"
        ));
    }
    text.push_str(
        "### Notes\n\n\
         The reply is long on purpose: every turn carries several thousand \
         bytes of prose, code and tables, so the markdown parse, the code \
         highlighting and the wrap all have real work to do. ",
    );
    text.push_str(&"This paragraph is repeated to push one reply past four kilobytes so a long session's history is megabytes, as the report says it is. ".repeat(5));
    text
}

/// A tool result of a few kilobytes, shaped like a diff and a test log.
fn tool_result(turn: usize) -> String {
    let mut text = String::from("--- a/crates/surface/aura-app/src/record_loader.rs\n+++ b/crates/surface/aura-app/src/record_loader.rs\n@@ -880,7 +880,7 @@\n");
    for line in 0..24 {
        text.push_str(&format!(
            "-    let from_seq = head_seq.saturating_sub(window.saturating_sub(1)).max(1);\n\
             +    let from_seq = head_seq.saturating_sub(window.saturating_sub(1)).max(1); // turn {turn}, line {line}\n"
        ));
    }
    for line in 0..20 {
        text.push_str(&format!(
            "test record_loader::tests::case_{turn}_{line} ... ok\n"
        ));
    }
    text.push_str("test result: ok. 44 passed; 0 failed; 0 ignored\n");
    text
}

fn write_args(turn: usize, call: usize) -> String {
    format!(
        "{{\"path\":\"crates/surface/aura-app/src/gen_{turn}_{call}.rs\",\"content\":\"{}\"}}",
        "pub fn generated() -> usize { 42 }\n".repeat(40)
    )
}

/// `entries` chat entries: prompts, thinking blocks, tool calls with large
/// results, and long markdown replies, over as many turns as it takes.
fn conversation(entries: usize) -> Vec<MessageData> {
    let mut out: Vec<MessageData> = Vec::with_capacity(entries);
    let mut ts = 1_790_000_000_000u64;
    let mut turn = 0usize;
    while out.len() < entries {
        let push = |out: &mut Vec<MessageData>, mut data: MessageData, ts: &mut u64| {
            *ts += 1_000;
            data.timestamp_ms = Some(*ts);
            out.push(data);
        };
        push(
            &mut out,
            MessageData::new(
                MessageRole::User,
                format!("Load pass {turn}: measure where the time goes and report the stages."),
            ),
            &mut ts,
        );
        for thought in 0..3 {
            push(
                &mut out,
                MessageData {
                    activity: Some(ActivityData::Thinking {
                        text: format!(
                            "The chat is slow to open. Pass {turn}, thought {thought}: the \
                             history is read, then every block is laid out. Measuring the \
                             stages tells them apart, so I time the scan, the message \
                             rebuild and the first frame separately. {}",
                            "Then the markdown parse, the code highlight and the wrap are \
                             each timed on their own. "
                                .repeat(2)
                        ),
                        duration_ms: Some(4_100),
                    }),
                    ..MessageData::new(MessageRole::Assistant, "")
                },
                &mut ts,
            );
        }
        for call in 0..12 {
            push(
                &mut out,
                MessageData {
                    activity: Some(ActivityData::Tool {
                        id: format!("t{turn}-{call}"),
                        name: if call % 4 == 3 {
                            "write_file"
                        } else {
                            "read_file"
                        }
                        .to_string(),
                        args: write_args(turn, call),
                        result: Some(tool_result(turn)),
                        is_error: false,
                        duration_ms: Some(120),
                        policy: Some("allowed by user.on".to_string()),
                    }),
                    ..MessageData::new(MessageRole::Assistant, "")
                },
                &mut ts,
            );
        }
        for part in 0..4 {
            let mut data = MessageData::new(MessageRole::Assistant, reply(turn * 4 + part));
            data.model = Some("claude-sonnet-5".to_string());
            data.usage = Some(CallUsage {
                tokens: 41_800,
                cost_usd: Some(0.061),
                unpriced_calls: 0,
            });
            push(&mut out, data, &mut ts);
        }
        turn += 1;
    }
    out.truncate(entries);
    out
}

fn agents() -> Vec<AgentSummary> {
    (0..2)
        .map(|i| AgentSummary {
            id: id(i),
            name: format!("Agent {i}"),
            is_active: i == 0,
            ..AgentSummary::default()
        })
        .collect()
}

struct Bench {
    app: App,
    theme: Theme,
    terminal: Terminal<TestBackend>,
}

impl Bench {
    fn new() -> Self {
        let mut app = App::new();
        app.process_command(UiCommand::SetAgents(agents()));
        app.process_command(UiCommand::SetActiveAgent(id(0)));
        Self {
            app,
            theme: Theme::cyber(),
            terminal: Terminal::new(TestBackend::new(WIDTH, HEIGHT)).unwrap(),
        }
    }

    fn frame(&mut self) -> Duration {
        let start = Instant::now();
        self.terminal
            .draw(|f| render(f, &mut self.app, &self.theme))
            .unwrap();
        start.elapsed()
    }

    fn load(&mut self, history: Vec<MessageData>) -> Duration {
        let start = Instant::now();
        self.app
            .process_command(UiCommand::LoadConversation(history));
        start.elapsed()
    }

    fn load_for(&mut self, agent: usize, history: Vec<MessageData>) {
        self.app.process_command(UiCommand::ForAgent {
            agent_id: id(agent),
            command: Box::new(UiCommand::LoadConversation(history)),
        });
    }
}

/// One chat's first-frame cost in a fresh app, the cheapest of `runs`: the
/// chat-line layout runs across the rayon pool, so a single run moves by
/// hundreds of milliseconds.
fn fresh_load(history: &[MessageData], runs: usize) -> (Duration, Duration, Duration) {
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

fn main_bench(entries: usize) {
    let history = conversation(entries);
    let bytes: usize = history.iter().map(|m| m.content.len()).sum();
    println!(
        "\n{entries} entries, {} steps, {:.1} KB of text, {WIDTH}x{HEIGHT}",
        history.iter().filter(|m| m.activity.is_some()).count(),
        kb(bytes),
    );
    println!("cheapest of 3 fresh apps per row:\n");

    let (apply, first, after) = fresh_load(&history, 3);
    print_row("(a) LoadConversation (apply only)", apply);
    print_row("(b) first frame (every block laid out)", first);
    print_row("(c) second frame (nothing changed)", after);

    // Switching between two chats already held in memory.
    let mut bench = Bench::new();
    bench.load_for(0, history.clone());
    bench.load_for(1, conversation(entries));
    bench.frame();
    let mut switches = Vec::new();
    let mut switch_frames = Vec::new();
    for round in 0..6 {
        let other = if round % 2 == 0 { 1 } else { 0 };
        let start = Instant::now();
        bench
            .app
            .process_command(UiCommand::SetActiveAgent(id(other)));
        switches.push(start.elapsed());
        switch_frames.push(bench.frame());
    }
    switches.sort();
    switch_frames.sort();
    print_row("(d) chat switch: SetActiveAgent", switches[0]);
    print_row(
        "(d) chat switch: frame after it (cache kept)",
        switch_frames[0],
    );

    // A switch to a chat never shown: it is laid out from nothing.
    let mut bench = Bench::new();
    bench.load_for(0, history.clone());
    bench.frame();
    bench.load_for(1, history.clone());
    let start = Instant::now();
    bench.app.process_command(UiCommand::SetActiveAgent(id(1)));
    let command = start.elapsed();
    let frame = bench.frame();
    print_row("switch to a chat never laid out: command", command);
    print_row("switch to a chat never laid out: frame", frame);
}

/// The first frame's cost of `history`, the cheapest of `runs` fresh apps.
fn first_frame(history: &[MessageData], runs: usize) -> Duration {
    let mut best = Duration::MAX;
    for _ in 0..runs {
        let mut bench = Bench::new();
        bench.load(history.to_vec());
        best = best.min(bench.frame());
    }
    best
}

/// Which kind of content the first frame's cost comes from: the whole
/// conversation, then each kind on its own, at the same entry count. Each
/// is the cheapest of three runs, since the order alone moves a run by
/// hundreds of milliseconds here.
fn breakdown(entries: usize) {
    let full = conversation(entries);
    let plain = |m: &MessageData| m.activity.is_none() && m.role == MessageRole::User;
    let reply = |m: &MessageData| m.activity.is_none() && m.role == MessageRole::Assistant;
    let prompts: Vec<MessageData> = full.iter().filter(|m| plain(m)).cloned().collect();
    let replies: Vec<MessageData> = full.iter().filter(|m| reply(m)).cloned().collect();
    let tools: Vec<MessageData> = full
        .iter()
        .filter(|m| m.activity.is_some())
        .cloned()
        .collect();
    println!(
        "\nfirst frame by content kind (cheapest of 3): {} prompts, {} replies, {} steps",
        prompts.len(),
        replies.len(),
        tools.len()
    );
    for (label, history) in [
        ("all entries", &full),
        ("assistant replies only", &replies),
        ("tool/thinking steps only", &tools),
        ("user prompts only", &prompts),
    ] {
        print_row(&format!("first frame, {label}"), first_frame(history, 3));
    }

    // The same replies with parts of the markdown taken out, so the parse,
    // the fences and the tables can be told apart.
    type Edit = fn(&str) -> String;
    let variants: [(&str, Edit); 4] = [
        ("replies, no fenced code, no tables", |text| {
            drop_tables(&strip_fences(text))
        }),
        ("replies, no tables", drop_tables),
        ("replies, no fenced code", strip_fences),
        ("replies, nothing taken out", str::to_string),
    ];
    for (label, edit) in variants {
        let history: Vec<MessageData> = replies
            .iter()
            .map(|m| MessageData::new(MessageRole::Assistant, edit(&m.content)))
            .collect();
        print_row(&format!("first frame, {label}"), first_frame(&history, 3));
    }
}

/// What a fenced code block costs on its own, laid out as the chat lays it
/// out: the highlighter against the wrap, and the parse against the
/// token-to-colour mapping.
fn code_breakdown(entries: usize) {
    let replies: Vec<MessageData> = conversation(entries)
        .into_iter()
        .filter(|m| m.activity.is_none() && m.role == MessageRole::Assistant)
        .collect();
    let theme = Theme::cyber();
    // The first render is cold: it loads the syntax set and the theme.
    let blocks: Vec<String> = replies
        .iter()
        .flat_map(|m| fenced_code(&m.content))
        .collect();
    let lines: usize = blocks.iter().map(|code| code.lines().count()).sum();
    let bytes: usize = blocks.iter().map(String::len).sum();
    println!(
        "\n{entries} entries: {} fenced rust blocks, {lines} lines, {:.1} KB of code",
        blocks.len(),
        kb(bytes),
    );
    let start = Instant::now();
    let cold = CodeBlock::new("rust", blocks[0].clone())
        .render(&theme, 150)
        .len();
    print_row(
        "CodeBlock::render, cold (loads the syntax set)",
        start.elapsed(),
    );
    assert!(cold > 0);
    let start = Instant::now();
    let rendered: usize = blocks
        .iter()
        .map(|code| {
            CodeBlock::new("rust", code.clone())
                .render(&theme, 150)
                .len()
        })
        .sum();
    print_row("CodeBlock::render, warm, highlighted", start.elapsed());
    let start = Instant::now();
    let plain: usize = blocks
        .iter()
        .map(|code| CodeBlock::new("", code.clone()).render(&theme, 150).len())
        .sum();
    print_row("CodeBlock::render, warm, no language", start.elapsed());
    assert_eq!(plain, rendered);

    // Just the highlighting, line by line, the way a block does it.
    let start = Instant::now();
    let mut span_count = 0usize;
    for code in &blocks {
        for line in code.lines() {
            span_count += highlight_code_line("rust", line, &theme).len();
        }
    }
    print_row("highlight_code_line, warm, every line", start.elapsed());
    println!(
        "    {span_count} spans over {bytes} bytes, {:.1} bytes per span",
        ratio(bytes, span_count)
    );

    // The number to plan with: how long a kilobyte of fenced rust costs.
    let start = Instant::now();
    let n: usize = blocks
        .iter()
        .map(|code| {
            CodeBlock::new("rust", code.clone())
                .render(&theme, 150)
                .len()
        })
        .sum();
    let elapsed = start.elapsed();
    assert!(n > 0);
    let per_kb = ms(elapsed) / kb(bytes);
    println!(
        "    {per_kb:.2} ms per KB of fenced rust, one thread; {:.2} ms across {} threads",
        per_kb / f64::from(threads()),
        threads()
    );

    // The same lines under other grammars: the parse is grammar-specific,
    // the token-to-colour mapping is not, so a cheaper grammar costing much
    // less says the parse is where the time goes.
    for language in ["rust", "yaml", "json", "txt"] {
        let start = Instant::now();
        let n: usize = blocks
            .iter()
            .map(|code| {
                CodeBlock::new(language, code.clone())
                    .render(&theme, 150)
                    .len()
            })
            .sum();
        print_row(
            &format!("the same lines parsed as {language}"),
            start.elapsed(),
        );
        assert!(n > 0);
    }

    // Bytes against tokens: lines with one token each, then the same bytes
    // in many small tokens. Costing the same says the time goes by byte of
    // code parsed, not by span produced.
    let wide: String = (0..10_000)
        .map(|i| format!("identifier_number_{i}\n"))
        .collect();
    let dense: String = (0..10_000)
        .map(|i| format!("let a{i} = f(b{i}, c{i})?;\n"))
        .collect();
    println!(
        "\n{} bytes one token a line, {} bytes many tokens a line",
        wide.len(),
        dense.len()
    );
    for (label, code) in [("one token a line", wide), ("many tokens a line", dense)] {
        let start = Instant::now();
        let n = CodeBlock::new("rust", code).render(&theme, 150).len();
        print_row(&format!("10k lines, {label}"), start.elapsed());
        assert!(n > 0);
    }

    // What the fix would put on the first frame: the blocks of the last
    // screenful, which is all the panel can show, against all of them.
    let per_reply = blocks.len() / replies.len().max(1);
    let visible_replies = HEIGHT as usize / 4;
    let window: Vec<&String> = blocks
        .iter()
        .skip(blocks.len().saturating_sub(visible_replies * per_reply))
        .collect();
    let start = Instant::now();
    let n: usize = window
        .iter()
        .map(|code| {
            CodeBlock::new("rust", (*code).clone())
                .render(&theme, 150)
                .len()
        })
        .sum();
    print_row(
        &format!(
            "only the last {visible_replies} replies' code ({} blocks)",
            window.len()
        ),
        start.elapsed(),
    );
    assert!(n > 0);
}

/// `text` with every fenced code block replaced by a blank line.
fn strip_fences(text: &str) -> String {
    let mut out = String::new();
    let mut in_fence = false;
    for line in text.lines() {
        if line.starts_with("```") {
            in_fence = !in_fence;
            out.push('\n');
            continue;
        }
        if !in_fence {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// `text` with the rows of every markdown table taken out.
fn drop_tables(text: &str) -> String {
    text.lines()
        .filter(|line| !line.starts_with('|'))
        .map(|line| format!("{line}\n"))
        .collect()
}

/// The code of every fenced block in `text`.
fn fenced_code(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current: Option<String> = None;
    for line in text.lines() {
        if line.starts_with("```") {
            match current.take() {
                Some(code) => out.push(code),
                None => current = Some(String::new()),
            }
            continue;
        }
        if let Some(code) = &mut current {
            code.push_str(line);
            code.push('\n');
        }
    }
    out
}

#[test]
#[ignore = "load test; run with --ignored --nocapture"]
fn chat_load_perf() {
    // The backend sends at most 300 messages (`HISTORY_MESSAGE_LIMIT`), so
    // that is the realistic load; 1000 is the worst a caller could send.
    for entries in env_list("AURA_PERF_ENTRIES", &[300, 1000]) {
        main_bench(entries);
        breakdown(entries);
    }
    code_breakdown(300);
}
