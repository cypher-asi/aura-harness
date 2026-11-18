---
name: 'Consistent side panel: Sandbox / Processes / Trace / Plans'
overview: 'Make the four tabs of the bottom-right panel behave and look the same: one click (or Enter) opens the item in the main (chat) panel through the shared detail views, every row goes through the shared `list_row` and `cell_style`, and all four tabs use the same hover, scrollbar, hint, empty-state and key handling.'
todos:
- id: side-list-helpers
  content: Add renderer/side_list.rs with shared empty-state, hint split/render, and unified hint constants; add detail-suffix variant of panels::list_row
  status: in_progress
- id: sandbox-render
  content: Render sandbox rows via list_row + cell_style (selection/hover backgrounds, muted dot-entries); update dot_folders test
  status: pending
- id: trace-render
  content: 'Trace feed: add hover (HoverTarget::TraceRow), scrollbar, muted detail suffix, shared helpers'
  status: pending
- id: plans-processes-render
  content: 'Plans/Processes renderers: shared helpers; todo text colors muted when done, status color on icon only'
  status: pending
- id: hover-unify
  content: Move sandbox hover into hovered_side_panel via sandbox_row_at; add trace hover
  status: pending
- id: single-click-open
  content: 'click_side_panel: single click opens item in all tabs; fold via glyph column for dirs/plans'
  status: pending
- id: trace-open-step
  content: Trace Enter/click opens step JSON FileView; add `g` -> open_selected_trace_in_graph
  status: pending
- id: plans-open
  content: Plans Enter/click opens plan file (todo jumps to its line); Space/←→/glyph-click fold
  status: pending
- id: keys-unify
  content: Shared SIDE_PAGE, Home/End + Space + per-tab letters via handle_side_list_shortcut before chat fallthrough; plain ←→ fold in trees, no tab switch; update overlays help text
  status: pending
- id: tests
  content: Update/add app and renderer tests; run fmt, clippy -D warnings, tests for aura-ui-terminal
  status: pending
---

## What's inconsistent today

All of this is in `crates/ui/aura-ui-terminal/src`.

**Click / open behavior** (`app/processes.rs::click_side_panel`):
| Tab | Click today | Enter today |
|---|---|---|
| Sandbox | file: opens; dir: folds | same (and `→`) |
| Processes | 1st click selects, **2nd click opens** | opens the process detail |
| Trace | opens, but in the **run graph**, not the step | same |
| Plans | plan row: **folds only**; todo: 1st click selects, **2nd opens** | plan: folds; todo: opens the file |

The KERNEL/Records panel already opens on one click (`click_record_row`), so we use it as the model.

