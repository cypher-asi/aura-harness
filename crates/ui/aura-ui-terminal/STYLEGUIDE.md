# AURA Terminal Style Guide

This document defines the color palette and styling conventions for the
AURA terminal UI and the desktop window that draws it. The default theme
(`cyber`) is graphite and gray: the chrome is neutral, and color is kept
for status.

## Palette

| Color      | Hex Code  | RGB               | Rust Constant | Usage                                  |
|------------|-----------|-------------------|---------------|----------------------------------------|
| Cyan/Green | `#01f4cb` | `(1, 244, 203)`   | `CYAN`        | Status, the focused panel's border     |
| Blue       | `#01a4f4` | `(1, 164, 244)`   | `BLUE`        | Warnings, waiting states, flags        |
| Purple     | `#cb01f4` | `(203, 1, 244)`   | `PURPLE`      | Syntax highlighting only               |
| Red        | `#e06c75` | `(224, 108, 117)` | `RED`         | Errors, danger                         |
| White      | terminal  | `Color::White`    | `WHITE`       | Titles, what the user typed            |
| Light gray | `#f2f2f2` | `(242, 242, 242)` | `LIGHT_GRAY`  | Reply text                             |
| Silver     | `#b3b3b3` | `(179, 179, 179)` | `SILVER`      | Hovered labels, to-do counts           |
| Gray       | `#808080` | `(128, 128, 128)` | `GRAY`        | Secondary text, prompts                |
| Dim gray   | `#5a5a5a` | `(90, 90, 90)`    | `DIM_GRAY`    | Borders at rest, markers, separators   |
| Black      | `#191919` | `(25, 25, 25)`    | `BLACK`       | Background                             |
| Dark gray  | `#242424` | `(36, 36, 36)`    | `DARK_GRAY`   | Raised surfaces (user prompt band)     |
| Graphite   | `#2e2e2e` | `(46, 46, 46)`    | `GRAPHITE`    | Hover fill                             |
| Slate      | `#383838` | `(56, 56, 56)`    | `SLATE`       | Selection fill                         |

## Semantic Color Mapping

Use theme colors via `theme.colors.*`, never the constants, so the other
themes keep working:

| Theme Field     | Cyber      | When to Use                                          |
|-----------------|------------|------------------------------------------------------|
| `background`    | Black      | The main background                                  |
| `foreground`    | White      | Titles, the user's messages, the item being worked on |
| `text`          | Light gray | The agent's replies, what is being typed             |
| `primary`       | White      | Headings and emphasis                                |
| `secondary`     | Silver     | Hovered labels, the to-do count                      |
| `success`       | Cyan       | Live and working status (`●`, `⬢`), focused borders, non-agent modes |
| `warning`       | Blue       | Warnings, waiting on approval                        |
| `error`         | Red        | Errors, failed steps, danger actions                 |
| `pending`       | White      | Pending states                                       |
| `muted`         | Gray       | Secondary text, prompts                              |
| `faint`         | Dim gray   | Borders at rest, step markers, separators            |
| `surface`       | Dark gray  | Raised rows                                          |
| `surface_hover` | Graphite   | Hovered rows and tabs                                |
| `selection`     | Slate      | The selected row and the focused active tab          |
| `on_selection`  | White      | Text on `selection`                                  |

## Usage Guidelines

### Weight and Slant

- No italics in UI chrome: steps, hints, notices and counts are upright.
  Italics are kept for markdown emphasis in replies.
- Bold is for titles and headings, the active tab, the live tail's
  `Working` and status label, and the to-do being worked on. Prompts,
  project names, code block labels and the section titles of panel
  bodies are regular weight.

### Status Indicators

