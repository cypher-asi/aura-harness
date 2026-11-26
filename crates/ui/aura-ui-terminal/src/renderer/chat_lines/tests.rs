use super::*;
use crate::app::ToggleTarget;
use crate::components::MessageRole;

/// `turns` prompts, each answered with a tool call, a notice and a
/// reply that has a list, code and a footer.
fn long_chat(turns: usize) -> VecDeque<Message> {
    let mut chat = VecDeque::new();
    for turn in 0..turns {
        let prompt = format!("question {turn}: what is in `src/{turn}.rs`?");
        chat.push_back(Message::new(MessageRole::User, &prompt));
        let call = Activity::tool(&format!("call-{turn}"), "read_file", "{\"path\":\"a.rs\"}");
        let mut step = Message::activity(call, false);
        step.update_activity(|activity| activity.finish("read"));
        chat.push_back(step);
        let notice = format!("policy: read_file allowed by rule {turn}");
        chat.push_back(Message::new(MessageRole::System, &notice));
        let text = format!(
            "It holds **{turn}** things:\n\n- one\n- two, which goes on past the \
                 edge of a narrow pane\n\n```rust\nfn f() -> u32 {{ {turn} }}\n```\n"
        );
        let mut reply = Message::new(MessageRole::Assistant, &text);
        reply.set_model("claude-sonnet");
        chat.push_back(reply);
    }
    chat
}

#[test]
fn a_chat_laid_out_across_threads_is_laid_out_as_it_is_a_message_at_a_time() {
    let theme = Theme::cyber();
    let open = HashMap::new();
    let groups = GroupState {
        open: &open,
        expand_all: false,
        turn_live: false,
    };
    let size = (40, Measure::GRID);
    let chat = long_chat(40);
    let mut whole = ChatLineCache::default();
    whole.update((&chat, &[]), size, &theme, "*", &groups);

    let mut grown = ChatLineCache::default();
    let mut so_far = VecDeque::new();
    for message in &chat {
        so_far.push_back(message.clone());
        grown.update((&so_far, &[]), size, &theme, "*", &groups);
    }
    let drawn = |cache: &ChatLineCache| {
        cache
            .blocks()
            .map(|block| {
                (
                    block.lines.clone(),
                    block.prose.clone(),
                    block.toggles.len(),
                )
            })
            .collect::<Vec<_>>()
    };
    assert!(whole.blocks().count() > PARALLEL_BLOCKS * 4);
    assert!(drawn(&whole) == drawn(&grown), "the same lines either way");
}

/// A `read_file` step that succeeded, or one still running.
fn read_step(i: usize, done: bool) -> Message {
    let args = format!("{{\"path\":\"src/{i}.rs\"}}");
    let mut step = Message::activity(
        Activity::tool(&format!("call-{i}"), "read_file", &args),
        false,
    );
    if done {
        step.update_activity(|activity| {
            if let Activity::Tool(tool) = activity {
                tool.status = crate::components::ToolStatus::Success;
                tool.result = Some("fn main() {}".to_string());
            }
        });
    }
    step
}

#[test]
fn a_live_group_drawn_from_its_kept_steps_matches_one_laid_out_afresh() {
    let theme = Theme::cyber();
    let open = HashMap::new();
    let groups = GroupState {
        open: &open,
        expand_all: false,
        turn_live: true,
    };
    let size = (60, Measure::GRID);
    let mut chat = VecDeque::from([Message::new(MessageRole::User, "look around")]);
    let mut kept = ChatLineCache::default();
    // A group's first line is its summary, drawn afresh every frame
    // with the clock of a running group, so it is left out: a second
    // passing between the two layouts would tell them apart.
    let lines = |cache: &ChatLineCache| {
        cache
            .blocks()
            .map(|block| {
                let skip = usize::from(!block.user);
                (block.lines[skip..].to_vec(), block.toggles.len())
            })
            .collect::<Vec<_>>()
    };
    // Past nine steps the numbers widen, which moves every step's
    // marker; one step opened midway and one failing change only
    // their own lines.
    for i in 0..12 {
        let mut step = read_step(i, i != 7);
        if i == 7 {
            step.update_activity(|activity| activity.finish("denied"));
        }
        chat.push_back(step);
        if i == 5 {
            if let Some(step) = chat.get_mut(3) {
                step.set_expanded(true);
            }
        }
        kept.update((&chat, &[]), size, &theme, "*", &groups);
        let mut fresh = ChatLineCache::default();
        fresh.update((&chat, &[]), size, &theme, "*", &groups);
        assert!(lines(&kept) == lines(&fresh), "step {i}: the same lines");
    }
    let texts: Vec<String> = kept
        .blocks()
        .flat_map(|block| block.lines.iter().map(ToString::to_string))
        .collect();
    assert!(
        texts.iter().any(|line| line.contains("1 failed")),
        "{texts:#?}"
    );
    assert!(
        texts.iter().any(|line| line.contains("fn main() {}")),
        "the open step"
    );
}

