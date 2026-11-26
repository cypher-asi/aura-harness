use super::super::{content_lines, ChatLineCache, GroupState};
use super::*;
use crate::renderer::text::Measure;
use crate::Theme;
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};

const REPLY: &str = "Here's what I found for **the crates**:\n\n\
## Layout\n\n\
| Crate | Role |\n|---|---|\n\
| `aura-ui` | event protocol shared by the frontends |\n\
| `aura-desktop` | the window, drawn on the GPU |\n\n\
The tests pass, and I updated the README so that it says what each of \
the frontends is for and how a session is started from either of them:\n\n\
```rust\nlet session = start(&options).await?;\n\n// then\nsession.run();\n```\n\n\n\
- Both UIs share one event loop\n- Fonts are Geist Mono and Inter\n\
# A heading right after a list\n\
> quoted\n\n---\n\n1. first\n2. second\r\n\r\nA line after a CRLF gap\n  \n\
```\nunclosed code\n";

/// A loose list of fields in two records, then a paragraph.
const FIELDS: &str = "## New Interfaces\n\
- Path: `lib/ansible/modules/mount_facts.py`\n\n\
- Name: `get_udevadm_device_uuid`\n\n\
- Type: function\n\n\
- Description: Returns the device's UUID, or None when udevadm is missing\n\n\
- Path: `lib/ansible/modules/mount_facts.py`\n\n\
- Name: `MountInfo`\n\n\
- Type: class\n\n\
Both are new in this change.\n";

fn at_once(content: &str, width: usize, theme: &Theme) -> Laid {
    content_lines(
        content,
        MessageRole::Assistant,
        (width, Measure::GRID),
        theme,
    )
}

/// Prose flags as the renderer reads them: lines past the end are not
/// prose.
fn flags((lines, prose, _): &Laid) -> Vec<Option<u16>> {
    (0..lines.len())
        .map(|i| prose.get(i).copied().flatten())
        .collect()
}

/// Stream `content` in `chunks`, checking after each that the reply so
/// far is laid out as it would be at once.
fn stream(content: &str, chunks: impl Iterator<Item = usize>, width: usize) {
    let theme = Theme::cyber();
    let mut settled = Settled::default();
    let mut laid: Laid = Laid::default();
    let mut end = 0;
    for chunk in chunks {
        if end == content.len() {
            break;
        }
        end = (end + chunk.max(1)).min(content.len());
        while !content.is_char_boundary(end) {
            end += 1;
        }
        let so_far = &content[..end];
        settled.extend(so_far, (&mut laid.0, &mut laid.1, &mut laid.2), |text| {
            at_once(text, width, &theme)
        });
        let whole = at_once(so_far, width, &theme);
        assert_eq!(laid.0, whole.0, "lines after {so_far:?}");
        assert_eq!(laid.2, whole.2, "copy buttons after {so_far:?}");
        assert_eq!(flags(&laid), flags(&whole), "prose after {so_far:?}");
    }
    assert_eq!(end, content.len(), "the chunks ran out early");
}

/// Numbers that look random and are the same every run.
struct Dice(u64);

impl Dice {
    fn roll(&mut self, sides: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        usize::try_from(self.0 >> 33).unwrap_or(0) % sides
    }
}

/// A reply of lines of every kind in an order no one would write.
fn jumble(dice: &mut Dice) -> String {
    const LINES: [&str; 29] = [
        "plain text that goes on for long enough to wrap in a narrow pane",
        "short",
        "",
        "",
        "",
        "```",
        "```",
        "```rust",
        "let x = 1;",
        "| a | b |",
        "|---|---|",
        "| 1 | 2 |",
        "|",
        "# Heading",
        "## Another",
        "- item one",
        "  - nested",
        "- [x] done",
        "1. first",
        "> quoted",
        "  ",
        "\t",
        "---",
        "   ```",
        "text with `code` and **bold**",
        "``` not closed `",
        "- Path: `x`",
        "- Name: y",
        "  continued",
    ];
    let crlf = dice.roll(4) == 0;
    let mut reply = String::new();
    for _ in 0..4 + dice.roll(14) {
        reply.push_str(LINES[dice.roll(LINES.len())]);
        reply.push_str(if crlf && dice.roll(3) > 0 {
            "\r\n"
        } else {
            "\n"
        });
    }
    if dice.roll(3) == 0 {
        reply.pop();
    }
    reply
}

#[test]
fn a_break_follows_a_gap_in_the_text_or_a_closed_code_block() {
    assert_eq!(
        last_break("one\n\n\ntwo\n"),
        Some(Break {
            before: 4,
            blank: 2,
            after: 6
        })
    );
    assert_eq!(
        last_break("one\r\n\r\ntwo\r\nthree\n\nfour\n"),
        Some(Break {
            before: 18,
            blank: 1,
            after: 19
        })
    );
    assert_eq!(
        last_break("```\ncode\n\nmore\n```\n\n\nafter\n"),
        Some(Break {
            before: 19,
            blank: 0,
            after: 21
        })
    );
    assert_eq!(
        last_break("```\ncode\n```\nafter\n"),
        Some(Break {
            before: 13,
            blank: 0,
            after: 13
        })
    );
}

#[test]
fn nothing_breaks_inside_code_between_lines_or_before_a_line_still_written() {
    for text in [
        "",
        "one\ntwo\nthree\n",
        "one\n\ntwo",
        "one\n\n",
        "\n\none\n",
        "one\n\n```\ncode\n\nmore\n",
        "one\n\n```rust\ncode\n```\n",
        "```\ncode\n```\n\n```\nmore\n```\n",
        "```\ncode\n```\n`",
    ] {
        assert_eq!(last_break(text), None, "{text:?}");
    }
}