- **Ready (`●`)** and **working (`⬢`)**: `success`
- **Waiting (`◐`)**: `warning`
- **Error (`✗`)**: `error`
- **Failed agent**: an agent that errored out is drawn in the Stop
  button's red until its next turn starts. It errors out by failing to
  start (its session won't open, its local model is down, the cloud
  refuses it), by failing to get a cloud machine, by a turn that ends
  in an error, or by stalling (the stall sweep gave up on it). All four
  look the same in all three places:
  - On PROJECTS its name is red and its icon column shows `!` in red.
  - On a channel's MEMBERS the `!` replaces the `@` and its second row
    reads `Failed: <why>` in red, cut to fit, in place of its title.
  - Its chat shows the whole reason as an error: `✗ ` then the reason,
    both in red. Other harness notes keep `* ` in `muted`.

  The red is `failure_color(theme)` (`renderer/mod.rs`), the one the
  input's `■ Stop` is drawn in, never a red of its own. Any new way an
  agent fails uses it and the same `!`.
- **Starting**: a channel member whose cloud machine is starting reads
  `Starting in the cloud…` in `muted` on its MEMBERS row, so a member
  still starting can't be mistaken for an idle one.
- **Benchmarks** never use `warning`: a stopped or cancelled benchmark,
  a run that errored and the detail's notes read in the accent, and a
  failed check is an accent `▓` in the progress bar.
- **To-dos**: `☐` pending in `muted`, `◐` in progress in bold
  `foreground`, `✓` done in `faint`, `⊘` cancelled in `faint` and struck
  through. Each state has its own shape, never only a fill, so an item in
  progress can't read as checked: no filled or crossed boxes. Past ten,
  the rest are counted on a `muted` `+N more` row. The TASKS tab
  draws a plan's todos and the agents' todo items the same way, with
  their boxes right under their group header's fold arrow. What the
  user asked has no row: each request's items stand for it, merged per
  agent since the list carries over between requests. Only a task an
  agent handed out is indented, one level (two columns) under the todo or
  item it does; one doing none sits at the left edge after the items.
  Work goes under the header of the plan it works on, below its todos,
  or else under a `No Plan` header.

### Borders and Panels

- **Focused panel**: `success` border, and the input box's while the
  Chat panel has focus
- **Unfocused panel**: `faint` border
- **Menus**: `muted` border
- **The to-do drawer, queue and benchmark panels**: the Chat panel's
  border, focused or not, joined to it by `├`/`┤`
- **Approval dialog**: `warning` border
- **Code blocks and step rules**: `faint`
- **Spacing**: panels side by side sit one blank column apart and one
  column in from the screen's edges. A cell is about twice as tall as it
  is wide, so that matches panels stacked on rows that touch. The side
  columns end level with the input box; the footer sits under the Chat
  column alone.
- **Padding**: a panel's body starts one blank row under its top border,
  and its headings and text keep two columns from each side border, as
  far from the sides as from the top: an info tab such as POLICIES, HOST
  or a channel's INFO, a record, file or thread opened over the chat,
  the chat, the terminal, the benchmarks, LOG's header, TIME's summary,
  the headers of the to-do drawer and the queue, the benchmark panel and
  the input box. Lists keep less: a list's rows start a blank column in
  and their fills run border to border, and the heads over a list's
  columns, such as LOG's and TIME's, line up with them. The sections
  under the chat (the to-do drawer, the benchmark and queue panels and
  the input box) get no blank row.

### Panel Headers

Every panel has its own header bar above its body: a bordered frame of
equal-width tab cells, then an info cell filling the rest of the width.
Most tab bars spread their tabs over the whole width. The Kernel panel's
KERNEL | MEMBERS | POLICIES | INBOX bar keeps its tabs at their own width
and leaves the rest to the info cell, which shows the open tab's count
(or, on POLICIES, the agent's `MODE/APPROVAL`; on INBOX, `N NEW` while
threads are unread) on its right. Bars
without tabs (a record or file shown over the chat) show a title on the
left of the info cell.

