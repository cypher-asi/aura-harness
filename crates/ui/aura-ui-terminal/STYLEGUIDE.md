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
| `success`       | Cyan       | Live and working status (`●`, `⬢`), focused borders  |
| `warning`       | Blue       | Warnings, waiting on approval, non-agent modes       |
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
- Bold is for headings, the active tab, the live tail's `Working` and
  status label, and the to-do being worked on. Prompts, project names and
  code block labels are regular weight.

### Status Indicators

- **Ready (`●`)** and **working (`⬢`)**: `success`
- **Waiting (`◐`)**: `warning`
- **Error (`✗`)**: `error`
- **To-dos**: `☒` done in `faint`, `▣` in progress in bold `foreground`,
  `☐` pending in `muted`, cancelled `☐` struck through in `faint`. Past
  ten, the rest are counted on a `muted` `N more` row.

### Borders and Panels

- **Focused panel**: `success` border, and the input box's while the
  Chat panel has focus
- **Unfocused panel**: `faint` border
- **Menus and the `/todos` list**: `muted` border
- **Approval dialog**: `warning` border
- **Code blocks and step rules**: `faint`
- **Spacing**: panels side by side sit one blank column apart and one
  column in from the screen's edges. A cell is about twice as tall as it
  is wide, so that matches panels stacked on rows that touch. The side
  columns end level with the input box; the footer sits under the Chat
  column alone.

### Panel Headers

Every panel has its own header bar above its body: a bordered frame of
equal-width tab cells, then an info cell filling the rest of the width.
Most tab bars spread their tabs over the whole width. The Kernel panel's
KERNEL | MEMBERS | POLICIES bar keeps its tabs at their own width and
leaves the rest to the info cell, which shows the open tab's count (or,
on POLICIES, the agent's `MODE/APPROVAL`) on its right. Bars
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
┌──────────┬──────────┬──────────┬────────────┐
│  KERNEL  │▓MEMBERS ▓│ POLICIES │   3 AGENTS │
└──────────┴──────────┴──────────┴────────────┘
```

The MEMBERS tab lists the agent on screen, which leads its run, then the
agents the run brought in, each indented under the agent that started it.
Its rows use the Processes tab's status glyphs, and `lead`, `task` or
`agent` with the time worked in place of a row's time.

The POLICIES tab is a read-only document in sections (STATUS,
CAPABILITIES, TOOLS, ALWAYS ALLOW, SESSION GRANTS, INTEGRATIONS), each
headed in bold `primary` with a `muted` note after it. STATUS rows put a
`muted` label column before the value. TOOLS rows mark each tool `✓ on`
in `success`, `? ask` in `warning` or `✗ off` in `error`, followed by the
deciding rule in `muted`. What weakens the guardrails (`yolo`, commands
that can't be confined, no live prompts) is shown in `warning`, and the
header's `MODE/APPROVAL` info turns `warning` under `yolo`.

- **Frame and dividers**: the panel's border color
- **Active tab**: bold. While the panel is focused it is filled with `selection` and labelled in `on_selection`; otherwise the label is `foreground` with no fill
- **Hovered tab**: `surface_hover` fill with a `secondary` label
- **Other tabs**: `muted` label, no fill
- **Info cell**: no fill; counts in `foreground`, problems (trace issues) in `error`

Narrow bars drop the tab padding, then switch to three-letter labels
(`short_label`), and give the info cell's room to the tabs when it would be
under three columns.

### The Chat Column

```text
┌──────────────────────────────────────────────┐
│                                              │
│  → fix the login bug                         │
│                                              │
│    ▸ Explored 4 files, 2 searches            │
│      2 steps (click to expand)               │
│                                              │
│  ⬢ Working on 2 to-dos                       │
│    ☒ Read the handler                        │
│    ▣ Fix the redirect                        │
│                                              │
│  ⬢ Thinking… 12s                             │
└──────────────────────────────────────────────┘
┌──────────────────────────────────────────────┐
│ →  Add a follow-up                           │
└──────────────────────────────────────────────┘
 qwen3:4b · 23% context used · 2 calls  ● Ready
```

- The conversation is inset two columns, starts a row below the border
  and runs at most 110 columns wide.
- **Room below**: once the conversation reaches the bottom, a quarter of
  the chat's rows stay blank under its last line, so new text scrolls up
  with space below it instead of running into the border. The terminal
  pane's output runs to the bottom row.
- **User messages**: a `muted` `→ ` prompt, then `foreground` text
- **Replies**: `text`, list markers `muted`, inline code `secondary` on
  `surface`
- **Fields**: a list item that starts with a short label (`- Path: …`)
  has the label `muted`; blank lines between two such items are not
  drawn, and where the first label of a run of them comes back, a blank
  row sets the next record apart
- **Steps**: markers `faint`, the whole line `muted`
- **Live tail**: while a turn runs, the to-do checklist (`⬢` `faint`,
  `Working` bold `foreground`, the count `secondary`), then the working
  line flush with the reply: `⬢` in `success`, the label bold
  `foreground`, the elapsed time `muted`
- **Input box**: as wide as the Chat panel, bordered like it, with a
  `muted` `→ ` prompt and a `muted` placeholder (`Ask anything`, then
  `Add a follow-up`) set one column past the cursor so the cursor never
  covers its first letter. Shell and login prompts replace the arrow.
- **Footer**: the mode outside agent mode, the approval level, the model
  and what the agent has used in `muted`, separated by `faint` dots,
  with the status at the right, on the screen's last row. Parts that
  don't fit drop from the right.

### Interactive Elements

- **Selected item**: `selection` fill, `on_selection` text
- **Hover state**: `surface_hover` fill
- **Clickable text** (records link, collapsed steps): `muted`

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

- Use the accent for borders, tabs or selections
- Use arbitrary RGB values not in the palette
- Use italics outside markdown emphasis
- Use color as the only indicator (add icons too)

## The Desktop Window

`aura-desktop` paints the same cell grid, so it follows this guide. Its
own title bars and window tabs take their colors from `grid::Palette`:
the focused active box is filled with `selection`, boxes at rest are
outlined in `faint`, hovered ones in `muted`, and the busy bar under a
window tab is the status accent. It draws `⬢ ☐ ☒ ▣` itself, since Geist
Mono has no glyphs for them.
