# Changelog

All notable changes to the Aura agent-first architecture refactor
are tracked here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the
project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Benchmarks — two arms on the same work

##### Added

- The chat panel has a BENCH tab (`/bench`, `/bench new`) that lists every
  comparison and sets new ones up with a form: the work (your last prompt,
  a typed prompt, or one of this chat's sessions), arm A, arm B, runs per
  arm, check and budget, then the estimate and a confirmation. Opening one
  shows both arms side by side, each prompt's cost and checks, the judge's
  verdicts, and the runs to diff, apply or pick.
- `/compare --session current|<ms>` replays a session prompt by prompt in
  each copy, starting from the files as they were when the session started
  (put back from its file checkpoints). The view shows the session's own
  cost, each arm's cost per prompt, and how many of each prompt's turns
  passed or failed their tests.
- For a replayed session, the blind judge also sees what you kept (the
  session's diff and last reply) and rates each run better than, as good
  as, or worse than it.

##### Changed

- `/compare` arms are A and B, each `gate`, `gate:nodelegate` or a model
  (`--a`, `--b`; `--against` still names arm B), so two models can be
  compared without Gate. Runs are named `a-1`, `b-2`, … and `/compare pick`
  takes `a` or `b`. Older comparisons read as Gate (A) against their other
  arm (B).

### Terminal and desktop — coordinating agent and tasks

##### Added

- In `agent` mode the main agent coordinates: its prompt has it plan with
  `todo_write`, hand each non-trivial item to the best-fitting permanent
  agent with `task`, and keep small edits and simple answers to itself. The
  `task` tool lists each permanent agent with its description, model and
  tools.
- `task` takes a `todo_id`. The item goes in progress, assigned to the agent
  it was handed to (`TodoItem.assignee`, shown as `→ coder`), as soon as the
  call is made. It is done, or pending again on failure, when the agent
  reports back. A detached task reports back through a
  `<task_report todo_id=… ok=…/>` line on its notice. The assignee is
  mirrored into the plan file and replayed from history.
- The parent's record gets a `SubagentSpawn` entry for each `task` call,
  with its `tool_use_id`, `subagent_type`, child name and `todo_id`.
- A TASKS tab next to PLANS in the side panel. Each request whose turn
  calls a tool is a task, and each `task` call is a task under it, done by
  the spawned agent. A task's items follow its agent's todo list. Tasks
  link to the plan and todo they work on, and are kept in
  `.aura/tasks/<id>.json` across restarts. Enter shows the task's agent,
  `p` opens its plan; `t` on a PLANS todo jumps to its task.

##### Changed

- Spawned agents in the projects list are joined to the agent that spawned
  them by tree lines.
- In the GRAPH tab a `task` row's wire runs to the agent doing it, colored
  by how the task went. The agent's card ends `↩ reported to <agent>`, or
  `↩ failed for <agent>`, where its report runs back to the caller.
- With five tabs, the side panel shows short labels (`SBX`, `PRC`) for
  tabs whose full label doesn't fit its cell.

### Reading code with less context

##### Changed

- `read_file` returns numbered lines (`  12|text`, the number as wide as
  the last one shown) for whole files too: at most 2000 lines and 32 KB
  (`ToolConfig::max_read_bytes`, was 64 KB), cut at a whole line, ending
  with the `start_line` to continue from. Lines over 2000 characters are
  cut, and binary files are described instead of dumped. The schema no
  longer offers `max_bytes` or claims a 1 MB default.
- `search_code` and `find_files` skip what the project's `.gitignore`
  (and `.ignore`, git's excludes) leave out, in a git repository or not,
  and walk in name order. `search_code` gains `output_mode`:
  `files_with_matches` and `count` locate matches without printing them.
  It returns at most 500 results whatever `max_results` asks for, and
  cuts matched lines over 300 characters.
- `find_files` lists the most recently modified matches first and says
  when it stopped at 200; `list_files` stops at 500 entries and says so.
- A `run_command` output over its limit keeps its first 3/8 and its last
  5/8 around a note of how much was cut, instead of only its start, so
  errors and test summaries printed last survive.
- Every agent loop leaves its transcript untouched below 70% context
  use (`AgentLoopConfig::compaction_trigger` now defaults to
  `aura_config::COMPACTION_TRIGGER_DEFAULT`), as the terminal already
  did; the node's chat sessions, automatons and spawned agents used to
  start trimming old tool results at 15%.
- Compaction folds a `read_file` result once a later, complete read of
  the same path shows the same lines (`supersede_older_reads`), so a
  file edited and read again is not carried twice.
- The run cache keys `search_code` on `output_mode`, `max_results` and
  `file_pattern` too.
- The chat system prompt asks the model to locate before reading, read
  ranges, batch independent reads and hand broad exploration to an
  explorer subagent.

### Terminal and desktop — chat restyle

##### Changed

- The default `cyber` theme is graphite and gray. The cyan accent is kept
  for status (`● Ready`, the working `⬢`, busy tabs) and the focused
  panel's border; other borders, tabs, selections and menus are neutral. Themes gain a `faint` color for
  borders at rest, markers and separators. Errors and failed counts use a
  softer red (`#e06c75`).
- The input is a box under the Chat panel, as wide as it, with a `→`
  prompt and an `Ask anything` / `Add a follow-up` placeholder. The model,
  approval level, usage (`23% context used`) and status moved to a footer
  under it, on the screen's last row. The side panels end level with the
  input box.
- While a turn runs, its to-do checklist (`☒ ▣ ☐`) and a `⬢ Thinking…`
  line follow the conversation, replacing the progress strip on the Chat
  panel's border. The current to-do is bold, finished ones fade, and past
  ten the rest are counted (`3 more`). `/todos` uses the same boxes.
- The chat is inset two columns, starts a row below the border and stops
  at 110 columns. User prompts start with `→`. Steps are gray throughout
  and a collapsed group reads `6 steps (click to expand)`. Inline code is
  light gray like the reply. Steps, hints and notices are no longer
  italic, and project names and code block labels are no longer bold.
- The header names linked plans in capitals (`CLAUDE MAX 20X · CHATGPT
  PRO`), like the title bar and tabs.
- The desktop window's title bars and tabs fill the selected box in gray
  instead of the accent, and it draws `⬢ ☐ ☒ ▣` itself.

##### Added

- The chat panel expands to fill the window: the `⤢` at the right end of
  its header bar, `Ctrl+X` or `/expand` (`/zoom`, `/fullscreen`) hides
  the other panels, and `⤡` or the same key brings them back. CHAT,
  TERMINAL, GRAPH and a file or record over the chat all expand; over a
  file or record the button sits left of its `✕`. The desktop window
  draws it as a box of corner brackets. `PaneChrome::expand` and
  `App::expand_pane` let other front ends draw and work it.

### Context compaction

##### Changed

- A tool result longer than 32 KB (`aura_config::TOOL_RESULT_MAX_BYTES`)
  enters the transcript as its first and last 16 KB around a marker
  saying how much was cut and how to read the rest, so later requests
  never resend the whole of a huge `search_code` or `read_file` output.
  Events and records still carry the full result.
  `AURA_AGENT_DISABLE_COMPACTION` turns the cap off along with the
  rest of compaction.
- Compaction tiers trim harder as the context fills: the 15% tier now
  cuts older tool results to 3,000 characters and keeps the last 8
  messages, and the 30% tier cuts to 1,500 and keeps 6 (they were the
  other way round). `CompactionConfig::micro()` and `history()` are
  renamed `emergency()` and `minimal()`, and `OverflowStep::MICRO` is
  `OverflowStep::EMERGENCY`.

### Gate router (`aura-agent-gate`)

Gate is now one learning router and the default model. The coordinator
mode that used to share the name is gone. See [Gate](docs/cli.md#gate).

##### Breaking changes

- The coordinator mode is removed: `gate/<model>` ids, `run_blocks`,
  draft-first turns, the tier ladder, the ledger in `AURA_HOME/gate/`
  and the `aura gate report|fit|bench|promote` subcommands.
- `AURA_GATE_POLICY`, `AURA_GATE_EXPLORE_BUDGET_USD`, `AURA_GATE_TIERS`
  and `AURA_GATE_LEDGER` are no longer read. `AURA_GATE_EXPLORE` now
  sets Gate's exploration rate (default `0.05`).
- The payload of a `UserPrompt` transaction is a JSON `PromptPayload`
  (`text`, `category`, `classifier`, `route`, and `kind` for subtasks)
  instead of raw text. There is no backward compatibility: prompts
  recorded before this change read back as empty text. `POST /tx` wraps
  submitted prompt text in a payload.
- New `TransactionType::Outcome` and `SystemKind::Compare`. Consumers
  that mirror these enums must add them.
- The terminal starts on `gate` instead of the local default model, and
  a pinned model that loses its sign-in switches to `gate`.
  `AURA_DEFAULT_MODEL` and a model picked in `/model` still win.

##### Added

- `gate` model id, first in `/model`, accepted by `--model gate`. Gate
  routes among the models your sign-ins allow: installed local models,
  aura-router models when signed in to zOS, `claude/*` with a Claude
  plan, and `chatgpt/*` with a ChatGPT plan. It keeps one route per
  model (the plan first) and tiers them `local`, `fast`, `standard` and
  `frontier`. A model whose route refuses the sign-in is dropped and the
  turn carries on with the next pick. Each turn shows
  `gate → <model> (<route>) · <category>, <reason>`, and the status line
  at the bottom right shows `GATE` in place of the model.
- The footer under a reply covers the whole turn: every model that ran,
  with Gate's calls and the chat title marked, and the tokens and cost
  of all of them, e.g. `[claude-sonnet-5-5 ×2 · gate: claude-haiku-5-5 ·
  title: claude-haiku-5-5 · 27.9k tok · $0.040]`. It used to show the
  last call only. The classifier, the judges and the title call record
  their `purpose` in their `Reasoning` entry, so a reloaded chat shows
  the same footer. Chat titles are written by the classifier's model
  instead of a fixed Haiku 4.5 (`AURA_TITLE_MODEL` still overrides it).
- Every prompt is classified into one of 8 categories by the cheapest
  fast-tier model, through the agent's kernel. Every turn ends with an
  `Outcome` (`error`, `checks_failed`, `checks_passed` or `none`), and
  the judge's findings, rewinds and stronger retries append later ones.
- Gate routes each category to the cheapest model with
  `AURA_GATE_MIN_TRIES` (10) counted outcomes at `AURA_GATE_TARGET`
  (85%) success. Otherwise it uses the category's default, and it
  explores the next cheaper model at `AURA_GATE_EXPLORE`. The table is
  rebuilt from the record at startup and updated as each outcome is
  committed. `/gate` shows it with the current picks, the delegation
  figures and the last 20 routing decisions.
- Subtasks under `gate` are classified and routed with their own table
  rows. Models named by the call, the permanent agent or a council seat
  stay pinned.
- Under `gate` the main agent leads the turns classified
  `multi_file_change` or `planning`, and does the rest itself on the
  model Gate routes it to. On a turn it leads it gets the `gate_lead`
  prompt section and, unless `AURA_GATE_DELEGATE=off`, no file-changing tools,
  capped reads and searches, and a `run_command` limited to checks and
  inspection. New `implement` and `tests` permanent agents (and bundled
  kinds) take the work. `task` gains a `check` command and returns a
  compact result. Read-only agents run in parallel and the rest one at
  a time. A routed subtask that fails its check is retried once on the
  next tier up with its file changes undone. In a git repository that
  includes changes made by shell commands: the workspace folder goes
  back to a snapshot taken before the attempt, leaving ignored files,
  the index, `HEAD` and stash alone.
- `/compare [prompt | --plan f] [--against m | gate:nodelegate]
  [--repeats N] [--check cmd] [--budget $]` runs a task with Gate and
  another model in copies under `AURA_HOME/compare/`, judges each pair
  blindly, and shows time, cost, tokens per model and check passes per
  arm. Subcommands: `go`, `cancel`, `show [n]`, `diff [run]`,
  `pick gate|other`, `apply <run>` and `dismiss`. On a Claude or ChatGPT
  plan it also shows the share of each plan window each arm used, read
  from the usage headers of each response and recorded with the model
  call; each rise between readings goes to the run whose call saw it,
  so parallel runs don't count it twice. Providers that send no such
  headers show tokens per model instead.
  The check runs as a `run_command` call through each run's kernel, so
  it is in the run's record and under the same command policy. Each run
  is unattended in its own copy and runs in `agent` mode at `yolo`.
- `evals/gate_cost`: 20 tasks with hidden checks, categories and
  reference solutions. Eval tasks can set `category`, `tags` and a
  `checks/` folder copied in after the run; `aura eval run` reports each
  run's cost and takes `--budget-usd`. `evals/gate_hard` adds eight
  tasks with traps a careless fix falls into.
- `aura eval run --model gate` runs each task as `aura -p` turns: Gate
  classifies and routes them, the agent leads and delegates where the
  category calls for it, and the
  verify command runs as a `run_command` call through the run's agent's
  kernel, its verdict recorded as a later outcome so eval results count
  in Gate's table. The results table shows the models Gate routed each
  task to. `EvalOptions` takes an `EvalRunner` instead of `config`,
  `tool_config` and `provider`. The runs' agents work at `yolo`, like
  `aura -p` in `agent` mode.
- Gate prices a model it has not tried from its list price scaled by how
  the tried models of its tier (else the nearest tier) cost against
  theirs, so newer, cheaper models are no longer ranked behind older
  ones with history. When the lead is on a frontier model, its subtasks
  are first routed below the frontier tier; an escalated retry may still
  reach it.
- Gate starts work cheap and moves it up on failure. `small_change`
  starts on the cheapest fast-tier model, `planning` on the strongest
  and every other category on the cheapest standard-tier model. A start
  model that can no longer reach the target in a category hands it to
  the next tier up. A routed turn that fails through the model's own
  doing goes on in the same conversation on the next tier up (route
  reason `escalate`, not counted), in the terminal, `aura -p` and
  `aura eval run`.
- Gate's judge reviews failed work. When a routed turn or a subtask's
  first attempt ends in a model error or with its last test or build
  failing, the strongest Opus you can use is shown the request, the
  diff, the final reply and the failure, and says whether the model was
  at fault. If so (`judged_fail`) the failure counts in Gate's table and
  the work moves up a tier, told what the judge found. If not
  (`judged_excused`) it counts for nothing and stays where it is. Work
  that did not fail is never judged, so the judge costs nothing on a
  clean run; `aura eval run` reports its cost per task and in total, and
  `aura -p --output-format stream-json` emits a `judged` event.
  `AURA_GATE_JUDGE=off` switches it off.
- Exploration now goes to the nearest cheaper model the category has
  not ruled out, else the nearest, so a ruled-out model is tried again
  once nothing cheaper is more promising.
- `/good` and `/bad` are removed. How a turn went is learned from its
  own signals and the judge; `/compare pick` still records your pick.
- A `task`, `run_command` or plugin tool call drops the caller's cached
  tool results, so the next read shows what is on disk
  (`aura_config::FILE_NEUTRAL_TOOLS` lists the uncached tools that keep
  the cache).
- On Claude models the thinking effort stays at medium for the whole
  run. Lowering it after the first write made Anthropic rewrite the
  conversation's prompt cache.
- `aura eval run` prints one line per task on stderr as it starts and
  ends (verdict, cost, time, spend so far, routed models), and with
  `--model gate` the classifier and the models Gate routes among.
- The Claude plan's live model list is saved as `plan_models.json`
  beside the model preferences and used when a later fetch fails, with
  a note saying so.
- Under any approval level, the lead's `run_command` limit only narrows
  what the level lets run, and a `task` `check` runs through the
  subtask's kernel at the level the subtask inherited from its parent.
- `aura-app`'s `isolation` module holds the worktree and copy helpers
  `/compare` runs in and the change carrying, diffing and scratch-index
  snapshots `/compare` and the escalation use.

### Terminal — GRAPH tab

##### Added

- The chat panel has a third tab, GRAPH, next to CHAT and TERMINAL
  (Shift+←/→ or a click). It shows the flow between the agent on screen
  and every agent below it as their kernels recorded it, as cards wired
  together on a canvas: a main card per turn, with its prompt and
  replies, and under it on a spine a subcard per model call with its
  tool calls as rows. A `task` row shows what came back from it, and
  the agent that took the work has turns of its own, wired from that
  row. Each agent's turns go down a column under its name until the
  view has no room for the next, then on at the top of the next column;
  agents that work in turn share a band, and bands share the view's
  height. Wires never run backwards.
- Model calls and turns have a line of their own with the tokens read
  and written (the whole prompt, cached or not), the cost and the time;
  a turn's adds up its calls.
- A step opened as JSON says when it happened to the millisecond, and
  which agent spawned its agent. A turn's prompt also shows the turn's
  totals: steps, tokens, cost and time.
- The calls the harness makes around a turn are subcards of their own
  kind, muted: `gate` (classifying the prompt, the first step of its
  turn), `judge` and `title`.
- The tab follows the agents live. Turns are open until folded with the
  `▾` button in a card's title (or Space), and stay as they were put as
  steps arrive and when the agent is shown again. `↑`/`↓` go through a card's
  lines and on to the card above or below, `←`/`→` along the agent's
  turns, `[` / `]` jump between prompts, `f` follows a wire to the
  other agent (opening what is folded over its end), Space folds a
  model call's tool calls or a turn's model calls, `z` folds every
  turn, Enter or a second click opens a step as JSON, `r` opens its
  record entry and `o` switches to its agent. Dragging empty canvas or
  the wheel pans the view over a canvas without edges; `0` brings it
  back to the selection. See
  [The GRAPH tab](docs/cli.md#the-graph-tab).
- The desktop app draws the canvas itself (`App::set_host_canvas`):
  square cards on the window's background, right-angled wires,
  and a view that drags by the pixel, pans smoothly on a touchpad and
  zooms from 25% to 200% around the pointer (Ctrl+wheel, a pinch, or
  `+`/`-`/`0`). Only what is on screen is drawn; text sizes snap to
  half a pixel and turn to bars below 50%, so zooming stays fast. The
  terminal UI leaves the
  canvas cells blank and says which of them no menu or dialog covers
  (`App::flow_canvas_clip`), so those stay on top.
- `UiEvent::OpenFlow` asks the session for an agent's flow;
  `UiCommand::FlowLoad` and `UiCommand::FlowUpsert` carry it as
  `FlowLane`s and `FlowNode`s (`aura_ui::flow`). The session builds it
  in `event_loop/flow.rs` from a record observer on every kernel and
  the record store, off the kernels' and the event loop's tasks. Only
  the boxes on screen are drawn.

### Many agents at once

##### Fixed

- The desktop app no longer slows down and stops answering input while
  many agents stream at once. It rendered the UI once per command the
  agents sent, which with 30 agents took more than the whole second.
  Commands are now applied as they arrive and the window renders at
  most once per frame of the display: a second of 3,000 commands from
  30 agents costs about 50 ms of the window's thread.
- The desktop app shapes only the rows of the grid whose content is
  new. Any change, such as a streamed word or a ticking timer, used to
  repaint and re-shape every row; a row that is unchanged, or that only
  scrolled, now reuses the quads it was drawn with. A frame that comes
  out the same as the one on screen is not shown again, so an idle
  window draws only when the cursor blinks. `AURA_DESKTOP_PERF` reports
  the rows painted anew and the frames skipped.
- The Sandbox tree is read again once per tick rather than after every
  tool call of every agent.
- The terminal UI takes the agents' commands every 5 ms while it waits
  for input, rather than once a frame, so a busy swarm no longer fills
  the command channel and holds up the agents and the event loop.

### Chat titles

##### Added

- Agents with a placeholder name (`Agent`, `Agent N`, `Terminal Agent`)
  get a short title for their chat after their first turn, such as
  "Task build issue", written by Claude Haiku and shown in the agents
  list and the Processes tab. The title is kept in `agents.json`; the
  agent's name and identity don't change. Names the user picked and
  permanent-agent instances are left alone. `AURA_TITLE_MODEL` picks
  another model.

### Projects list

##### Added

- A "+" at the right end of a project's row adds an agent to the project.
  Agents added to a project, this way or by adding a permanent agent to
  it, are listed at the same level as its first agent; only agents
  another agent spawned are indented under it. Registries that linked
  added agents under the first agent are moved on load.
- An ARCHIVE tab in the agents panel. Delete on an agent in PROJECTS (or
  `/archive`) moves it there with the agents under it, and Enter brings
  it back. Shift+Delete (or `/remove-project`) removes the project from
  the list after a `y`; its records and folder stay on disk.

### Approval levels

##### Added

- An approval level per agent, shown as a badge before the model name in
  the terminal and desktop status line, and picked with `/approval
  <level>`, Shift+Tab (`yolo` → `ask` → `strict`), `--approval` or
  `AURA_TUI_APPROVAL`:
  - `ask`: reads run; everything else asks unless a
    project "always allow" rule covers it.
  - `strict`: everything but reads asks, "always allow" rules are
    ignored, and prompts offer no "always" answer.
  - `yolo` (the default): nothing asks, and the agent's commands can write only inside
    the working folder, temp folders and package caches. On Linux this
    is enforced with Landlock; elsewhere commands still ask unless they
    plainly stay in the folder. `AURA_YOLO_WRITABLE_PATHS` adds folders.
- Spawned agents start at their parent's level, and every level change is
  recorded in the agent's `mode_change` entries (`agent/ask` →
  `agent/yolo`).

##### Changed

- `/mode` now picks only the work mode: `agent`, `plan`, `read-only`
  (formerly `/mode ask`) or `debug`. The old `default`, `accept-edits`
  and `unrestricted` modes are gone; `/mode unrestricted` points to
  `/approval yolo`, saved sessions load them as `agent`, and
  `AURA_TUI_MODE` set to one of them is ignored with a warning.
- Shift+Tab cycles the approval level instead of the mode.
- `aura -p` in agent mode runs at `yolo`, so on Linux its commands can no
  longer write outside the working folder. `--approval ask` or `strict`
  denies edits and commands instead.

### Workspace layout

##### Changed

- Crates live in one folder per layer, `crates/<layer>/<crate>`, with
  layers ordered from most to least fundamental: `core`, `config`,
  `store`, `model`, `context`, `plugin`, `exec`, `agent`, `auth`,
  `engine`, `ui`, `surface`. `auth`, `engine` and `ui` are new layers;
  their crates were previously classified as `surface`, and
  `aura-exec-domain-http` moved to `exec`.
- Every crate below `surface` is named `aura-<layer>` or
  `aura-<layer>-<part>`. Renamed, with no compatibility aliases:
  - `aura-protocol` → `aura-core-protocol`
  - `aura-plugin-core` → `aura-plugin`
  - `aura-tools` → `aura-exec-tools`
  - `aura-domain-http` → `aura-exec-domain-http`
  - `aura-ui-core` → `aura-ui`
  - `aura-terminal` → `aura-ui-terminal`
  - `aura-runtime` → `aura-node` (the binary keeps its `aura-node` name)
  - `aura-model-reasoner` → `aura-model`
  - `aura-automaton` → `aura-engine-automaton`
- `aura-surface-cli` split into `aura-app`, the session host shared by
  the terminal, the desktop app and `aura -p`, and `aura-cli`, the
  command line. `aura-desktop` now depends on `aura-app` instead of the
  CLI.
- `ReadStore`, `DequeueToken` and `StoreError` moved to
  `aura-store-record`; RocksDB errors surface as `StoreError::Backend`.
  The tool hook traits, `AutomatonController` and `DomainApi` moved to
  `aura-exec-traits` (still re-exported from `aura-exec-tools`). The
  `/ws/terminal` PTY handler moved from the terminal UI crate into the
  `aura-node` gateway.
- The TypeScript bindings are exported with
  `cargo test -p aura-core-protocol --features typescript export_typescript_bindings`.
  `aura-os` still calls its mirror of the wire types `aura_protocol`, so
  the two no longer share a name.

##### Removed

- `aura-exec-sandbox` and `aura-exec-isolation`. The filesystem
  sandbox the tools use is `sandbox.rs` in `aura-exec-tools`, and the
  workspace copies Gate's `/compare` and escalation use are `aura-app`'s
  `isolation` module.
- The old `aura-core-protocol` placeholder, which only held
  `ProtocolVersion`. It was merged into the wire protocol crate, which
  now carries the `aura-core-protocol` name.

### Run traces (`aura-agent-trace`)

##### Added

- Every terminal and desktop conversation is recorded as a run under
  `AURA_HOME/runs/`, one run per agent conversation, with a scorecard
  written after each turn. Kernel snapshots are kept with the runs so
  `aura runs` can replay them. See [`docs/run-traces.md`](docs/run-traces.md).
- `/graph [id]` opens a run's live graph (step labels, record and file
  links), `/runs` lists recorded runs and `/status` shows this
  conversation's scorecard. The status line shows each model call's
  iteration and running token count.
- `aura runs` (list, show, diff, label, verify, branch), `aura dataset
  build` and `aura eval run/compare` with the `evals/coding` suite.
- The `/graph` view says when each node happened, to the millisecond,
  and what model calls, turns and the run cost. A model call's tokens
  count its whole prompt (they double-counted cached tokens on OpenAI
  models). A call cut off at `max_tokens` is a warning, and a turn
  that ended on a failed call shows it as a failed iteration
  (`turn_end` steps gain `error`). Tool calls show the kernel's policy
  decision (verdict, rule, approver, reason), live too
  (`RunRecorder::subscribe_records`). A `task` call names the run of
  the agent it started and `o` opens it; `o` on a spawned run's root
  node opens its parent. The run node lists every `meta.json` field,
  and missing values show as `null` instead of blank.

### Surface — native desktop app (`aura-desktop`)

##### Added

- `aura` (and so `cargo run`) opens the desktop window when given no
  subcommand; `aura --terminal` (alias `--tui`) or `aura run` runs in the
  terminal, as does `aura` with no display (e.g. over SSH). `aura desktop`
  takes the window's options (`--workspace`, `--demo`, `--theme`, …).
  The window comes from the root package's default `desktop` feature;
  the Docker images and the 1.85 MSRV check build without it.
- Tabs in the desktop window, which work as workspaces. A strip above
  the grid has a numbered box per tab, a `+` box, and a wide box naming
  what the tab in front shows. Each tab holds its own projects and
  agents and shows only those, in its own `aura_terminal::App` with its
  own draft, panels, overlays, scroll and selection, all on the one
  session. The window starts with every project in one tab; `+` starts
  an agent in a new tab, and a project created in a tab stays in it.
  Ctrl+Shift+T / Ctrl+Shift+W open and close tabs (Cmd+T / Cmd+W on
  macOS), Ctrl+1…9 or Alt+1…9 (Cmd+1…9) go to a tab by number, and
  Ctrl+Tab / Ctrl+Shift+Tab move between them. Closing a tab moves its
  projects to the tab before it. The split is kept in `desktop_tabs` in
  the data directory. `UiEvent::FocusAgent` makes an agent the
  session's active one without reloading its chat.
- `aura-desktop`, the terminal UI in a native window (winit) that
  draws itself: a frame is one list of quads over a glyph atlas, drawn
  by the GPU in one call (wgpu) or by the CPU where there is no GPU.
  No UI toolkit is involved. It runs the same in-process session as the terminal (`session::start`) and the
  same `aura_terminal::App`, rendered through a ratatui backend into a
  cell grid, so the screen, keys, mouse selection and copy behave as
  they do in a terminal.
- The grid is painted in Geist Mono (bundled) with one text draw per
  styled run; box drawing, block elements, braille and the status
  symbols are drawn as shapes, so borders join and glyphs missing from
  the font still render. A row's quads are kept until its content
  changes, and glyphs and drawn shapes are rasterized once into an
  atlas.
- Window shortcuts: Ctrl+Shift+C copies, Ctrl+V / Ctrl+Shift+V /
  Shift+Insert paste (images attach, as in the terminal), Ctrl+= /
  Ctrl+- / Ctrl+0 zoom (Cmd on macOS). Turn alerts use the terminal's
  settings, with the window asking for attention in place of the bell.
  `--theme` picks the terminal theme; `AURA_DESKTOP_MONO_FONT`,
  `AURA_DESKTOP_FONT_FILES` and `AURA_DESKTOP_FONT_SIZE` change the
  font. `--demo` runs a scripted session and `AURA_DESKTOP_PERF` logs
  frame timings. `AURA_DESKTOP_RENDERER=cpu` draws on the CPU whatever
  the machine has, and `=gpu` on whatever adapter there is.
- Offscreen snapshot tests draw whole screens with the CPU rasterizer;
  `AURA_DESKTOP_SNAPSHOTS=<dir>` saves them as PNGs, and
  `AURA_DESKTOP_GPU=1` checks the GPU draws the same pixels. Ignored
  `replay_timings` and `swarm_timings` tests report boot, streaming,
  keystroke and frame timings, and a second of thirty agents streaming
  at once; `history_timings` reports frames of a 2,000-message
  conversation and of a long reply streaming after it, `scroll_timings`
  reports frames of a long chat and 400 agents scrolling, and
  `aura-ui-terminal` has `history_timings` and `streaming_timings` for
  the UI alone.
- `aura_terminal::render`, `App::paste_from_clipboard` and
  `alerts::desktop_notify` are public so other frontends can draw the UI
  and share its clipboard and notifications.
- Panels are window panes: the window draws their title bars itself,
  with tabs packed to the left, each as wide as its label and always
  outlined, the shown one filled (in the accent while its panel has
  focus). Panels without tabs show their title in a filled box and their
  info at the right. Dragging the border between two panels resizes
  them. `App::set_host_chrome`, `App::pane_chrome`,
  `App::set_pane_splits`, `App::select_pane_tab` and `App::focus_pane`
  let other frontends do the same.

##### Changed

- The CI MSRV check skips `aura-desktop`, which needs Rust 1.88.
- The window's clipboard is `arboard`'s.
- A reply is laid out a part at a time while it streams, in both
  frontends: what comes before its last blank line or closed code block
  is kept, and only the paragraph, list, table or code block still being
  written is wrapped again. A frame after a chunk costs the same however
  long the reply is: about 0.5 ms in the window at 76,000 characters,
  where it was 3.9 ms. The reply is laid out whole once more when it
  ends.
- The chat reads like a terminal in both frontends: it starts at the
  panel's left edge, prompts are echoed as `> text`, replies follow
  without `YOU` / `AURA` headers, a dim `[model · tokens · cost]` line
  closes each reply, and notices start with `*`.
- The window draws everything in Geist Mono, chat prose included, with
  a block cursor; the proportional Geist faces are no longer bundled.
- The chat scrolls by pixels in the window. A touchpad moves it twice
  as far as the fingers do, and a notch of the wheel eases five rows
  over a few frames; notches in quick succession go further each time,
  up to three times as far, and turning the wheel back stops what is
  easing out. `AURA_DESKTOP_SCROLL_SPEED` scales all of it. The
  projects list scrolls three rows a notch, like the other panels.
  The terminal UI still scrolls by whole rows. Between
  rows, the window draws the chat's body higher than its cells, cut to
  the body, and shows the row below the body coming in. A frame that
  only slides the body is painted from the rows already rendered, with
  no render of the terminal UI. A click goes to the row drawn under
  the pointer. Other panels still scroll by whole rows.
  `App::set_smooth_scroll`, `App::chat_scroll`, `App::chat_row_below`
  and `App::scroll_chat_rows` let other frontends do the same.
- Rows are painted and kept in pieces, cut at the panels' borders. A
  panel that scrolls leaves the pieces of the panels beside it as they
  were. A notch of the wheel over the chat used to paint 2,161 rows
  anew over 160 frames; now it paints 322 pieces.
- The projects list and the archive format only the rows on screen, so
  hundreds of agents cost what fits in the panel.
- Moving the pointer only asks for a frame when what is under it
  changes: a highlight, or a selection being dragged.
  `App::handle_mouse` returns whether the screen may look different.

### Surface — UI protocol and agent session shared by every frontend

##### Added

- `aura-ui-core`: the `UiEvent` / `UiCommand` protocol, input history,
  tool-call formatting, the sandbox file tree and the `/mode` parser,
  with no ratatui dependency. `aura-terminal` re-exports them under the
  same paths (`aura_terminal::events`, `aura_terminal::mode_command`,
  `aura_terminal::input::InputHistory`).
- `aura_surface_cli::session::start`, which opens the agent registry,
  record store, model provider and kernel factory and spawns the event
  loop, returning a `UiSession` (event sender, command receiver and
  `SessionTask`). `SessionTask::finish` stops background commands,
  surfaces an event loop that stopped on an error and aborts one still
  running.

##### Changed

- `aura` (terminal mode) runs on `session::start`. The terminal now
  enters the alternate screen after the session is ready, so a startup
  error prints to the normal screen.

### Headless — `aura -p`

##### Added

- `aura -p "prompt"` (`--print`) runs one prompt without the terminal
  UI, prints the result and exits. `-p -`, or `-p` with no prompt, reads
  the prompt from stdin; piped stdin is appended to a given prompt.
- `--output-format text|json|stream-json`: the final reply; one result
  object with the reply, session id, model calls, usage (cache reads
  and writes included), cache hit rate, estimated cost, compactions,
  todo list, changed files and exit reason; or one event per line (text
  and thinking deltas, tool calls and results, todo updates, per-call
  usage, warnings) ending with that object.
- `--resume <session_id>` and `--continue` pick up a saved session.
  Runs belong to a dedicated "Headless" agent, so they never replace
  the session of an agent the terminal shows, and the terminal can
  `/resume` them.
- `--allowed-tools` / `--disallowed-tools` filter both what the model is
  offered and what the kernel permits. Nothing prompts: a tool call the
  policy would ask about is denied.
- `--max-turns`, `--max-time` and `--max-cost-usd` limit the whole run
  (the time and cost limits default to `AURA_TUI_TURN_TIMEOUT` and
  `AURA_TUI_MAX_TURN_COST_USD`). A limit stops the agent between steps
  and keeps its work in the session.
- `--cwd`/`-d`, `--model` (not saved as the `/model` choice),
  `--provider` and `--verbose`. The global `--mode` applies: `plan`,
  `ask` and `debug` act as the terminal's modes with every approval
  denied, and `agent` lets edits and commands run instead of asking.
  Agents spawned with `task` keep the run's mode and tool filter.
- Exit codes: 0 finished, 1 failed, 2 bad arguments, 3 stopped by a
  limit, 130 interrupted.
- The run waits for the agent's background work after its turn
  (background commands, detached `task` spawns and agents it started
  with `spawn_agent`; up to `AURA_HEADLESS_JOB_WAIT`, default 10
  minutes) and gives it each exit notice or reply as a new turn, then
  stops any command still running.
- stdout carries only the chosen output; logs go to stderr.
- `evals/long_horizon/`: four multi-file feature tasks (three Python,
  one Rust) with hidden tests, and `run.py`, which runs `aura -p
  --output-format json` on each task in a scratch copy, grades it with
  the hidden tests, and writes a JSON and Markdown report with pass/fail,
  cost, cache hit rate, compactions, model calls and wall time. Real
  models run only with `--live` or `AURA_EVALS_LIVE=1`; `--self-check`
  verifies the fixtures against reference solutions without a model.

##### Fixed

- Every model call now reports its tokens and cost to the turn's
  listeners. The streaming transport never did, so the terminal's
  per-reply cost, status line cache and cost figures, and
  `AURA_TUI_MAX_TURN_COST_USD` saw no model calls.

### Terminal — long-running tasks

##### Added

- `todo_write` tool: the agent keeps a plan for multi-step work. The
  loop tracks the list, puts it back after a history summary, and
  carries it across turns and restarts.
- A turn that ends with todo items still open is sent back to the next
  one. It ends anyway when the agent asks the user something, says it
  is blocked, or two nudges in a row leave the list unchanged.
- A message sent to a busy agent goes into its running turn instead of
  waiting for the turn to end. One that arrives too late runs next.
- `run_command` takes `background: true`. The command keeps running
  after the call returns and its output goes to a log file. When it
  exits, the agent that started it gets the exit code and the end of the
  log, in its running turn or in a new one, whether or not it is on
  screen. `job_output` reads a job's log and `job_kill` stops it (a job
  the agent stopped itself sends no message). Quitting the terminal, or
  the end of an `aura -p` run, stops every background command.
- `AURA_TUI_TURN_TIMEOUT` and `AURA_TUI_MAX_TURN_COST_USD` set optional
  per-turn time and cost limits.
- Anthropic requests mark the last two user messages for prompt
  caching (four breakpoints with the system prompt and tools), so a
  large batch of tool results no longer misses the cache.
  `AURA_PROMPT_CACHE_TTL=1h` uses the one-hour cache.
- A status line under the input shows, for the agent on screen, how
  full its context is, its prompt cache hit rate, the session's
  estimated cost and model calls, running background jobs, and queued
  work. The Chat panel's bottom border shows todo progress and the item
  being worked on.
- `/context` breaks down what fills the context window, `/cost` lists
  the session's tokens and cost, `/todos` opens the agent's todo list,
  and `/compact [focus]` summarizes the older history now, keeping
  `focus` in particular.
- Typing `@` in the input opens a fuzzy picker of the agent's files and
  folders. Each `@path` in a sent message is attached for the model:
  up to 64 KiB of a file, or a listing of a folder. The chat and the
  record keep the message as typed.
- Ctrl+V pastes a clipboard image into the message, and pasting the
  path of an image file attaches it. Each image shows as an
  `[image N]` chip and reaches the model as an image block. Images are
  capped at 3.75 MiB each and 5 per message.
- The terminal turns on bracketed paste, so pasted text with newlines
  stays in the input instead of sending each line as a message.
- Ctrl+G opens the input in `$VISUAL` or `$EDITOR` (falling back to
  `vi`). What is saved replaces the input when the editor exits.
- The terminal checkpoints each agent's session after every turn under
  `<data_dir>/sessions/`: the conversation as the model saw it (tool
  calls included), the todo list and usage totals. A restart continues
  from the checkpoint plus whatever the record holds after it, instead
  of from prompts and replies alone. `/resume` lists earlier sessions
  and `/resume <n>` continues one.
- Tool calls that need approval pause the turn and ask in the agent's
  chat. `y` allows the call once, `a` always allows that tool (and, for
  `run_command`, that command, such as `cargo test`) in the current
  project, and `n` or Esc denies it. Saved answers live in
  `<data_dir>/approvals.json`. A prompt nobody answers is denied after
  10 minutes (`AURA_TUI_APPROVAL_TIMEOUT_SECS`), and Ctrl+C denies it and
  stops the turn. An agent that is not on screen asks in its own chat
  and the terminal says which agent is waiting.
- Permission modes: `/mode [name]` and Shift+Tab switch the agent on
  screen between `default` (asks before edits and commands),
  `accept-edits` (file edits run, commands still ask), `plan`
  (read-only) and `unrestricted` (nothing asks). The status line
  starts with the current mode. Each switch is written to the agent's
  record before the next tool call uses it, and spawned agents start in
  their parent's mode.
- The terminal rings the bell when a turn that took at least 10 seconds
  finishes (`AURA_TUI_NOTIFY_AFTER_SECS`) and when an agent needs
  approval. `AURA_TUI_DESKTOP_NOTIFY=1` also sends a desktop
  notification through `notify-send` on Linux or `osascript` on macOS,
  and `AURA_TUI_BELL=0` turns the bell off.
- Terminal agents follow project instructions. Each turn loads
  `AGENTS.md`, `CLAUDE.md`, `.cursor/rules.md` and `.cursor/rules/*.md`
  / `*.mdc` from the agent's working folder and every folder above it
  up to the home directory, plus the user's own `~/.aura/AGENTS.md`.
  The nearest file wins when two disagree. Cursor rules with
  `alwaysApply: true` are included, rules with `globs` are included
  with their globs noted, and rules with only a `description` are
  listed for the agent to read when relevant. The instructions are
  capped at 32 KiB, cutting the most general files first and saying
  so. Files are re-read only when they change, and the section sits
  right after the base system prompt, so unchanged files keep the
  prompt cache warm. `AURA_PROJECT_INSTRUCTIONS=off` turns this off.
- `/init` asks the agent, as a normal turn, to study the project and
  write an `AGENTS.md` at its root with the build and test commands,
  layout and conventions. When the project already has one, the agent
  improves it instead of starting over.
- `/context` lists the instruction files the agent loads, with their
  size and whether any were cut or left out.
- File checkpoints: before the file tools first change a file in a
  turn, the terminal saves its content (or its absence) under
  `<data_dir>/checkpoints/<agent>/`, content-addressed, with a manifest
  per turn. Files changed by shell commands are not covered.
- `/rewind` lists the session's turns; `/rewind <n> [code|chat|both]`
  puts files back as they were before turn `n`, cuts the conversation
  back to before its prompt, or both. The rewind is noted in the record
  as a `rewind` system entry.
- `/diff` shows how the files the latest turn changed differ from before
  it; `/diff session` covers the whole session.
- `ToolExecutor::with_file_change_hook` and the `FileChangeHook` trait
  tell a surface before and after each file a tool changes; tools name
  that file through `Tool::changed_path`. `aura-store-snapshot` gains
  `DirSnapshotStore`, a directory-backed snapshot store.
- `web_fetch` tool: terminal and `aura -p` agents can read a public web
  page as markdown. Scripts, styles and navigation are dropped, and
  headings, links, lists and code blocks are kept. Requests time out
  after 20 seconds, follow at most 5 redirects, read at most 2 MiB, and
  refuse binary content, non-HTTP URLs and local or private addresses
  (`AURA_WEB_FETCH_ALLOW_PRIVATE=1` allows those).
- `web_search` tool: terminal and `aura -p` agents can search the web
  through Brave Search (`BRAVE_API_KEY`), Tavily (`TAVILY_API_KEY`) or a
  SearXNG instance (`AURA_SEARXNG_URL`). `AURA_WEB_SEARCH_PROVIDER`
  picks one when several are set. Without a provider, the tool tells
  the agent and the user what to set. Plan and Ask modes allow both web
  tools; Debug mode does not. `aura -p --disallowed-tools` can turn
  either off.
- Post-edit check: after a step in which the agent edited files, the
  terminal (and `aura -p`) runs the project's check command and gives
  the agent the end of the output if it fails. The command comes from
  `AURA_CHECK_COMMAND` or from a `check:` line or a fenced `check` block
  in the project's `AGENTS.md`. Several edits in one step run the check
  once, and a failure that repeats word for word is reported in one
  line. `AURA_CHECK_TIMEOUT` bounds a run (2 minutes by default).

##### Changed

- The terminal now starts in `default` mode, so edits and commands the
  agent proposes wait for approval. Commands you type in the chat still
  run without asking. Set `AURA_TUI_MODE=unrestricted`, or pick that
  mode with `/mode`, to get the old behaviour where nothing asks.
- Ctrl+C while an approval prompt is open cancels the turn instead of
  quitting the terminal.
- `write_file`, `edit_file` and `delete_file` blocks in the chat show
  their change as a diff (with line numbers) instead of the raw
  arguments, and the block header counts added and removed lines.

- Terminal turns have no iteration cap and no 30-minute timeout.
- The terminal compacts history only from 70% context use
  (`AURA_COMPACTION_TRIGGER`). Below it, old tool results are no longer
  trimmed every step, which rewrote the cached prefix.
- Foreground `run_command` in the terminal defaults to a 2-minute
  timeout (was 5 seconds), and agent commands get no stdin, so they
  can't read keystrokes meant for the terminal UI.
- Build and check commands started by the agent loop are killed when
  the turn that started them is cancelled. After a timeout, their
  output is waited for at most 5 seconds.

### Terminal — plan mode

##### Fixed

- `/plan` leaves plan mode when the session is already in it and returns to the previous permission mode (`default` when the session started in plan mode). While plan mode is on, the status line says `/plan to leave`.

##### Added

- `/plan` (or Shift+Tab to the `plan` mode) puts the agent on screen in
  plan mode: its turns get plan-mode instructions, the kernel holds it
  to reads and Markdown notes, and it researches, asks if the request is
  unclear, and saves a plan with the new `create_plan` tool to
  `.aura/plans/<name>_<id>.plan.md` (YAML frontmatter with todos, then
  Markdown). `/build [plan path]` switches the agent back to a mode that
  edits (the one it had before `/plan`, else `default`), makes the
  plan's todos its `todo_write` list and has it implement the plan; the
  plan file's checklist follows the list as the agent works. The plan's
  open todos show above the input, and a PLAN, ASK or DEBUG badge sits
  next to the model name.
- `/mode ask` and `/mode debug` pick the kernel's read-only `ask` and
  `debug` ceilings. They are not in the Shift+Tab cycle.
- `Kernel::switch_mode` changes a live kernel's mode after appending a
  `mode_change` `System` record; submitted `mode_change` transactions
  are rejected as kernel-only. `Kernel::switch_permissions` switches
  the mode and the per-tool permission table together in one
  `mode_change` entry that names both tables (`ModeChangePayload`
  gained `from_permissions` / `to_permissions`); the terminal's mode
  switches go through it. `Policy` keeps the mode (and its snapshot
  hash) behind a lock, and `mode()` / `set_mode()` wrap the swappable
  policy config.
- `create_plan` counts as a Markdown write for the mode gate, so plan
  mode can use it; `todo_write` is a read.

### Kernel — task-runner tools and agent-host records go through the kernel

##### Changed

- `task_done`, `submit_plan` and `get_task_context` are recorded and
  gated like other tool calls (`ToolProposal` with `source`
  `task_executor`). The task runner's handler only runs once policy
  allows the call, so a denied `task_done` doesn't stop the loop or
  save notes. Calls blocked after a `task_done` rejected for having
  no writes are recorded with `source` `task_guard`.
- The agent host's spawn records and recorded agent messages, and the
  turns of agents it runs, reach the RECORD panel live: the scheduler
  hands its kernels the terminal's record observer, and the host
  notifies it after its own writes.
- TOOLCALL rows say why a tool didn't run (`cached result`,
  `blocked by steering`, `handled by task runner`, …).

##### Added

- `ToolRun` (`Executor`, `Preset`, `Local`) and
  `Kernel::process_tool_runs`, replacing
  `Kernel::process_tools_with_presets`. A `Local` handler is polled
  only after policy allows the call and is timed like an executor run.
- `AgentToolExecutor::spawn_local`.
- `write_system_entry` (returns the stored entry),
  `Scheduler::with_record_observer` and `Scheduler::record_observer`.

### Kernel — every tool call is gated and every entry is timed

##### Changed

- Tool results the agent loop already has (per-run cache hits,
  duplicate reads blocked by steering, invalid tool uses) now go
  through the kernel instead of being returned inline. Each one is
  recorded as `ToolProposal` (with `source` `cache`, `steering_block`
  or `invalid_tool_use`), `PolicyDecision` and `ToolExecution`, and a
  policy denial replaces the cached result. The tool itself still
  doesn't run again.
- `KernelConfig.tool_audit_observer` / `ToolAuditObserver::on_tool_entry`
  is now `KernelConfig.record_observer` / `RecordObserver::on_entry`,
  and is called for every entry the kernel appends (prompts, replies,
  reasoning, session start, process completion), not only tool-call
  entries. The RECORD panel no longer skips ids for reasoning entries,
  only shows the active agent's entries, and receives rows in order.

##### Added

- `ToolExecution.duration_ms`: how long the executor ran (`0` when a
  preset result stood in for it). `Reasoning` payloads carry
  `duration_ms` for the whole model call. The RECORD panel shows both
  in the TOOLRES and LLM rows.
- `PresetToolResult` and `AgentToolExecutor::spawn_preset`.

### Terminal — policy decisions stay with their tool calls

##### Changed

- A tool call's policy decision no longer adds its own `policy:` chat
  line, which split a run of agent steps into separate blocks. The
  decision is kept with the call (live and when restored from the
  record) and shown as `policy: allowed by <rule>` when the step is
  expanded. A decision for a call the chat doesn't show still gets
  a `policy: <tool> …` line.
- New `UiCommand::ToolPolicy` and `ActivityData::Tool.policy`.

### Terminal — PROCESSES tab and background work

##### Added

- The right panel has SANDBOX and PROCESSES tabs. PROCESSES lists
  what is running for your agents: agent turns, `task` spawns, agents
  started with `spawn_agent`, background commands, and the processes
  those commands started, including daemons that detached. Each row
  links to the agent that started it.
- The tab labels show counts: SANDBOX the files in the sandbox (not
  inside `.git`; past 10,000 it reads `10000+`), PROCESSES the rows
  still running.
- Opening a row shows its details, like a record entry: owner, parent
  chain, start and end times, the tool call that started it, pid, cwd
  and command or prompt. From there you can jump to the owning agent,
  open a command's log, or kill the row after confirming. Killing a
  turn or task that waits for an approval denies the call.
- The status line's background-job count comes from the same table.
- `task` takes `spawn_mode: "detached"`. The permanent agent works in
  the background, and its reply arrives as a message when it is done.
- Terminal and `aura -p` agents can use `spawn_agent`,
  `send_to_agent`, `agent_lifecycle`, `get_agent_state` and
  `list_agents`. Spawned agents run in their parent's mode, and their
  replies arrive as messages.
- Every command the tools run carries `AURA_AGENT_ID`,
  `AURA_TOOL_USE_ID` and `AURA_HARNESS_PID` in its environment.

##### Changed

- The PROJECTS and AGENTS tabs are written in capitals and drawn like
  SANDBOX and PROCESSES.

### Terminal — model, tokens and cost on each reply

##### Added

- Each AURA reply header shows, at the right edge, the model that
  wrote it, the tokens of that model call (whole prompt, cached or not,
  plus output) and its cost at API list prices. Replies restored from
  the record show the same. Local models show no cost; Claude and
  ChatGPT plan models show the API-equivalent estimate. On narrow
  chats the cost, then the tokens, are dropped first.
- `debug.llm_call` frames and `Reasoning` record entries carry
  `cache_creation_input_tokens` and `cache_read_input_tokens` when the
  provider reports them.

### Designers — Cursor terminal fonts

##### Changed

- Seeded `ui-designer` and `web-designer` tokens use Cursor's workbench
  sans for UI content and Cursor's terminal monospace for terminal
  output, error messages and code. Headings stay on the sans scale.

### Permanent agents — AURA's default roster

##### Added

- AURA's default specialists are seeded as permanent agents alongside
  `explorer` and `reviewer`: `engineer`, `rust-engineer`,
  `frontend-engineer`, `ui-designer`, `web-designer` and
  `product-manager`. Each keeps AURA's role, routing ("spawn it for")
  and guidelines; the two designers carry AURA's design tokens.

##### Changed

- Seeding adds each built-in definition once, also to an existing
  `agent_defs/`, and records it in `agent_defs/.seeded`. A deleted
  built-in is never re-added, and one whose name is already taken is
  skipped.

### Terminal — agent activity in the chat

The terminal chat shows what the agent is doing while it works: each
stretch of thinking and each tool call is a block between your prompt
and the reply, collapsed to one line and expandable.

##### Added

- Thinking streams into a block in the chat (spinner, running clock and
  its last few lines), then collapses to `Thought for 4.2s`. This
  replaces the three-line Thinker strip under the chat.
- Each tool call is a block: `▸ ✓ read_file src/main.rs · 3 lines ·
  12ms`. While it runs it shows a spinner, its elapsed time, the
  arguments as they stream in, and `retrying (n/m)`. A failed call shows
  the first line of its error.
- `Ctrl+O` expands or collapses every block (new blocks follow suit);
  clicking a block's header toggles just that one. Expanded, a tool call
  shows its full arguments and output (up to 200 lines).
- Kernel `Reasoning` records now keep the model's thinking text
  (`thinking`, `thinking_ms`), so reopening a conversation rebuilds its
  thinking and tool-call blocks from the record.

##### Changed

- Reply text that follows a tool call gets its own message below the
  tool's block.
- Tool calls no longer appear as `$ tool` and `✓ tool: …` system lines.

### Recursive agents replace the fleet

Subagents are now ordinary agents started by other agents, and they can
start agents of their own. One node-wide `AgentHost` in
`aura-engine::agents` replaces the four `aura-fleet-*` crates.

##### Changed

- `task` starts a child of a bundled kind to get a job done and returns
  its result. `general_purpose` children get `task` and `spawn_agent`
  too, so work can be split recursively, down to `AURA_SUBAGENT_MAX_DEPTH`
  levels (default 8). A task child is removed once its job is done.
- `spawn_agent` starts a long-lived named child. It keeps its
  conversation across `send_to_agent` / `delegate_task` messages until
  `agent_lifecycle` stops it. It can take an `initial_prompt`.
- A detached `task` returns at once; the child's result arrives later as
  a message. In a chat session that message starts a new turn once the
  session is idle.
- Children may only narrow what their parent has: mode, permissions,
  model and tools. A spawn that asks for more permissions is rejected.
- `AURA_FLEET_DEFAULT_MODE` is renamed `AURA_DEFAULT_MODE`
  (`AgentConfig::default_mode`). The old name is still read when the new
  one is unset.
- New `AURA_SUBAGENT_MAX_RUNNING` (default 32) caps how many spawned
  agents run at once across the node.

##### Removed

- The `aura-fleet-registry`, `aura-fleet-quota`, `aura-fleet-spawn` and
  `aura-fleet-subagent` crates, and the `fleet` layer.
- `SpawnMode::Batch` and `JoinPolicy`. Run several `task` calls in one
  turn instead.
- Orphan files under `~/.aura/state/orphans/` and the
  `aura agents inspect` / `aura agents reap` commands. Stopping an agent
  now stops its whole subtree.

### Terminal — permanent agents

The terminal's agents panel has two tabs (it has no border title; the
tab bar names it). **Projects** lists the live
agents, and **Agents** is the roster of permanent agents: reusable
definitions that any agent can spawn by name. With the panel focused,
←/→ switches tabs, and clicking a tab shows it.