```text
┌──────────┬──────────┬───────────┐
│   CHAT   │▓TERMINAL▓│  PLANNER  │
└──────────┴──────────┴───────────┘
┌─────────────────────────────────┐
│ body                            │
```

```text
┌──────────┬──────────┬──────────┬──────────┬────────────┐
│  KERNEL  │▓MEMBERS ▓│ POLICIES │  INBOX   │   3 AGENTS │
└──────────┴──────────┴──────────┴──────────┴────────────┘
```

The MEMBERS tab lists the agent on screen, which leads its run, then the
agents the run brought in, each indented under the agent that started it.
Its rows use the Processes tab's status glyphs, and `lead`, `task` or
`agent` with the time worked in place of a row's time.

The POLICIES tab is an info body (see Panel Bodies) in sections: STATUS,
CAPABILITIES, TOOLS, ALWAYS ALLOW, SESSION GRANTS and INTEGRATIONS.
TOOLS rows mark each tool `✓`, `?` or `✗` in the accent, then give its
state (`on`, `ask`, `off`) and the deciding rule in `muted`. Only what
weakens the guardrails takes the accent: `yolo`, commands that can't be
confined, no live prompts. The header's `MODE/APPROVAL` info stays
`muted` like every other count.

The INBOX tab lists the threads of mail between the agents of the run,
the most recently active first. A thread row starts with the unread dot
`●` in the accent (`success`, a space once read) and the fold arrow
`▸`/`▾`, then the subject, bold while unread, and its `(count)`; the
participants (`Lead ↔ scout`) and the `HH:MM` of its newest message sit
on the right. An unfolded thread lists its messages under it, indented:
an optional `↩` for replies, `from → to`, a preview cut to fit and the
time. Counts, participants, names and times are `muted`; the subject
and preview are `foreground`. Rows highlight like MEMBERS rows.

```text
 ● ▾ Review parser changes (3)   Lead ↔ scout  12:04
       Lead → scout   Please check the tokenizer…  12:01
     ↩ scout → Lead   Found two issues: …          12:03
   ▸ Benchmark run (1)           Lead → bench  11:50
```

Opening a thread or a message shows the conversation over the chat,
headed `THREAD <subject>` with its message count. Each message has a
`▸`/`▾ from → to · HH:MM` header (` · reply` for replies), bold on the
selection when highlighted; a folded message shows a `muted` preview
after its header, an unfolded one its text indented two columns. `n`/`p`
move between messages, ⏎ or Space folds one, `e`/`c` unfold or fold
them all, and Esc closes the view.

- **Frame and dividers**: the panel's border color
- **Active tab**: bold. While the panel is focused it is filled with `selection` and labelled in `on_selection`; otherwise the label is `foreground` with no fill
- **Hovered tab**: `surface_hover` fill with a `secondary` label
- **Other tabs**: `muted` label, no fill
- **Info cell**: no fill; counts and `MODE/APPROVAL` in `muted`, unread mail and trace issues in `warning`

Narrow bars drop the tab padding, then switch to three-letter labels
(`short_label`), and give the info cell's room to the tabs when it would be
under three columns.

The side panel's bar ends in buttons, each a cell after a divider of its
own: `⤢` against the border, `*` (`ALL`) left of it, and left of that
HOST, drawn as its icon `🖥` rather than a label. The icon is a tab like
the others, lit, hovered and clicked the same way, and centered in its
cell. The cells are padded while the tabs fit beside them, then shrink to
their glyphs, and on a bar narrower still `⤢` goes.

```text
│ SBX │REPO │ PRC │TRACE│TASKS│ 🖥  │ * │ ⤢ │
```

The agents panel's bar ends the same way in BENCH, drawn as its icon
`🔧` in a cell of its own against the border, after ARCHIVE. While the
pointer is over an icon tab, its name (`Bench`, `Host`) shows on the row
under the bar, right-aligned under the icon and shaded like a hovered
button.