#[test]
fn a_live_group_keeps_its_finished_steps_but_not_running_ones() {
    let theme = Theme::cyber();
    let open = HashMap::new();
    let live = GroupState {
        open: &open,
        expand_all: false,
        turn_live: true,
    };
    let size = (60, Measure::GRID);
    let mut chat = VecDeque::from([
        Message::new(MessageRole::User, "look around"),
        read_step(0, true),
        read_step(1, true),
        read_step(2, false),
    ]);
    let mut cache = ChatLineCache::default();
    cache.update((&chat, &[]), size, &theme, "*", &live);
    let group = |cache: &ChatLineCache| {
        cache
            .blocks()
            .find(|block| !block.user)
            .map(|block| {
                let mut steps: Vec<(u64, bool)> = block
                    .steps
                    .iter()
                    .map(|(id, step)| (*id, step.lines.is_some()))
                    .collect();
                steps.sort_unstable();
                steps
            })
            .unwrap_or_default()
    };
    let ids: Vec<u64> = chat.iter().skip(1).map(|m| m.version().0).collect();
    let mut expected = vec![(ids[0], true), (ids[1], true), (ids[2], false)];
    expected.sort_unstable();
    assert_eq!(
        group(&cache),
        expected,
        "a running step's spinner and clock move, so it is laid out every frame"
    );

    if let Some(step) = chat.back_mut() {
        step.update_activity(|activity| activity.finish("read"));
    }
    cache.update((&chat, &[]), size, &theme, "*", &live);
    assert!(group(&cache).iter().all(|&(_, kept)| kept));

    // Once the turn ends the group is cached whole and keeps no steps.
    let done = GroupState {
        turn_live: false,
        ..live
    };
    cache.update((&chat, &[]), size, &theme, "*", &done);
    assert!(group(&cache).is_empty());
}

/// A `write_file` call still running whose content has streamed in
/// `bytes` long, its JSON not yet closed. It has no start time, so it
/// shows no clock that could tick between two layouts.
fn streaming_write(id: &str, bytes: usize) -> Message {
    let content = "fn main() {}\n".repeat(bytes / 13 + 1);
    let args = format!(
        "{{\"path\":\"src/{id}.rs\",\"content\":{}",
        serde_json::to_string(&content).unwrap_or_default()
    );
    let mut call = Activity::tool(id, "write_file", &args);
    if let Activity::Tool(tool) = &mut call {
        tool.started = None;
    }
    Message::activity(call, false)
}

#[test]
fn a_running_tool_drawn_from_its_kept_parts_matches_one_laid_out_afresh() {
    let theme = Theme::cyber();
    let open = HashMap::new();
    let groups = GroupState {
        open: &open,
        expand_all: false,
        turn_live: true,
    };
    let size = (70, Measure::GRID);
    let lines = |cache: &ChatLineCache| {
        // A live group's summary line carries a clock (see above), and
        // is drawn afresh every frame anyway.
        cache
            .blocks()
            .flat_map(|block| block.lines.iter().map(ToString::to_string))
            .filter(|line| !line.contains(" · step "))
            .collect::<Vec<_>>()
    };
    // A lone step, then the same step at the end of a group, open
    // and closed, as the spinner turns.
    let mut chat = VecDeque::from([
        Message::new(MessageRole::User, "write it"),
        streaming_write("w0", 4_000),
    ]);
    let mut kept = ChatLineCache::default();
    for (frame, spinner) in ["⠋", "⠙", "⠹", "⠸"].into_iter().enumerate() {
        if frame == 2 {
            chat.insert(1, read_step(0, true));
            if let Some(step) = chat.back_mut() {
                step.set_expanded(true);
            }
        }
        kept.update((&chat, &[]), size, &theme, spinner, &groups);
        let mut fresh = ChatLineCache::default();
        fresh.update((&chat, &[]), size, &theme, spinner, &groups);
        let (kept, fresh) = (lines(&kept), lines(&fresh));
        assert_eq!(kept, fresh, "frame {frame}");
        assert!(
            kept.iter()
                .any(|line| line.contains(spinner) && line.contains(" KB")),
            "frame {frame}: the spinner turns on the running step\n{kept:#?}"
        );
    }
}

#[test]
fn a_message_from_another_agents_inbox_folds_to_one_line_until_opened() {
    let theme = Theme::cyber();
    let open = HashMap::new();
    let groups = GroupState {
        open: &open,
        expand_all: false,
        turn_live: false,
    };
    let header = crate::inbox::header(
        crate::inbox::InboxKind::Assignment,
        "3f9a",
        "PRC1",
        "#site coordinator",
        "Fix the tokenizer",
    );
    let text = format!("{header}\n\nThe last token is dropped.\nDone is a passing test.");
    let mut chat = VecDeque::from([Message::new(MessageRole::User, &text)]);
    let drawn = |chat: &VecDeque<Message>| {
        let mut cache = ChatLineCache::default();
        cache.update((chat, &[]), (60, Measure::GRID), &theme, "*", &groups);
        cache
            .blocks()
            .map(|block| {
                let lines: Vec<String> = block.lines.iter().map(ToString::to_string).collect();
                let opens = block
                    .toggles
                    .iter()
                    .any(|toggle| matches!(toggle.target, ToggleTarget::Step(_)));
                (block.user, lines, opens)
            })
            .collect::<Vec<_>>()
    };

    assert_eq!(
        drawn(&chat),
        vec![(false, vec!["✉ PRC1: Fix the tokenizer".to_string()], true)]
    );

    if let Some(message) = chat.front_mut() {
        message.set_expanded(true);
    }
    let opened = drawn(&chat);
    let [(user, lines, opens)] = opened.as_slice() else {
        panic!("one block: {opened:#?}");
    };
    assert!(*user, "framed as a prompt once open");
    assert!(*opens, "it closes again");
    assert!(
        lines
            .iter()
            .any(|line| line.contains("Done is a passing test.")),
        "{lines:#?}"
    );
}