##### Added

- Permanent agents are Markdown files in `<data_dir>/agent_defs/`. YAML
  frontmatter sets `name`, `description`, and the optional `model`,
  `tools` (an allow-list; default is every built-in tool except `task`)
  and `skills`. The body is the rules the agent follows. `explorer` and
  `reviewer` are seeded the first time the directory is created.
- The roster spawns the selected permanent agent (Enter) and then shows
  Projects with the new instance. It also edits it in
  `$VISUAL`/`$EDITOR` (`e`), creates one (`n` or the `+` row), and
  deletes it after confirmation (`d`). `/spawn <name>` starts an
  instance from the chat.
- The terminal's agents get the `task` tool. Its `subagent_type` is
  one of the permanent agents. The call registers an instance (for
  example `reviewer #1`) under the calling agent, runs it with the
  definition's rules, skills, tools and model, and returns its final
  reply. Instances stay under Projects, indented under their parent
  and marked with a spinner while they work. Switching to an instance
  continues its conversation under the same rules.
- `agents.json` entries gain optional `definition` and `parent` fields.
  Existing files load unchanged.

### Auditable tool calls — policy decisions in the record

Every tool call is now three record entries, so the record shows which
policy rule allowed (or denied) each call.

##### Breaking changes

- Record schema `SCHEMA_VERSION` is now `3`. A tool call is recorded
  as a bare `ToolProposal` entry, then a `PolicyDecision` entry
  (references the proposal hash; carries the decision), then, for
  approved calls only, a `ToolExecution` entry (references the
  decision hash; carries the actions and effects). Batched calls record
  every proposal/decision pair before any execution. The kernel writes
  only v3; earlier entries stay readable.