```text
│ CHN │ PRJ │AGT │ARC │ 🔧 │
```

### Panel Bodies

Below its header bar every panel draws its tab in the same body
(`chrome::Body`): the tab's rows, padded as above, and a scrollbar on
the right border when they overflow. No body spells out its keys or
what a click does; its text names things and gives their state.
What the rows are depends on what the tab shows:

- **Lists** (SANDBOX, REPO, PROCESSES, TRACE, TASKS, KERNEL, MEMBERS, INBOX,
  a channel's MEMBERS) draw one `components::list_row` row per thing: its
  status glyph, its name and its details on the right. ↑↓ move the
  selection and ⏎ opens it.
- **A channel's HISTORY** (its tabs: MEMBERS, INFO, QUEUE, TASKS,
  HISTORY) lists every message the user sent in the channel, newest
  first: those posted to members, those typed to the coordinator
  (read from its chat; a message sent to both is listed once) and its
  `/plan` requests, which the channel's chat never shows, under a `muted` bold `Your messages` heading with a `faint`
  `· N`. Each takes two rows: the message on one line in `foreground`,
  line breaks folded into spaces, then its local time in `muted` (`20:31`
  today, `11-27 20:31` before). A message over 80 characters shows its
  one-line model summary once it comes, until then the message cut at a
  word with `…`. The highlight and the pointer light the item as a
  `list_row` does. ⏎ or a click jumps the chat back to the message: it
  scrolls so the message starts a row below the top and lights its rows
  in `selection` on `on_selection` for 1.2 seconds; the focus stays on
  the list. A `/plan` request opens its planner's own chat instead,
  jumped and lit the same way, or says the planner has left. The wheel scrolls the list only, never the highlight.