**Rendering** (`renderer/{sandbox,processes,trace_feed,plans}.rs`):
- Sandbox draws rows with its own `sandbox_line`: selected rows are primary + bold and hovered rows use secondary text with no background. The other tabs use `panels::list_row` + `cell_style` (selection background, `surface_hover` background).
- Trace has no hover (`hovered_side_panel` returns `None` for Trace, and there's no `HoverTarget::TraceRow`), no scrollbar, and its label and detail run together in foreground color.
- Text colors follow different rules. Processes: foreground when live, muted when finished. Plans: todo *text* takes the status color (green when completed, primary when in progress). Trace: always foreground. Sandbox: files muted, dirs foreground.
- Hints differ in wording and layout: `↑↓ move  ⏎ open  ← up`, `⏎ details … ⇧← files`, `⏎ open in graph  ⇧←→ tabs`, `⏎ fold / open  ⇧←→ tabs`.
- The empty-state, hint-row and rows-area code is copied four times.

**Keys**: plain `←/→` switches tabs in Processes, Trace and Plans, but means up/fold in Sandbox. PageUp/PageDown moves 5 rows in some tabs and 10 in others. Home/End never reach the side panel lists, because `keys.rs` ~L402 sends them to the chat input. Space can't fold plans for the same reason.

## Target behavior

**One rule for every tab: a single click or Enter opens the row's item in the main panel. Folding is a separate action.**

| Tab | Click / Enter opens | Main panel view |
|---|---|---|
| Sandbox | file | `DetailView::File` (unchanged). A dir click still folds, since a dir has nothing to open. |
| Processes | process | `DetailView::Process` (unchanged, live refreshed) |
| Trace | the step itself | `DetailView::File(FileView::snapshot(node_json))`, titled `TRACE <KIND> <label>`, the same view the graph opens. `g` opens the step in the run graph. |
| Plans | plan row: the plan file; todo row: the plan file with the cursor on that todo's line (`FileView::jump_to`) | `DetailView::File` |

Folding: clicking the fold glyph (the icon column) on a dir or plan row, Space, or `←/→` in the tree tabs (Sandbox, Plans).

Keys, the same in all tabs:
- `↑↓` move; PgUp/PgDn move one shared `SIDE_PAGE` of 10 rows; Home/End go to the first/last row when the input is empty.
- `⏎` opens.
- `⇧←/→` switches tabs (already global through `handle_side_tab_key`). Plain `←/→` stops switching tabs: it folds in tree tabs and does nothing in flat lists.
- Tab-specific letter shortcuts, when the input is empty: Processes `o` agent, `k` kill, `l` log; Trace `g` graph; Sandbox and Plans Space folds.

Hints, all in the format `⏎ open  <extras>  ⇧←→ tabs`:
- Sandbox: `⏎ open  ←→ fold  ⇧←→ tabs`
- Processes: `⏎ open  o agent  k kill  l log  ⇧←→ tabs`
- Trace: `⏎ open  g graph  ⇧←→ tabs`
- Plans: `⏎ open  ←→ fold  ⇧←→ tabs`

## Styling rules (shared)

Every row is drawn with `list_row` + `cell_style`, so all four tabs get the same selected background, hover background and icon column.

- **The icon carries status color. Text does not.** Primary text is `foreground`, finished or secondary items are `muted`, and the right-hand meta (elapsed time, `#seq`, `n/m`) is muted (already the case in `cell_style`).
- Sandbox: dirs and files are foreground; dot-entries, symlink suffixes and "more" rows are muted; the folder glyph is muted.
- Processes: unchanged (live is foreground, finished is muted).
- Trace: label in foreground, with the detail drawn as a muted suffix; the status glyph is colored.
- Plans: plan name in foreground; pending and in-progress todos in foreground; completed todos muted; cancelled todos muted and crossed out. The marker glyph keeps its status color (✓ success, ▸ primary, ○/✗ muted).

To support the muted suffix, `panels::list_row` gets a variant that takes `detail: Option<&str>`, drawn muted after the name and truncated before it. `list_row` becomes a thin wrapper so existing callers don't change.

## Code changes

1. **`renderer/side_list.rs` (new)**: shared pieces for the four tabs:
   - `render_empty(frame, inner, &[&str], theme)`
   - `split_hint(inner, focused) -> (rows_area, Option<Rect>)`
   - `render_hint(frame, area, text, theme)`: truncated, muted, leading space
   - the hint constants above
2. **`renderer/sandbox.rs`**: replace `sandbox_line` with `list_row` (depth = `row.depth`, `StatusIcon` = folder glyph or empty) using `CellState::of(selected, hovered)`. Update the `dot_folders_are_muted` test.
3. **`renderer/trace_feed.rs`**: take the `area` param, add hover (`HoverTarget::TraceRow`) and `render_scrollbar`, use the detail suffix.
4. **`renderer/plans.rs`, `renderer/processes.rs`**: use the `side_list` helpers and the new todo color rule.
5. **`renderer/overlays.rs:370`**: change the help text to `Shift+←/→  side panel tabs`.
6. **`app/mod.rs`**: add `HoverTarget::TraceRow(usize)`. Move sandbox hover into `hovered_side_panel` (drop the special branch at ~L1492) behind a `sandbox_row_at` that matches `process_row_at`, `trace_row_at` and `plan_row_at`. Have `click_sandbox_row` fold only when the click lands on the glyph column or the row is a dir.
7. **`app/processes.rs`**:
   - `click_side_panel`: one click selects and opens in every tab.
   - Update its doc comment.
   - Replace `handle_processes_shortcut` with `handle_side_list_shortcut`, which dispatches per tab (Space, `g`, `o`/`k`/`l`, Home/End). Call it from `keys.rs` ~L398 so Home/End/Space are handled before they fall through to the chat input.
8. **`app/trace_feed.rs`**: `open_selected_trace_entry` opens the step JSON with `trace_json::node_json`. Links come from `node.links`; their labels are looked up in the graph when it's loaded and in the feed otherwise. The current graph behavior moves to `open_selected_trace_in_graph` (`g`). `Left`/`Right` stop switching tabs.
9. **`app/plans.rs`**:
   - Enter and click open; Space, `←/→` and clicking the fold glyph fold.
   - A todo opens the plan with `jump_to` on the first line containing that todo's content.
   - Plain `←/→` stops switching tabs.
10. **`app/keys.rs::handle_sandbox_key`**: `→` expands only (no file open), `←` collapses or goes to the parent, Enter opens or folds. Add the shared page size.

## Tests

- `app/processes.rs::clicking_tabs_and_rows`: one click opens `DetailView::Process`. `shift_arrows_switch_tabs_from_either_tab` now uses only Shift+arrows, and a new assertion checks that plain `←` no longer switches tabs.
- `app/plans.rs`: Enter on a plan row opens the file; Space and `←` fold; clicking a todo opens the file with the cursor on the todo line; clicking the fold glyph folds.
- `app/trace_feed.rs`: a click opens a File view titled `TRACE …` whose JSON parses; `g` opens `DetailView::Graph` with the node selected. Update the graph.rs test at ~L673 to call `open_selected_trace_in_graph`.
- Hover: `HoverTarget::TraceRow` and `SandboxRow` both come through `hovered_side_panel`.
- Renderer: one style test per tab checking that the selected row uses the `selection` background, the hovered row uses `surface_hover`, finished items are muted, and the status color is only on the icon. Each hint renders in the shared format.
- Run `cargo fmt`, `cargo clippy -p aura-ui-terminal --all-targets -- -D warnings` and `cargo test -p aura-ui-terminal`.

## Risks

- Dropping plain `←/→` tab switching changes muscle memory in Processes and Trace. It's a small change to revert if you'd rather keep it in the flat lists.
- Opening a trace step no longer opens the graph straight away; `g` keeps that path one key away.