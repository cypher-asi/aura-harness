//! Loading one chat, switching between two, and which kind of content the
//! first frame's cost comes from.

use super::bench::{first_frame, fresh_load, Bench, HEIGHT, WIDTH};
use super::fixtures::{conversation, id};
use super::markdown::{drop_tables, strip_fences};
use super::report::{kb, print_row};
use aura_ui_terminal::events::{MessageData, MessageRole};
use aura_ui_terminal::UiCommand;
use std::time::Instant;

pub(crate) fn main_bench(entries: usize) {
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

/// Which kind of content the first frame's cost comes from: the whole
/// conversation, then each kind on its own, at the same entry count. Each
/// is the cheapest of three runs, since the order alone moves a run by
/// hundreds of milliseconds here.
pub(crate) fn breakdown(entries: usize) {
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
