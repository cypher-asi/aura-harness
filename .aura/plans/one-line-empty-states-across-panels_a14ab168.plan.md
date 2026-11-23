---
name: One-line empty states across panels
overview: Make every panel's empty state a single muted line, drawn by one shared helper with the same 2-column indent, no padding rows, and truncated with an ellipsis instead of wrapping.
todos:
- id: helper
  content: Make side_list::render_empty one line; add empty_line and EMPTY_INDENT
  status: pending
- id: side-tabs
  content: Trim SANDBOX, PROCESSES, TRACE, PLANS, TASKS and render_hidden to one line
  status: pending
- id: kernel-tabs
  content: Trim KERNEL records, MEMBERS (both states), POLICIES and INBOX to one line
  status: pending
- id: other-panels
  content: Route ARCHIVE, graph overlay, flow canvas and chat welcome through the one-line style
  status: pending
- id: fix-tests
  content: Update archive and hidden-processes tests to the new text
  status: pending
- id: new-tests
  content: Add render_empty unit test and an every-empty-tab-is-one-line test
  status: pending
- id: changelog
  content: Add CHANGELOG Unreleased entry
  status: pending
- id: verify
  content: Run fmt, clippy -D warnings and cargo test -p aura-ui-terminal
  status: pending
---

# One-line empty states across panels

Make every panel's empty state a single muted line, drawn by one shared helper with the same 2-column indent, no padding rows, and truncated with an ellipsis instead of wrapping.

## Todos

- [ ] Make side_list::render_empty one line; add empty_line and EMPTY_INDENT
- [ ] Trim SANDBOX, PROCESSES, TRACE, PLANS, TASKS and render_hidden to one line
- [ ] Trim KERNEL records, MEMBERS (both states), POLICIES and INBOX to one line
- [ ] Route ARCHIVE, graph overlay, flow canvas and chat welcome through the one-line style
- [ ] Update archive and hidden-processes tests to the new text
- [ ] Add render_empty unit test and an every-empty-tab-is-one-line test
- [ ] Add CHANGELOG Unreleased entry
- [ ] Run fmt, clippy -D warnings and cargo test -p aura-ui-terminal

<!-- end of generated header: edit below -->

## Approach

All empty states live in `crates/ui/aura-ui-terminal/src/renderer` (the desktop app paints this same buffer, so fixing it here covers both front ends). Most side-panel and Kernel-panel tabs already go through `side_list::render_empty`, but it takes `&[&str]` and callers pass 2 to 4 lines. A few panels draw their own: the Records list, the Archive list, the graph overlay, the flow canvas and the chat welcome.

Plan:
- Change `render_empty` to take one `&str`. It draws a single muted line with a 2-column indent, cut to fit with `truncate_record_info`, so it never wraps.
- Add `empty_line(text, width, theme) -> Line<'static>` next to it, and move `EMPTY_INDENT` from `panels.rs` to `side_list.rs`, so lists that build their own `Vec<Line>` (Archive, the Members footer) draw it the same way.
- Keep the first sentence of each message and drop the explanation lines after it. No blank rows above the message.

```rust
/// Draw a tab's empty state: one muted line, indented two columns,
/// cut to the panel's width rather than wrapped.
pub(super) fn render_empty(frame: &mut Frame, inner: Rect, text: &str, theme: &Theme) {
    frame.render_widget(Paragraph::new(empty_line(text, inner.width, theme)), inner);
}
```

## Message changes

| Panel / file | Before | After |
|---|---|---|
| SANDBOX, no root (`sandbox.rs`) | No sandbox yet. | No sandbox yet. |
| SANDBOX, empty root | Sandbox is empty. + 2 lines | Sandbox is empty. |
| PROCESSES (`processes.rs`) | Nothing running. + 3 lines | Nothing running. |
| TRACE (`trace_feed.rs`) | No steps yet. + 3 lines | No steps yet. |
| PLANS (`plans.rs`) | No plans yet. + 2 lines | No plans yet. |
| TASKS (`tasks.rs`) | No tasks yet. + 2 lines | No tasks yet. |
| Other-agent items (`side_list::render_hidden`) | 3 lines | `No {what} for this agent. * lists {hidden}.` |
| KERNEL records (`panels.rs::render_empty_records`) | blank + No records yet. + blank + 2 lines | No records yet. |
| MEMBERS, no agent (`members.rs`) | No agent on screen. + 2 lines | No agent on screen. |
| MEMBERS, lead alone (footer under the row) | blank + 2 lines | blank + No agents joined yet. |
| POLICIES (`policies.rs`) | No kernel open for this agent. + 2 lines | No kernel open for this agent. |
| INBOX (`inbox.rs`) | No messages between agents yet. + 2 lines | No messages between agents yet. |
| ARCHIVE (`panels.rs::build_archived_lines`) | 2 sentences, word-wrapped | No archived agents. (total = 1) |
| Graph overlay (`graph.rs`) | hand-built `"  No steps recorded yet."` | same text, through `render_empty` |
| Flow canvas (`flow.rs`) | long wrapped sentence, 1-col pad | Nothing recorded yet. / Loading the graph… through `render_empty` |
| Chat welcome (`panels.rs::render_welcome`) | wraps (`Wrap { trim: true }`) | `Type a message, $ for a terminal, /help for commands.` with no wrap, truncated |

These stay as they are, because they are already one line and drawn in cells: the flow timeline (`flow_timeline.rs`, "Nothing recorded yet." / "Nothing failed. e shows every agent again."), `Loading chat…`, the run log notice, and `Empty file` in the detail view.

## Files to change

- `renderer/side_list.rs`: new `render_empty` signature, `empty_line`, `EMPTY_INDENT`, the one-line `render_hidden`, and update the module doc.
- `renderer/{sandbox,processes,trace_feed,plans,tasks,members,policies,inbox,graph,flow}.rs`: switch callers to the single string.
- `renderer/panels.rs`: `render_empty_records`, `build_archived_lines` (drop `wrap_words` for the empty case) and `render_welcome`.
- Tests:
  - `app/archive.rs::the_empty_archive_says_so_inside_the_panels_padding` should assert one line, "No archived agents.", with a 2-column indent.
  - `renderer/tests.rs:1348-1350` should assert the new one-line hidden message.
- New tests:
  - In `side_list.rs`: `render_empty` fills exactly one row and ends in `…` on a narrow panel.
  - In `renderer/tests.rs`: one test that opens each empty tab (SANDBOX, PROCESSES, TRACE, PLANS, TASKS, KERNEL, MEMBERS, POLICIES, INBOX, ARCHIVE) and asserts exactly one non-blank body row.
- `CHANGELOG.md`: a `##### Changed` entry under Unreleased.

## Risks

- The hint text disappears. For example, PLANS no longer says that `/plan` asks for a plan, and the `*` scope hint is shorter. The hidden-items line keeps the `*` hint because without it the empty tab would be confusing.
- On very narrow panels the line is cut with `…` instead of wrapping. This is intended.

## Verification

`cargo fmt --all -- --check`, `cargo clippy -p aura-ui-terminal --all-targets -- -D warnings`, `cargo test -p aura-ui-terminal`.