- `ProcessResult.entry` for a tool call is the last entry written (the
  execution, or the decision when the call did not run). The new
  `ProcessResult.policy_decision` points at the decision and proposal.
- `PolicyDecision` and `ToolExecution` transactions are kernel-only:
  `process_direct` / `process_dequeued` reject them.

##### Added

- `aura_core_types::PolicyDecision`: the verdict, the deciding
  `PolicyRule` (`source` + `rule`, e.g.
  `user_default.per_tool[run_command]=on`), the trace of every gate
  evaluated, any live approval (hook or user, with its remember scope),
  the agent mode, and a hash of the evaluated policy configuration.
- `KernelConfig.tool_audit_observer` (`ToolAuditObserver`): called as
  each tool-call entry is persisted.
- WebSocket `tool_policy_decision` outbound message, sent when a
  decision is recorded and before the tool runs.
- `GET /agents/:agent_id/tool_calls/:tool_use_id/audit` returns the
  proposal, decision, and execution entries for one tool call.
- Terminal UI: tool-call entries appear live in the RECORD panel, each
  decision adds a `policy: <tool> allowed by <rule>` chat line, and the
  record detail view has a "Policy decision" section.

### Simplification — enforce modes, collapse crates, sync docs

Three phases that make the runtime match what it documents: the
`AgentMode` enum is now enforced, scaffolding crates with no
production caller are gone, and the principles are restated as the
four the code actually upholds. The workspace goes from 51 to 39
member crates.