- **Info bodies** (POLICIES, HOST, a channel's INFO, the RECORD and
  PROCESS views, a project's info box) describe one thing as a document
  built with `components::Info`. The whole document scrolls with ↑↓,
  PgUp/PgDn and Home/End.

```text
boxd · aura-re36-3f9c              title     bold foreground
● ready · up 2h 14m                status    the accent

RESOURCES  1 near full             section   secondary, then a muted note
  Region    fra                    fact      muted label, value cut with …
  Disk      █████████░ 91%         bar       the accent, the rest faint
  ✓ read_file   on   per tool      marked    only the glyph colored
  `cargo test` commands            item      foreground, then a muted detail

[p] pull changes  [s] sleep        actions   primary keys, muted labels
```

- **Sections**: titled in `secondary`, regular weight, flush left, with
  one blank row before each but the first. Everything under a title is
  indented two columns.
- **Facts**: a `muted` label in a ten-column label column, then the
  value, cut with `…`. The RECORD and PROCESS views use fourteen columns
  and wrap long values under themselves. A fact without a value shows a
  `muted` `—`.
- **Values** are `foreground`; ids, hashes and `none` are `muted`; what
  failed is `error`. Only a value that needs attention takes the accent
  (`yolo`, a disk 90% full, a verdict still pending). Section titles,
  labels and the header's info cell never do.

The REPO tab is a list under a one-row strip of filters: `All`, the kinds
of event the repo has, and past a `faint` `│` its branches, tags and
worktrees. Filters are drawn like tabs, and a strip too wide for the
panel is cut around the one picked, with `‹` and `›` where it is cut.
Under `All` the branch's sync comes first, then the uncommitted changes
under a fold arrow, then the timeline, newest first.

```text
 All  Changes  Commits  Pushes │ Branches  Tags
 ↑  main  2 ahead of origin/main         a1b2c3d4
 ▾  Uncommitted changes                   2 files
   M  src/lib.rs                           staged
   ?  notes.txt
 ○  Add the REPO tab  a1b2c3d4 · main          now
 ↑  Pushed 2 commits to origin/main             5m
 ●  Fix the parser  e5f6a7b8                    2h
```

- **Sync**: `↑` `↓` `↕` in `warning` while the branch and its upstream
  are apart, `✓` in the accent when they aren't, `○` with no upstream.
- **Changes**: the status letter in `warning` while any of the change is
  unstaged, in the accent once all of it is staged, `error` for a
  conflict and `muted` for untracked files.
- **Events**: glyphs are `muted` but for what is live: a commit not
  pushed yet (`○`, `warning`), a push or pull (`↑`, `↓`), an open PR
  (`↪`) and a merged one (`✓`) take the accent, a reset (`↩`) `warning`.
  Merged and closed PRs, gone branches and notes such as how to sign in
  to GitHub read as settled. The age is on the right in `muted`.

### The Chat Column

```text
┌──────────────────────────────────────────────┐
│                                              │
│  → fix the login bug                         │
│                                              │
│    ▸ Explored 4 files, 2 searches            │
│      2 steps                                 │
│                                              │
│  ⬢ Thinking… 12s                             │
├──────────────────────────────────────────────┤
│  ⬢ Working on 1 active · 1 done           —  │
│ ◐ Fix the redirect     → FEN6 · working 4m   │
│ ▸ Finished to-dos  1                            │
├──────────────────────────────────────────────┤
│  →  Add a follow-up                          │
└──────────────────────────────────────────────┘
 qwen3:4b · 23% context used · 2 calls  ● Ready
```

- The conversation is inset two columns, starts a row below the border
  and runs at most 110 columns wide.
- **Room below**: once the conversation reaches the bottom, a quarter of
  the chat's rows stay blank under its last line, so new text scrolls up
  with space below it instead of running into the border. The terminal
  pane's output runs down to its prompt.
- **Terminal**: the TERMINAL tab is a terminal in the Chat body, with no
  input box under the panel. Each command shows as the shell
  prompt and the command, its output under it, then the prompt where the
  next command is typed, highlighted as a command, on the row after the
  last line, lined up with the output. Once the output fills the body it
  scrolls up and the prompt keeps the body's last rows. While a command
  runs, `■ Stop` sits at the prompt row's right end. A view over the chat
  brings the input box back for as long as it covers the terminal.

```text
│  [u@h aura]$ echo hi                         │
│  hi                                          │
│  [u@h aura]$ ls█                             │
│                                              │
└──────────────────────────────────────────────┘
```
- **Empty channel**: a channel's own chat with no post and no message
  shows its slug as a banner, a blank row, then its coordinator's
  welcome, flush with where the conversation starts:

  ```text
  │  ██████    ██    ████    ██████                │
  │      ██  ██  ██  ██  ██  ██                    │
  │    ██    ██  ██  ██  ██  ████                  │
  │  ██      ██  ██  ██  ██  ██                    │
  │  ██████    ██    ████    ██████                │
  │                                                │
  │  Welcome to #zode: Ship it.                    │
  │                                                │
  │  I'm COO2, the coordinator. Tell me what you   │
  │  need and I'll hand it to an agent, then       │
  │  report back here.                             │
  │                                                │
  │  Mention a member with @CODE to talk to it     │
  │  directly.                                     │
  ```

  The banner is drawn in `primary` with the built-in 3×5 pixel font
  (`renderer/channel_welcome/font.rs`, `a-z`, `0-9` and `-`), and takes
  the biggest form that fits the width and the rows left after the
  welcome. In order, those forms are large (`██` pixels, 5 rows), small
  (half blocks `▀▄█`, 3 rows), small broken after the slug's hyphens
  (at most 3 lines, a blank row apart), and the plain `#slug` in bold
  `foreground`, cut with `…`. The welcome is `text` and its last line
  `muted`. It is drawn, not posted: the first message replaces it, and
  a member's own chat never shows it.
- **User messages**: a `muted` `→ ` prompt, then `foreground` text
- **Replies**: `text`, list markers `muted`, inline code `secondary` on
  `surface`
- **Fields**: a list item that starts with a short label (`- Path: …`)
  has the label `muted`; blank lines between two such items are not
  drawn, and where the first label of a run of them comes back, a blank
  row sets the next record apart
- **Steps**: markers `faint`, the whole line `muted`
- **Live tail**: while a turn runs, the working line flush with the
  reply: `⬢` in `success`, the label bold `foreground`, the elapsed time
  `muted`. The to-do list is never part of the conversation.
- **To-do drawer**: while the agent on screen has a to-do item open, in
  a turn and between turns, a drawer sits under the Chat body in the
  same column of borders, above the benchmark and queue panels. In an
  open channel it shows the coordinator's list. Its
  header is `⬢` `faint`, then `Working` bold `foreground` with `on N
  active` `secondary` during a turn, or `N active` bold `foreground` when
  idle, then `· N done` (and `· N dropped`) in `muted` when there are
  any, and the `—`/`+` toggle at the right, `muted` and `foreground` under
  the pointer. Clicking the header or `/todos` folds it to that one row,
  which then also shows the item in progress. Open, it lists only the
  items still open (pending and in progress), one row each, each with
  `→ CODE · working 4m` (or `idle`) in `faint` for the agent it was given
  to: its code, whether it works, and how long the item has been in
  progress. The item's text is cut before the agent is; a narrow row
  keeps `→ CODE`. Done and cancelled items sit under a `muted`
  `▸ Finished to-dos  N` row, folded by default; a click on it, `Alt+H` or
  `/todos history` opens it (`▾`), listing each with its agent and
  `done` or `cancelled`. Up to ten rows show, starting at the item in
  progress so it stays in view and keeping the history row, with a
  `muted` `+N more` row for the rest. It gives up rows before the queue, the benchmark panel and
  the chat on a short screen. A list with every item closed hides it;
  `/todos` brings it back with its history open.
- **Input box**: as wide as the Chat panel, bordered like it, with a
  `muted` `→ ` prompt and a `muted` placeholder (`Ask anything`, then
  `Add a follow-up`) set one column past the cursor so the cursor never
  covers its first letter. Login prompts replace the arrow. The terminal
  has none: its prompt follows its output (see **Terminal**).
- **Footer**: the mode outside agent mode, the approval level, the model
  and what the agent has used in `muted`, separated by `faint` dots,
  with the status at the right, on the screen's last row. Parts that
  don't fit drop from the right.
- **Placement switch**: the footer's right end, `⌂ local` in `muted` or
  `☁  cloud` in `primary`, with the machine's state when it isn't ready
  and the owner for another user's agent. Under the pointer it lights
  like a button: a `surface_hover` fill one column past each side of the
  label, which reads in `foreground`. It doesn't light while it shows a
  move or on another user's agent. A click shows the move at once: a
  `success` `◐` turning a quarter at a time, then `moving to cloud` or
  `bringing back` with the chat's working shimmer. When the move lands,
  one `success` band crosses the new label. With spinners off, a move
  reads all in `success` and nothing sweeps.

### Plan Documents

```text
│ PLAN Plans that read like a document     2/5 TODOS ✕ │
┌──────────────────────────────────────────────────────┐
│                                                      │
│  Plans that read like a document                     │
│                                                      │
│  created 14:32 · 2h ago  by RUS199 rust-engineer     │
│  asked by COO2 in #aura                              │
│  status building 2/5  machine aura-rus199-12b4       │
│                                                      │
│  Change the guidance the agent gets for writing...   │
│                                                      │
│  TODOS  2/5                                          │
│  ☒ Rewrite the plan-writing bullet                   │
│  ▣ Update the create_plan description                │
│  ────────────────────────────────────────────────    │
│                                                      │
│  Why plans read badly today                          │
│                                                      │
│  The agent writes plans the way it is told to...     │
└ 1-12/80 ─────────────────────────────────────────────┘
```

- A plan opened from the TASKS tab, a `create_plan` or `read_file` step
  in the chat, or the sandbox tree reads as a document over the chat
  panel, never as its source: no frontmatter, no generated-header marker.
- **Title** bold `foreground`; **overview** `muted`; the `TODOS` label
  bold `muted` with its `closed/total`; todos drawn as everywhere else
  (`☒ ▣ ☐`, struck through when cancelled); a `faint` rule; then the body
  set like a reply. The text uses the chat's own column, with the same
  side padding, width limit and centering, so it does not move sideways
  when a plan opens over the chat.
- **Plan header**: under the title, after a blank row, come the plan's
  facts as `label value` pairs. The label is `muted` and the value
  `secondary`. An id is `muted` and a failed status is `error`. Pairs
  sit two columns apart. Who and when start one group: created,
  updated, by, asked by, assigned. Status, machine, branch, PR, project
  and model start a second group on a row of its own. A pair that does
  not fit moves to the next row whole, and one wider than the column
  has its value cut with `…`. Times read `14:32 · 2h ago`, with the date
  in front when it is not today. A fact with no source is left out, so
  a plan from before these were recorded shows only its status. The
  rows stay on the grid, like the `TODOS` label.
- Flowing text is prose, so the desktop sets it in its proportional face
  with inline code in monospace; code blocks, tables and rules stay on
  the grid.
- Opening at a todo scrolls it near the top once. `s` shows the source in
  the file view, on that todo's checklist item.

### Interactive Elements

- **Selected item**: `selection` fill, `on_selection` text
- **Hover state**: `surface_hover` fill
- **The wheel** scrolls a list's rows and never moves its highlight, so it
  never opens a channel or an agent; keys and clicks do. A list scrolled
  past its highlight stays put until the highlight moves or a key is
  pressed, then shows the highlight again. The lists in dialogs (the
  model selector, the folder browser) scroll the same way.
- **Clickable text** (records link, collapsed steps): `muted`
- **Links** in the chat: `muted`, with the ` (url)` after a label, and
  not underlined at rest. The pointer over a link underlines every row of
  it; a click opens web and mail links only.

## Code Examples

### Using Theme Colors

```rust
use crate::themes::Theme;

fn render_status(theme: &Theme, is_success: bool) -> Style {
    if is_success {
        Style::default().fg(theme.colors.success)
    } else {
        Style::default().fg(theme.colors.error)
    }
}
```

### Status Indicator Pattern

```rust
let (icon, color) = match status {
    Status::Ok => ("✓", theme.colors.success),
    Status::Error => ("✗", theme.colors.error),
    Status::Waiting => ("◐", theme.colors.warning),
    Status::None => ("·", theme.colors.faint),
};
```

## Do's and Don'ts

### Do

- Use semantic theme colors (`theme.colors.success`, etc.)
- Keep the chrome gray and let status carry the color
- Use `faint` for anything that frames content rather than being content
- Use red sparingly, only for actual errors

### Don't

- Use the accent for borders, tabs, selections, section titles or a
  header's info cell
- Use arbitrary RGB values not in the palette
- Use italics outside markdown emphasis
- Use color as the only indicator (add icons too)

## The Desktop Window

`aura-desktop` paints the same cell grid, so it follows this guide. Its
own title bars and window tabs take their colors from `grid::Palette`:
the focused active box is filled with `selection`, boxes at rest are
outlined in `faint`, hovered ones in `muted`, and the busy bar under a
window tab is the status accent. It draws `⬢ ☐ ☒ ▣` itself, since Geist
Mono has no glyphs for them, and HOST's `🖥` as a square box left of the
`*` box with a screen on a stand centered in it, across and down, and
BENCH's `🔧` as a square box at the agents bar's end with an upright
open-end wrench centered in it. Hovering either shows its name in a box
under it, over the grid.