#[test]
fn a_gap_before_a_field_or_an_indented_line_in_a_list_is_not_a_break() {
    for text in [
        "- A: 1\n\n- B: 2\n",
        "- a\n\n- B: 2\n",
        "- a\n\n  more\n",
        "  indented\n\n- A: 1\n",
    ] {
        assert_eq!(last_break(text), None, "{text:?}");
    }
    for (text, before) in [
        ("- a\n\n- b\n", 4),
        ("- a\n\nafter\n", 4),
        ("text\n\n- A: 1\n", 5),
    ] {
        assert_eq!(
            last_break(text),
            Some(Break {
                before,
                blank: 1,
                after: before + 1
            }),
            "{text:?}"
        );
    }
}

#[test]
fn a_reply_streamed_is_laid_out_as_it_would_be_at_once() {
    for width in [28, 80] {
        stream(REPLY, std::iter::repeat(1), width);
        stream(REPLY, std::iter::repeat(7), width);
        stream(REPLY, [REPLY.len()].into_iter(), width);
    }
}

#[test]
fn a_list_of_fields_streamed_is_laid_out_as_it_would_be_at_once() {
    let (lines, _, _) = at_once(FIELDS, 80, &Theme::cyber());
    let blank = lines.iter().filter(|line| line.width() == 0).count();
    assert_eq!(
        blank, 3,
        "under the heading, between records, above the text"
    );
    for width in [28, 80] {
        stream(FIELDS, std::iter::repeat(1), width);
        stream(FIELDS, std::iter::repeat(7), width);
    }
}

#[test]
fn replies_of_every_kind_of_line_are_laid_out_as_they_would_be_at_once() {
    let mut dice = Dice(7);
    for round in 0..400 {
        let reply = jumble(&mut dice);
        let width = [24, 60][round % 2];
        if round % 4 == 0 {
            stream(&reply, std::iter::repeat(1), width);
        } else {
            let sizes: Vec<usize> = (0..reply.len()).map(|_| 1 + dice.roll(40)).collect();
            stream(&reply, sizes.into_iter(), width);
        }
    }
}

#[test]
fn a_reply_that_was_replaced_is_laid_out_again() {
    let theme = Theme::cyber();
    let render = |text: &str| at_once(text, 60, &theme);
    let mut settled = Settled::default();
    let mut laid = Laid::default();
    for content in [
        "one\n\ntwo\n\nthree\n\nfour\n",
        "ONE\n\ntwo\n\nthree\n\nfour\n\nfive\n",
        "ONE\n\ntwo\n\nthree and then some\n\nfour\n",
        "ONE\n\ntwo",
        "",
        "other\n\ntext\n\nhere",
    ] {
        settled.extend(content, (&mut laid.0, &mut laid.1, &mut laid.2), render);
        let whole = render(content);
        assert_eq!(laid.0, whole.0, "{content:?}");
        assert_eq!(laid.2, whole.2, "{content:?}");
        assert_eq!(flags(&laid), flags(&whole), "{content:?}");
    }
}

#[test]
fn only_what_is_still_being_written_is_laid_out_again() {
    let theme = Theme::cyber();
    let seen = RefCell::new(Vec::new());
    let mut settled = Settled::default();
    let mut laid = Laid::default();
    let reply = REPLY.replace("```\nunclosed code\n", "").repeat(40);
    let chars: Vec<char> = reply.chars().collect();
    let mut so_far = String::new();
    for chunk in chars.chunks(12) {
        so_far.extend(chunk);
        settled.extend(&so_far, (&mut laid.0, &mut laid.1, &mut laid.2), |text| {
            seen.borrow_mut().push(text.len());
            at_once(text, 80, &theme)
        });
    }
    let longest = seen.borrow().iter().copied().max().unwrap_or(0);
    assert!(reply.len() > 20_000, "{}", reply.len());
    assert!(longest < 400, "laid out {longest} bytes at once");
    assert_eq!(laid.0, at_once(&reply, 80, &theme).0);
}

#[test]
fn the_chat_shows_a_streaming_reply_as_it_will_once_it_is_done() {
    let theme = Theme::cyber();
    let open = HashMap::new();
    let groups = GroupState {
        open: &open,
        expand_all: false,
        turn_live: true,
    };
    let size = (60, Measure::GRID);
    let mut reply = Message::new(MessageRole::Assistant, "");
    reply.set_streaming(true);
    let mut messages: VecDeque<Message> =
        [Message::new(MessageRole::User, "what is here?"), reply].into();
    let mut cache = ChatLineCache::default();
    let chars: Vec<char> = REPLY.chars().collect();
    for chunk in chars.chunks(5) {
        messages[1].append(&chunk.iter().collect::<String>());
        cache.update((&messages, &[]), size, &theme, "*", &groups);
        let shown = cache.blocks().nth(1).expect("the reply's block");

        let done = Message::new(MessageRole::Assistant, messages[1].content());
        let mut fresh = ChatLineCache::default();
        fresh.update((&[done].into(), &[]), size, &theme, "*", &groups);
        let whole = fresh.blocks().next().expect("the reply's block");
        assert_eq!(
            shown.lines,
            whole.lines,
            "after {:?}",
            messages[1].content()
        );
    }

    let streamed = cache.blocks().nth(1).map(|block| block.lines.clone());
    messages[1].set_streaming(false);
    cache.update((&messages, &[]), size, &theme, "*", &groups);
    assert_eq!(
        cache.blocks().nth(1).map(|block| block.lines.clone()),
        streamed
    );
}