#### Phase 1 — `AgentMode` enforcement (commit `421421e`)

##### Breaking changes

- `aura_protocol::RuntimeRequest` gains `mode: Option<AgentMode>`
  (`"agent" | "plan" | "ask" | "debug"`, absent = `agent`).
  Consumers that mirror the struct (e.g. `aura-os`) must add the
  field to round-trip it.
- `POST /v1/run` with `task_run` / `dev_loop` and a resolved mode
  other than `agent` returns `400`. Setting
  `AURA_FLEET_DEFAULT_MODE` to a non-agent mode therefore rejects
  those requests unless they pass `mode: "agent"` explicitly.

##### Added

- Mode gate (`aura-agent-kernel` `policy/check/mode_gate.rs`): the
  first stage of `evaluate_delegate`. Every catalog tool maps to a
  `ToolModeClass` via an explicit table; unclassified tools are
  treated as `Network`, and file-write tools targeting `.md` /
  `.markdown` paths are classified as `WriteMarkdown` so Plan mode
  can write plans. Denials are recorded with the mode and violation in the
  reason.
- `PolicyConfig::mode` / `with_mode`, plumbed through the gateway
  session helpers, `child_runner` (child specs keep their own
  mode), and the CLI runner.
- `aura_core_modes::resolve_session_mode` + `AgentModeInputs`
  (moved from `aura-fleet-daemon`) and the `aura_config::session_mode`
  wrapper: request → `AURA_FLEET_DEFAULT_MODE` → `agent`.
