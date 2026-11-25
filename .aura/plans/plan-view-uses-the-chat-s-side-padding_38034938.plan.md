---
name: Plan view uses the chat's side padding
overview: Lay the plan document out in the same text column as the main chat, with the same left and right padding, width limit and centering, so switching between the chat and a plan does not move the text.
todos:
- id: share-area
  content: Make terminal_text_area pub(super) in renderer/panels.rs
  status: completed
- id: plan-column
  content: Lay out and draw the plan in the chat's text column; drop MAX_TEXT_WIDTH
  status: completed
- id: selection
  content: Pass the plan's column to set_chat_rows and chat_scrollbar in detail.rs
  status: completed
- id: test
  content: Add a test that plan and chat text share the same column at two widths
  status: completed
- id: styleguide
  content: Update the STYLEGUIDE plan-document section
  status: completed
- id: gate
  content: Run fmt, clippy -D warnings and aura-ui-terminal tests
  status: completed
---

# Plan view uses the chat's side padding

Lay the plan document out in the same text column as the main chat, with the same left and right padding, width limit and centering, so switching between the chat and a plan does not move the text.

## Todos

- [x] Make terminal_text_area pub(super) in renderer/panels.rs
- [x] Lay out and draw the plan in the chat's text column; drop MAX_TEXT_WIDTH
- [x] Pass the plan's column to set_chat_rows and chat_scrollbar in detail.rs
- [x] Add a test that plan and chat text share the same column at two widths
- [x] Update the STYLEGUIDE plan-document section
- [x] Run fmt, clippy -D warnings and aura-ui-terminal tests

<!-- end of generated header: edit below -->

## How the two differ today

The chat and the plan view sit inside the same bordered panel, but they place their text differently.

The chat works out its text column in `terminal_text_area` in `crates/ui/aura-ui-terminal/src/renderer/panels.rs`. Inside the border, it keeps two blank columns on each side (one on panels narrower than 40 columns). It caps the text at `MAX_CHAT_WIDTH` (110 columns), gives up a further `CHAT_NARROW_PERCENT` (17%) of that width, and centers what is left, so the space on both sides is equal.

The plan view uses the rectangle every detail view gets in `render_detail_view` in `renderer/detail.rs`. That rectangle is one column in from the border on each side. On top of that, `layout` in `renderer/plan_doc.rs` caps lines at its own `MAX_TEXT_WIDTH` of 100 and keeps them flush left. So on a narrow panel the plan's text sits closer to the border than the chat's, and on a wide one it hugs the left edge with a wide gap on the right. Either way, the text jumps sideways when a plan opens over the chat.

## The change

The plan view takes its text column from the same function the chat uses, so the two cannot drift apart later.

- In `panels.rs`, make `terminal_text_area` `pub(super)` so the plan renderer can call it. Its body and the chat's behavior stay as they are.
- In `draw_plan` in `renderer/plan_doc.rs`, work out the text column as `terminal_text_area(inner, false)`, where `inner` is the panel inside its border. Keep the vertical rows the detail view already uses, so only the left and right edges change. Lay out, draw and publish prose rows within that column instead of `rows`.
- Remove `MAX_TEXT_WIDTH` from `plan_doc.rs`. The chat's 110-column cap with the 17% narrowing then sets the plan's widest line too, so a plan reads at exactly the chat's width.
- Return the column `draw_plan` used in `DrawnPlan`. In `render_detail_view`, pass it to `set_chat_rows` and `chat_scrollbar` for the plan view instead of the shared `rows`, the same way the chat passes its padded area. Text selection and copying then line up with the cells the plan's text covers.

Other detail views (records, files, processes, threads) keep their current padding. Files and logs need every column for code, and the request is only about plans.

## Tests

Add a renderer test in `plan_doc.rs` that draws the chat with a message, notes where the chat's text column starts and ends (from the chat's prose rows), then opens a plan at the same window size. The plan's prose rows should start at the same column and never run past the chat's right edge. Run it at two widths: about 80 columns, and 200 columns where the width cap and centering matter.

The existing plan tests keep passing as they are. `a_plan_lays_out_as_a_document_without_its_source` lays out at 60 columns, which is under any cap, and the screen tests look for text, not columns.

Update the plan-document section of `crates/ui/aura-ui-terminal/STYLEGUIDE.md`: replace "Text runs at most 100 columns wide" with a note that the plan's text uses the chat's column, padding and width.

The gate is `cargo fmt -p aura-ui-terminal -- --check`, `cargo clippy -p aura-ui-terminal -p aura-desktop --all-targets --all-features -- -D warnings` and `cargo test -p aura-ui-terminal`.

## What could go wrong

On a panel narrower than 40 columns the chat keeps only one column of padding, and the plan will too. That is intended: it matches the chat. The todo boxes and the divider rule are laid out from the column's width, so they shrink with it and need no separate change.
