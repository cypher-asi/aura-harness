# Changelog

All notable changes to the Aura agent-first architecture refactor
are tracked here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the
project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Cost gate (`aura-agent-gate`)

##### Added

- `gate/<model>` model ids (for example `gate/claude/claude-opus-5-5`,
  "Gate · …" in `/model`). The frontier model coordinates and hands
  blocks of work to `run_blocks`, which runs each block on the cheapest
  tier likely to pass (Haiku, Sonnet, Opus for Claude), verifies it with
  visible and hidden checks, fixes once in place, then escalates from
  the pre-block files. Works in the terminal, `aura -p` and
  `aura eval run`. See [Gate models](docs/cli.md#gate-models).
- Gate turns start with a draft: the request goes unchanged to the fast
  tier, and the coordinator reviews the draft's diff, answering or fixing
  it with blocks. Block results carry the diff of what they changed.
- A block that fails only the coordinator's acceptance checks goes back
  to the coordinator instead of being retried on the frontier tier.
- Every attempt is appended to `AURA_HOME/gate/ledger.jsonl`.
  `aura gate report`, `fit`, `bench` and `promote` turn it into a
  versioned policy and make a policy active only when a paired benchmark
  shows lower cost without a significant pass-rate loss, overall and per
  category. Settings: `AURA_GATE_POLICY`, `AURA_GATE_EXPLORE`,
  `AURA_GATE_EXPLORE_BUDGET_USD`, `AURA_GATE_TIERS`, `AURA_GATE_LEDGER`.
- `evals/gate_cost`: 20 tasks with hidden checks, categories and
  reference solutions. Eval tasks can set `category`, `tags` and a
  `checks/` folder copied in after the run; `aura eval run` reports each
  run's cost (gate workers included) and takes `--budget-usd`.
  `evals/gate_hard` adds eight tasks with traps a careless fix falls into.

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

### Surface — native desktop app (`aura-desktop`)

##### Added

- `aura-desktop`, the terminal UI in a native window built on iced and
  drawn on the GPU (wgpu, with a tiny-skia CPU fallback). It runs the
  same in-process session as the terminal (`session::start`) and the
  same `aura_terminal::App`, rendered through a ratatui backend into a
  cell grid, so the screen, keys, mouse selection and copy behave as
  they do in a terminal.
- The grid is painted in Geist Mono (bundled) with one text draw per
  styled run; box drawing, block elements, braille and the status
  symbols are drawn as shapes, so borders join and glyphs missing from
  the font still render. The painted grid is cached until a cell
  changes, and the cursor blinks on its own layer.
- Window shortcuts: Ctrl+Shift+C copies, Ctrl+V / Ctrl+Shift+V /
  Shift+Insert paste (images attach, as in the terminal), Ctrl+= /
  Ctrl+- / Ctrl+0 zoom (Cmd on macOS). Turn alerts use the terminal's
  settings, with the window asking for attention in place of the bell.
  `--theme` picks the terminal theme; `AURA_DESKTOP_MONO_FONT`,
  `AURA_DESKTOP_FONT_FILES` and `AURA_DESKTOP_FONT_SIZE` change the
  font. `--demo` runs a scripted session and `AURA_DESKTOP_PERF` logs
  frame timings.
- Offscreen snapshot tests render whole screens with the CPU renderer;
  `AURA_DESKTOP_SNAPSHOTS=<dir>` saves them as PNGs. An ignored
  `replay` test reports boot, streaming, keystroke and repaint timings.
- `aura_terminal::render`, `App::paste_from_clipboard` and
  `alerts::desktop_notify` are public so other frontends can draw the UI
  and share its clipboard and notifications.

##### Changed

- The CI MSRV check skips `aura-desktop`, which needs Rust 1.88.

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