- `aura --mode <agent|plan|ask|debug>` CLI flag.
- Tests: per-mode policy matrix, mode-before-tool-state ordering,
  a kernel Plan-mode denial/record test, and
  `aura-engine/tests/mode_gate_catalog.rs`, which fails if a
  catalog tool lacks a classification.

##### Removed

- `core_to_modes_mode` conversion shim.

#### Phase 2 — crate collapse (commits `a14a22d`, `5a1b294`)

##### Removed

- Crates with no production caller: `aura-fleet-daemon`,
  `aura-fleet-dispatch`, `aura-fleet-mailbox`, `aura-exec-policy`,
  `aura-exec-conflict`, `aura-surface-sdk`, plus
  `tests/sdk_smoke.rs` and `tests/daemon_shutdown.rs`.
- Re-export shells, with dependents repointed at the owning crate:
  `aura-exec-tools` / `aura-exec-runner` → `aura-tools`,
  `aura-agent-loop` → `aura-agent`, `aura-surface-automaton` →
  `aura-automaton`, `aura-surface-auth` → `aura-auth`,
  `aura-surface-terminal` → `aura-terminal`.
- The `tests/layer_boundary.rs` warn-only allowlist; the test is
  now `no_upward_layer_dependencies` and fails on any upward edge.

##### Changed

