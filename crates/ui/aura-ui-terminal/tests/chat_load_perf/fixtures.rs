//! The chats the load tests open: prompts, thinking blocks, tool calls with
//! large results and long markdown replies, and the agents that hold them.

use aura_ui_terminal::events::{ActivityData, AgentSummary, CallUsage, MessageData, MessageRole};

pub(crate) fn id(i: usize) -> String {
    format!("{i:032x}")
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

/// The user prompt that opens turn `turn`.
fn prompt(turn: usize) -> MessageData {
    MessageData::new(
        MessageRole::User,
        format!("Load pass {turn}: measure where the time goes and report the stages."),
    )
}

fn thinking(turn: usize, thought: usize) -> MessageData {
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
    }
}

fn tool_call(turn: usize, call: usize) -> MessageData {
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
    }
}

fn assistant_reply(turn: usize, part: usize) -> MessageData {
    let mut data = MessageData::new(MessageRole::Assistant, reply(turn * 4 + part));
    data.model = Some("claude-sonnet-5".to_string());
    data.usage = Some(CallUsage {
        tokens: 41_800,
        cost_usd: Some(0.061),
        unpriced_calls: 0,
    });
    data
}

/// Every entry of turn `turn`, in the order the chat shows them.
fn turn_entries(turn: usize) -> Vec<MessageData> {
    let mut entries = vec![prompt(turn)];
    entries.extend((0..3).map(|thought| thinking(turn, thought)));
    entries.extend((0..12).map(|call| tool_call(turn, call)));
    entries.extend((0..4).map(|part| assistant_reply(turn, part)));
    entries
}

/// `entries` chat entries: prompts, thinking blocks, tool calls with large
/// results, and long markdown replies, over as many turns as it takes.
pub(crate) fn conversation(entries: usize) -> Vec<MessageData> {
    let mut out: Vec<MessageData> = Vec::with_capacity(entries);
    let mut ts = 1_790_000_000_000u64;
    let mut turn = 0usize;
    while out.len() < entries {
        for mut data in turn_entries(turn) {
            ts += 1_000;
            data.timestamp_ms = Some(ts);
            out.push(data);
        }
        turn += 1;
    }
    out.truncate(entries);
    out
}

pub(crate) fn agents() -> Vec<AgentSummary> {
    (0..2)
        .map(|i| AgentSummary {
            id: id(i),
            name: format!("Agent {i}"),
            is_active: i == 0,
            ..AgentSummary::default()
        })
        .collect()
}