- `aura-terminal` owns the `/mode` slash-command parser
  (`SlashModeCommand`) formerly in `aura-surface-terminal`. It is
  still not wired into the TUI input loop.
- Doc comments that described removed or unused machinery now say
  so (`SessionStop` is reserved with no production writer,
  `load_enabled_plugins` has no production caller, the conflict
  config knobs have no consumer, fleet registry/quota/lease state
  is per chat session).

#### Phase 3 — principles and docs

##### Changed

- `README.md` is now a short landing page: what AURA is, the four
  principles (per-agent audit log, mediated effects, one writer per
  agent, the wire API is the contract), how to run, links to the
  reference pages, directory structure and license. The reference
  material moved to new pages: `docs/running.md`, `docs/api.md`
  (now listing every route, including workspace, secrets and
  processes), `docs/cli.md`, `docs/configuration.md` and
  `docs/development.md`.
- `docs/invariants.md`: reorganized under the four principles with
  every `§N` anchor preserved; adds §4.0 (mode gate), a definition
  of "deterministic", and corrects §5.a (AuditedLite replay ends in
  `SnapshotMissing`). Declared exceptions are frozen.
- `docs/architecture.md`: new layer map, `AgentMode` resolution
  chain, tool authorization pipeline diagram, and removal of the
  deleted crates.

### Architecture refactor — aura-runtime gateway refactor (Phases A–C)

Phases A through C of the `aura-runtime gateway refactor` plan
(`.cursor/plans/aura-runtime_gateway_refactor_09dbd8ab.plan.md`)
collapse `aura-runtime` into a thin HTTP/WS gateway + composition
root by (1) unifying the two divergent session-start request
shapes into one canonical wire-level `RuntimeRequest`, (2) lifting
the orchestration engine + subagent dispatcher out of the gateway
crate, and (3) reorganizing the remaining gateway files into a
handler-grouped layout. Each phase ended green on
`cargo fmt --check`, `cargo clippy -D warnings`, `cargo check
--workspace`, `cargo test --all --all-features`, and the
end-to-end smoke check (`POST /v1/run` → `WS /stream/:run_id`
round-trip).

#### Phase A — Wire-shape unification (commits `ce1cbaf` + `7fe0229`)

##### Breaking changes

- Collapse `SessionInit` + `AutomatonStartRequest` into a single
  canonical [`aura_protocol::RuntimeRequest`] shape. The wire
  surface moves from `WS /stream` + first-frame `session_init`
  and `POST /automaton/start` to `POST /v1/run` +
  `WS /stream/:run_id`. The legacy types and endpoints are
  deleted with no aliases. External consumers of `aura-protocol`
  (e.g. `aura-os`) must migrate; the sibling-repo migration
  landed back-to-back in `aura-os` on its own `main`.
- Rename `aura_protocol::AgentIdentityWire` to
  `aura_protocol::AgentPersona`, now nested as the `persona`
  field inside the new `AgentIdentity` wrapper.
- Remove `InboundMessage::SessionInit` variant. WS-protocol
  tests that exercised the deleted first-frame contract are
  deleted.
- Lifecycle endpoints rename for consistency: `/automaton/list`
  / `/automaton/:id/status` / `/automaton/:id/pause` /
  `/automaton/:id/stop` become `/v1/run/list` /
  `/v1/run/:id/status` / `/v1/run/:id/pause` /
  `/v1/run/:id/stop`.

#### Phase B — Engine extraction + subagent into agent layer (commit `97fcce6`)

##### Internal refactor

- Extract orchestration engine into new `aura-engine` crate
  (surface layer): `scheduler.rs`, `worker.rs`,
  `automaton/{mod,build,dispatch,event_channel}.rs`,
  `memory_observer.rs`, `capabilities.rs` (formerly
  `runtime_capabilities.rs`), `executor.rs` (formerly
  `executor_factory.rs`), `model_context.rs`, plus the
  surface-layer `RuntimeChildRunner` impl of
  [`aura_fleet_spawn::ChildRunner`].
- Split subagent dispatch across three layers: `aura-tools`
  (exec) still declares `SubagentDispatchHook`;
  `aura-agent-subagent` (agent) absorbs `SubagentRegistry` +
  bundled kinds + the pure-data adapter helpers
  (`parent_context_from_request`, `overrides_from_request`,
  `narrow_permissions`, `legacy_permissions_to_modes`,
  `core_to_modes_*`); new `aura-fleet-subagent` (fleet) owns
  the concrete `FleetSubagentDispatcher` impl. All edges
  remain downward — no new `WARN_ONLY_UPWARD_EDGES` entries.
  The `task_tool_*` integration tests move with the dispatcher
  into `crates/aura-fleet-subagent/tests/`.
- Move top-level `tool_permissions.rs` helpers
  (`load_agent_tool_context`, `validate_*`) into
  `aura-tools::permissions` (the router-layer HTTP handler
  stays in `aura-runtime`).
- `aura-surface-cli` import paths updated:
  `aura_runtime::memory_observer::MemoryTurnObserver` →
  `aura_engine::MemoryTurnObserver`. `aura_runtime::*::*`
  legacy paths preserved via thin re-export modules in
  `aura-runtime/src/lib.rs` (`scheduler`, `memory_observer`).

#### Phase C — Infrastructure extraction + gateway reorg (commit `2a4ec67`)

##### Internal refactor

- Pull HTTP `DomainApi` (`domain.rs` + `jwt_domain.rs`) into a
  new `aura-domain-http` crate (surface layer); `aura-runtime`
  no longer depends on `reqwest` for domain calls. (Note:
  `reqwest` is retained as a direct dep of `aura-runtime`
  because `gateway/session/cross_agent_hook.rs` still issues
  outbound HTTP for the cross-agent callback path.)
- Move terminal PTY WebSocket handler from
  `aura-runtime/src/terminal.rs` to `aura-terminal/src/ws.rs`;
  gateway keeps a one-line `terminal_ws_handler` in
  `gateway/middleware.rs` that delegates to
  `aura_terminal::ws::handle_terminal_ws`. `portable-pty`
  drops out of the `aura-runtime` deps.
- Reorganize `aura-runtime/src/router/` into
  `aura-runtime/src/gateway/`:
  `mod.rs` (module declarations + `pub use`),
  `middleware.rs` (`create_router` + middleware stack +
  health + terminal-WS delegate), `state.rs`
  (`RouterState` + `RouterStateConfig`), `handlers/*`
  (per-endpoint bundles: `run.rs`, `run_ws.rs`, `files.rs`,
  `tx.rs`, `memory/`, `skills.rs`, `tool_permissions.rs`,
  `util.rs`), `session/*` (per-WebSocket-connection protocol
  layer: `chat.rs`, `cross_agent_hook.rs`, `generation.rs`,
  `helpers.rs`, `partial_json.rs`, `state.rs`, `tool_approval.rs`).
- Workspace `Cargo.toml`: add `aura-engine`, `aura-domain-http`,
  `aura-fleet-subagent` to `[workspace].members` with matching
  `KNOWN_CRATES` entries and `//! Layer:` doc-tags. Trim
  `aura-runtime/Cargo.toml` deps that moved out (`aura-fleet-*`
  for spawn/registry/quota, `aura-memory`, `aura-automaton`,
  `aura-skills`, `aura-agent-subagent` reached only via the
  engine, `aura-prompts`, `aura-context-skills`,
  `aura-reasoner`, `aura-kernel`, `portable-pty`, `which`);
  add the new surface deps (`aura-engine`, `aura-domain-http`,
  `aura-terminal`, `aura-fleet-subagent`).

### Architecture refactor — Phase 10 residual cleanup

`refactor(arch-phase-10)` consolidates the five carve-outs the
prior subagents flagged but couldn't cleanly fit into the
tightened scopes of Phases 7b, 8, and 9. Each carve-out is paired
with a fresh test pin, and the closed-taxonomy schema bump is
done in one combined increment so the on-disk audit log never
sees an intermediate half-migrated shape.

- **Carve-out 1 — Binary body migration.** The full body of
  `src/main.rs` (the `aura` binary) and
  `crates/aura-runtime/src/main.rs` (the `aura-node` binary) is
  lifted out of the binary crates. The new entry points
  [`aura_surface_cli::run`] and [`aura_runtime::run_node`]
  (re-exported as [`aura_surface_cli::run_node`] to keep the
  documented surface path callable) own every line of useful
  logic. The two `main.rs` files are reduced to ≤ 10 lines
  apiece — a `tokio::main`, a `dotenvy::dotenv` call, optional
  tracing-subscriber init, and the delegated `run().await` /
  `run_node().await`. The `tests/cli_golden.rs` pins remain
  byte-identical across the rename.
- **Carve-out 2 — `RecordKind::SessionStop`.** A new variant
  emitted by [`FleetDaemon::run`] at session-end via
  [`aura_agent_kernel::write_system_record`]. Carries
  `session_id`, `agent_id`, `total_iterations`,
  `total_input_tokens`, `total_output_tokens`, `duration_ms`,
  and `clean_shutdown: bool`. Sessions registered through the
  new `FleetDaemon::record_session` API are emitted as one row
  each on shutdown.
- **Carve-out 3 — Spawn-record on-disk wire format.**
  `SubagentSpawn` audit rows now use the typed
  `TransactionType::SubagentSpawn` variant instead of the
  Phase 7a `TransactionType::System` + JSON discriminator
  workaround. The payload retains the same `OverrideManifest`
  shape but no longer carries the `kind: "subagent_spawn"`
  field. The `SubagentResult` Phase 7a guarantee (byte-identical
  to callers who don't override anything) is preserved — only
  the on-disk audit row changes shape.
- **Carve-out 4 — `daemon.run()` external shutdown token.**
  [`FleetDaemon::run`] reshapes to
  `pub async fn run(&self, shutdown: CancellationToken) -> Result<(), FleetDaemonError>`.
  The mailbox loop selects on `shutdown.cancelled()` alongside
  the receiver; on shutdown it cancels every in-flight child
  through the daemon-owned `fleet_shutdown` token, polls
  [`FleetRegistry::running_count`] for cooperative drain inside
  the configurable grace window (default 30s,
  [`DaemonConfig::shutdown_grace`]), hands off
  `SpawnMode::Detached` survivors to the [`OrphanStore`], and
  emits one `RecordKind::SessionStop { clean_shutdown }` per
  tracked session.
- **Carve-out 5a — `PreToolUse` mid-flight cancellation.**
  `PreToolUse` hooks now fire BEFORE tool dispatch inside the
  kernel (`crates/aura-agent-kernel/src/kernel/tools/single.rs`
  + `batch.rs`). `HookOutcome::Block` aborts the dispatch and
  produces a synthetic `Effect::failed` whose payload embeds the
  documented `{"kind": "tool_call_blocked_by_hook", "tool_name":
  ..., "tool_use_id": ..., "reason": ...}` JSON discriminator so
  the audit log captures the block without consuming a separate
  sequence number (the single tool-proposal `RecordEntry` is the
  one source of truth). The agent loop sees a clean rejection
  through the synthetic effect.
- **Carve-out 5b — `PermissionRequest` kernel-side wiring.**
  [`KernelConfig`] gains a `pub plugin_hooks:
  Option<Arc<PluginHookHost>>` field (defaulting to `None` for
  backward compat). When set, `Kernel::resolve_prompt_verdict`
  fires `PermissionRequest` hooks BEFORE the interactive
  `ToolApprovalPrompter` runs. `HookOutcome::Approve`
  short-circuits to `PolicyVerdict::Allow`; `HookOutcome::Deny`
  short-circuits to `PolicyVerdict::Deny`; any other outcome
  falls through to the interactive prompt.
- **Carve-out 5c — `ConnectorRegistry` last-wins API.**
  [`ConnectorRegistry`] gains `replace` and `remove` methods.
  The Phase 8 plugin materialiser
  (`aura-plugin-core::runtime::load_plugin_contributions`) now
  calls `replace` instead of `register` when a plugin-supplied
  connector conflicts with an existing one, logging the
  displacement with `tracing::info!`.

#### Schema-version bump — `aura-store-record::SCHEMA_VERSION`

The closed-taxonomy enum additions are combined into a single
schema-version increment: `SCHEMA_VERSION` advances from `1`
(implicit in Phases 1 through 9 — the constant did not yet
exist) to `2`. The new variants are
`RecordKind::SessionStop`,
`RecordKind::ToolCallBlockedByHook`, and
`TransactionType::SubagentSpawn`. Pre-bump audit rows continue
to deserialize: serde-default arms for `RecordKind` and
`TransactionType` fall back to the Phase 2 forward-compat
`Unknown` variants, so an older `aura-node` reading a
schema-v2 log degrades to "I don't know that variant, skip it"
rather than panicking. The kernel WRITES only the new format
going forward. Operators with mixed-version fleets should
upgrade readers before writers; otherwise pre-bump readers
will treat all SessionStop / ToolCallBlockedByHook /
SubagentSpawn rows as `Unknown`-class — recoverable but not
queryable. There is no destructive migration step.

### Architecture refactor — Phases 1 through 9

The architecture refactor lands the 10-layer crate stack (`core` →
`store` → `config` → `model` → `context` → `plugin` → `exec` →
`agent` → `fleet` → `surface`) with strict downward-only
dependencies enforced by `tests/layer_boundary.rs`. Every phase
left the workspace `cargo fmt` / `cargo clippy -D warnings` /
`cargo test --workspace` green.

- **Phase 1** — `refactor(arch-phase-1)` (`1851ad6`): split
  `aura-core` into `aura-core-types`, `aura-core-protocol`,
  `aura-core-permissions`, `aura-core-modes`, and `aura-core-auth`;
  introduce the closed `AgentMode` / `KernelMode` / `SpawnMode` /
  `JoinPolicy` / `ReplayMode` / `SandboxMode` enums; add
  advisory `tests/layer_boundary.rs`.
- **Phase 2** — `refactor(arch-phase-2)` (`7307b01`): split the
  store layer into `aura-store-db`, `aura-store-record`,
  `aura-store-snapshot` (the snapshot crate ships as a V1 no-op
  stub); preserve `aura-store` as a compatibility shell.
- **Phase 3** — `refactor(arch-phase-3)` (`6d2f69d`): rename the
  model + context crates into the layered `aura-model-reasoner`,
  `aura-context-prompts`, `aura-context-memory`,
  `aura-context-compaction`, `aura-context-skills`; add the
  `aura-agent-steering` placeholder.
- **Phase 4a** — `refactor(arch-phase-4a)` (`e15ff72`): extend
  `aura-config` with the fleet / subagent / conflict / plugins
  TOML tables, the `AURA_HOME` resolution helper, and the
  `aura migrate` stub.
- **Phase 4b** — `refactor(arch-phase-4b)` (`e8108ce`): plugin
  schema + install + cache + marketplace + `aura plugins` CLI;
  introduces `aura-plugin-api` (in-process contributor traits)
  and `aura-plugin-core` (declarative manifest pipeline).
- **Phase 4c** — `refactor(arch-phase-4c)` (`89201f6`): plugin
  runtime surfaces — `aura-plugin-hooks` (hook engine + 10
  Codex/Claude lifecycle events), `aura-plugin-mcp` (stdio
  JSON-RPC client), `aura-plugin-connectors`, plus the trust
  prompt flow.
- **Phase 5** — `refactor(arch-phase-5)` (`70f18ee`): exec layer
  split — `aura-exec-conflict`, `aura-exec-isolation`,
  `aura-exec-policy`, `aura-exec-sandbox`, `aura-exec-tools`,
  `aura-exec-runner`; introduce `ConflictRegistry` and
  `WorktreeIsolation`.
- **Phase 6a** — `refactor(arch-phase-6a)` (`a316784`) +
  follow-up `refactor(arch-phase-6a-audit)` (`5a220dc`): agent
  layer split — `aura-agent-kernel`, `aura-agent-loop`,
  `aura-agent-subagent`, `aura-agent-steering`; introduces the
  `KernelMode::{Audited, AuditedLite}` tiering.
- **Phase 6b** — `refactor(arch-phase-6b)` (`48356ba`): replay
  wiring — `ReplayConsumer`, `ReplayError`, `RecordLog::scan`,
  `KernelConfig::replay_from`.
- **Phase 6c** — `refactor(arch-phase-6c)` (`8aa9441`):
  context-memory inversion — `TurnSummary` + `ModelProvider`
  injection breaks the four remaining `aura-context-memory ->
  aura-agent` upward edges; `MemoryTurnObserver` relocated to
  `aura-runtime`; first warn-only edge promoted to fail-on-detect.
- **Phase 7a** — `refactor(arch-phase-7a)` (`682e888`): fleet
  daemon composition root — `aura-fleet-registry`,
  `aura-fleet-spawn`, `aura-fleet-dispatch`, `aura-fleet-quota`,
  `aura-fleet-daemon`; per-parent audit-append lease replaces
  the old single `spawn_lock`; task tool routed through
  `aura-agent-subagent::derive_subagent ->
  aura-fleet-spawn::spawn`.
- **Phase 7b** — `refactor(arch-phase-7b)` (`5f90526`): full
  `SpawnMode::{Detached, Batch}` + `aura-fleet-mailbox` +
  `JoinPolicy` enforcement + quota enforcement + orphan handoff
  on parent death + the complete `SubagentOverrides` /
  `OverrideManifest` validation surface.
- **Phase 8** — `refactor(arch-phase-8)` (`dcfd0c9`): plugin
  runtime integration end-to-end — skills wired into
  `aura-context-skills`, MCP merge into `aura-plugin-mcp`, hooks
  into `aura-plugin-hooks`, connectors into
  `aura-plugin-connectors`; hook events fired at every documented
  agent-loop / fleet-spawn lifecycle point; hook sandbox env
  scrubbing; backward compat verified (empty `~/.aura/plugins/`
  yields zero behavioural diff).
- **Phase 9** — `refactor(arch-phase-9)`: surface layer + SDK +
  strict layer boundary enforcement. See the per-section list
  below.

### Added — Phase 9

- New `aura-surface-cli` crate at the `surface` layer hosting the
  CLI composition-root types (`ModeFlag`, `parse_mode_str`,
  `CliModeInputs`, `version_banner`).
- New `aura-surface-sdk` crate exposing the external
  `AuraClient` / `AuraSession` shape over `aura-core-protocol`,
  with the documented `SessionConfig::mode: Option<AgentMode>`
  field that feeds the AgentMode resolution priority chain.
- New `aura-surface-terminal` relocation shell over the legacy
  `aura-terminal` crate. Adds the typed `SlashModeCommand` for
  parsing `/mode <agent|plan|ask|debug>` from the TUI input
  layer.
- New `aura-surface-automaton` relocation shell over the legacy
  `aura-automaton` crate.
- New `aura-surface-auth` relocation shell over the legacy
  `aura-auth` crate (`ZosClient`, `CredentialStore`). The token
  primitive types (`AccessToken`, `RefreshToken`, `Token`,
  `StoredSession`, `AuthError`) stay at the `core` layer in
  `aura-core-auth` and are re-exported through the surface
  shell.
- `FleetConfig::default_mode: AgentMode` field — the daemon
  default rung of the documented `AgentMode` resolution priority
  chain. Overridable via the new `AURA_FLEET_DEFAULT_MODE`
  environment variable.
- `aura_fleet_daemon::resolve_session_mode` helper +
  `AgentModeInputs` struct implementing the documented priority:
  CLI flag > TUI slash > SDK field > daemon default >
  `AgentMode::Agent` fallback. Children inherit the resolved
  mode through the existing `derive_subagent` chain (narrowing
  only).
- Top-level `aura --mode <agent|plan|ask|debug>` global CLI flag
  (clap-derived `ModeFlag` from `aura-surface-cli`).
- `tests/layer_boundary.rs` strict-mode promotion. The
  `WARN_ONLY_UPWARD_EDGES` allowlist now carries a single
  explicitly-documented Phase 10 follow-up entry
  (`aura-tools -> aura-kernel`); every other upward edge fails
  CI. Phase 9 also introduces the
  `every_crate_carries_a_matching_layer_doc_tag` test that
  parses each `crates/<name>/src/lib.rs` for the
  `//! Layer: <layer>` doc-comment and asserts consistency with
  the `KNOWN_CRATES` table.
- `crates/aura-fleet-daemon/tests/mode_resolution_priority.rs`
  with an `insta` snapshot pinning the priority order across
  ten input combinations.
- New `machete` job in `.github/workflows/ci.yml` running
  `cargo machete` against the workspace.

### Changed — Phase 9

- `aura-agent` no longer depends on `aura-auth`.
  `default_agent_config(model)` now delegates to the new
  `default_agent_config_with_auth(model, auth_token)` helper;
  surface-layer callers (`src/session_helpers.rs`) compose the
  env-var lookup (`load_auth_token`) with the credential-store
  lookup (`aura_auth::CredentialStore::load_token`) before
  invoking it.
- `aura-auth` reclassified from `core` to `surface` in
  `KNOWN_CRATES` since `ZosClient` (HTTP) and `CredentialStore`
  (OS keyring) are surface-layer concerns per the plan's
  cross-cutting "Secrets" ownership bullet.
- `deny.toml` finalised: explicit MIT / Apache-2.0 / BSD / ISC
  / MPL / Unicode allow-list; 30-day advisory window enforced
  via the `[advisories]` block (paired with the documented
  `audit` job soft-fail in `.github/workflows/ci.yml`);
  crates.io-only sources. The `openssl-sys` ban is documented
  as a TODO (the workspace's `reqwest 0.11` still pulls in
  native-tls today; flipping to `rustls-tls` is tracked as a
  Phase 10 follow-up). Duplicate-major detection for `tokio` /
  `serde` runs via the existing `multiple-versions = "warn"` +
  `highlight = "all"` toggle.
- `aura --help` now lists the top-level `--mode` global flag.
- The `audit` CI job retains `continue-on-error: true` (the
  hard-fail gate is `cargo deny check`'s 30-day advisory window
  in `deny.toml`); the YAML now carries an explanatory comment.
- `docs/architecture.md` carries a Phase 9 layer overview +
  refreshed per-layer crate inventory.

### Deprecated — Phase 9

- `aura_agent::session_bootstrap::load_auth_token` no longer
  reads the OS keyring; surface-layer callers chain
  `aura_auth::CredentialStore::load_token` explicitly. The
  agent-layer function remains for the env-var path.

### Notes — Phase 9 follow-ups

- `WARN_ONLY_UPWARD_EDGES` retains a single
  `aura-tools -> aura-kernel` entry. The clean fix is to
  relocate `ExecuteContext` / `Executor` / `ExecutorError` /
  `SpawnHook` out of `aura-agent-kernel` to the exec layer.
  Tracked as a Phase 10 follow-up; documented in the test
  source.
- The full body migration of `src/main.rs` (the `aura` binary)
  and `crates/aura-runtime/src/main.rs` (the `aura-node`
  binary) into `aura-surface-cli::run` is incremental. Phase 9
  ships the surface-layer type and dependency topology;
  Phase 10 lifts the binary bodies.
- The `openssl-sys` ban in `deny.toml` is documented as a TODO
  pending the workspace migration to `reqwest`'s `rustls-tls`
  feature.
