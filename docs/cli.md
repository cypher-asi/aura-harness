# CLI reference

## Binaries

| Binary | Entry point | Purpose |
|--------|-------------|---------|
| `aura` | [`src/main.rs`](../src/main.rs) | The main CLI. Opens the terminal UI by default, runs one prompt and exits with `aura -p`, runs headless with `aura run --ui none`, and hosts the account, plugin and subagent commands below. The CLI body lives in [`aura-surface-cli`](../crates/aura-surface-cli). |
| `aura-node` | [`crates/aura-runtime/src/main.rs`](../crates/aura-runtime/src/main.rs) | Standalone headless server exposing the [HTTP / WebSocket API](api.md). |

Older specs mention a separate `aura-cli` crate. It never shipped; read `aura-cli/src/...` paths as `src/...` in the root crate.

## Commands

Defined in [`crates/aura-surface-cli/src/cli.rs`](../crates/aura-surface-cli/src/cli.rs).

| Command | Description |
|---------|-------------|
| `aura run` (default) | Run the agent. Flags below. |
| `aura -p "prompt"` | Run one prompt without the terminal UI, print the result and exit. See [`aura -p`](#aura--p-one-prompt-without-the-terminal). |
| `aura login [zos\|claude\|chatgpt] [--paste]` | `zos` (default) signs in with email and password and unlocks the aura-router models. `claude` and `chatgpt` link your own Claude Pro/Max or ChatGPT plan through the browser; `--paste` uses a code you paste instead of a local redirect (SSH, headless). `/login [provider] [paste]` in the TUI does the same. |
| `aura logout [zos\|claude\|chatgpt] [--all]` | Sign out of zOS (linked plans are kept for when you sign back in), unlink one plan, or with `--all` unlink every plan and sign out. `/logout [provider]` in the TUI. |
| `aura whoami` | Show the zOS sign-in and each plan link's status. |
| `aura auth move --to-zos` | Attach plans linked while signed out to the signed-in zOS account. |
| `aura hello` | Print `Hello, World!` and exit (smoke test). |
| `aura migrate [--dry-run]` | Migrate aura state (a no-op stub today). |
| `aura plugins install <dir> [--trust]` | Install a plugin from a local directory into `AURA_HOME/plugins/`. |
| `aura plugins list` | List installed plugins and their active versions. |
| `aura plugins enable <id> [--yes\|--no]` / `disable <id>` | Toggle a plugin in `AURA_HOME/config.toml`, answering the trust prompt if required. |

## Sign-in and model routes

Each model id names the route that serves it, and the user picks it in `/model`:

| Model id | Route | Needs |
|----------|-------|-------|
| `local/<name>` | Local Ollama server | nothing |
| `aura-*` and other aliases | aura-router, billed to the zOS account | `aura login` |
| `claude/<model>` | `api.anthropic.com`, billed to your Claude plan | `aura login claude` |
| `chatgpt/<model>` | OpenAI Responses API, billed to your ChatGPT plan | `aura login chatgpt` |

Routes never fall back to one another, so who pays never changes silently. If a model loses its sign-in (logout, unlink, revoked token), the terminal switches to the local default model.

Plan links belong to the identity that made them: this machine's local profile while signed out, otherwise the zOS user. Tokens are stored in the OS keyring, or in `~/.aura/auth/secrets/` (mode 0600) when no keyring is available. Link metadata is in `~/.aura/auth/index.json`.

The Claude link uses the OAuth client that Claude Code uses. Anthropic does not publish it for other apps, and its terms may not allow plan use outside Claude Code. `aura login claude` prints this warning before you sign in.

## Long tasks in the terminal

- **No caps.** A turn keeps going until the agent finishes, you press Ctrl+C, or a limit from [Terminal long runs](configuration.md#terminal-long-runs) stops it.
- **Todo list.** For multi-step work the agent keeps a plan with the `todo_write` tool. If it stops with items still open, it is sent back to the next one. It ends anyway when it asks you something, says it is blocked, or twice in a row leaves the list unchanged. The list survives history summaries and restarts.
- **Talking to a busy agent.** A message you send while the agent works goes into its running turn, and the agent reads it before its next step. A message that arrives as the turn ends runs as the next turn. Shell commands typed in the chat still wait for the agent's current work.
- **Background commands.** `run_command` with `background: true` starts a job and returns at once. The agent checks it with `job_output` and stops it with `job_kill`. When a job exits, the agent that started it is told: inside its running turn, or in a new turn if it is idle. Quitting the terminal stops every job.
- **Foreground commands** time out after 2 minutes unless the call sets `timeout_ms`, up to 10 minutes.
- **Status line.** The row under the input shows the agent on screen: context use (`ctx 62% ▰▰▰▰▱ 124k/200k`, yellow from 75%, red from 90%), prompt cache hits, estimated cost at API list prices, model calls, running background jobs (`2 bg`), queued work, and how often the history was summarized. Parts that don't fit are dropped from the right. The Chat panel's bottom border shows todo progress, e.g. `[3/7] ▸ Write the parser`.
- **`/context`** shows how full the context window is and what fills it (system prompt, tools, skills, conversation) as of the last turn.
- **`/cost`** lists the session's model calls, tokens (new input, cache reads and writes, output) and estimated cost.
- **`/todos`** opens or closes the agent's full todo list above the input.
- **Sessions survive restarts.** After every turn the terminal saves the agent's session (the conversation as the model saw it, tool calls included, plus its todo list and usage totals) under `<data_dir>/sessions/<agent>/`. On restart the agent continues from there, with anything the record holds after the checkpoint replayed on top. Without a usable checkpoint it falls back to the prompts and replies in the record. `/new` starts a fresh session; the old one stays saved.
- **`/resume`** lists the agent's earlier sessions, newest first; **`/resume <n>`** continues number `n` (the agent must be idle). The chat shows the resumed conversation and the status line its usage.
- **Web tools.** `web_fetch` reads a public web page as markdown (scripts, styles and navigation dropped; headings, links, lists and code kept). It refuses binary files, non-HTTP URLs, and localhost or private-network addresses unless `AURA_WEB_FETCH_ALLOW_PRIVATE=1`. Long pages come back in parts the agent can page through. `web_search` returns titles, URLs and snippets from the provider set in [Terminal long runs](configuration.md#terminal-long-runs); without one, it says what to set. Plan and Ask modes allow both tools.
- **Post-edit check.** If the project defines a check command, the terminal runs it after each step in which the agent edited files and gives the agent the last 80 lines of output when it fails. The chat shows a warning for a failure and a note when the check passes again. Several edits in one step run the check once, and a failure that repeats word for word is reported in one line. Set the command with `AURA_CHECK_COMMAND`, or put it in the project's `AGENTS.md`, either as a line or as a fenced block whose lines run in order and stop at the first failure:

  ````markdown
  check: cargo check --workspace

  ```check
  cargo fmt --check
  cargo clippy -- -D warnings
  ```
  ````

  The first `check:` line or `check` block wins; `check:` lines inside other code blocks are ignored. The command runs in the agent's project folder through the shell, with `AURA_CHECK_TIMEOUT` as its time limit. A check from `AGENTS.md` runs without asking, so read the `AGENTS.md` of a project you don't trust, or set `AURA_CHECK_COMMAND=off`.
- **`/compact [focus]`** summarizes the older history now instead of waiting for context pressure, keeping the first message and the recent steps as they are. Text after the command tells the summary what to keep. A busy agent does it when its current work ends. The summary replaces history only in the running session; the record keeps every message.
- **File checkpoints.** Before the agent's file tools (`write_file`, `edit_file`, `delete_file`) first change a file in a turn, the terminal saves what the file held, or that it did not exist. Each content is stored once, by hash, under `<data_dir>/checkpoints/<agent>/blobs/`, and each turn has a manifest under `<data_dir>/checkpoints/<agent>/turns/` with its time, prompt and the files it changed. Edits by a subagent count toward the turn that started it. Files changed by shell commands (`run_command` or a command you type) are not seen, so they are not checkpointed and cannot be rewound.
- **`/rewind`** lists the turns of the current session, newest first, with their time, prompt and how many files each changed. **`/rewind <n> [code|chat|both]`** undoes turn `n` and every turn after it (the agent must be idle). `code` puts back each file those turns changed as it was before turn `n`, and removes files that did not exist then. `chat` cuts the conversation the model sees back to just before turn `n`'s prompt and saves the session. `both` is the default. If turn `n` was summarized away by a history summary, only `code` is possible. The rewind is noted in the agent's record, and the record itself keeps every message.
- **Diffs.** A `write_file`, `edit_file` or `delete_file` block in the chat shows the change as a diff with line numbers when you expand it, and its header counts the added and removed lines (`Edited main.rs · +3 -1`). `edit_file` blocks restored from history show the replaced text against its replacement. **`/diff`** shows how the files the latest turn changed differ now from before that turn; **`/diff session`** does the same for every file changed since the session started.
- **Approvals.** When a tool call needs your approval, the turn pauses and the agent's chat shows what it wants to run. Press `y` to allow it once, `a` to always allow it in this project, or `n` (or Esc) to deny it. "Always" is saved per project folder in `<data_dir>/approvals.json`: for `run_command` it covers the program and its subcommand (`cargo test`, not every `cargo` command) and never matches a shell script that chains or redirects commands; for other tools it covers the tool. Commands you type in the chat are never asked about. A prompt left unanswered is denied after 10 minutes, and Ctrl+C denies it and stops the turn. An agent that is not on screen asks in its own chat, and the terminal tells you which agent is waiting.
- **Modes.** `/mode` lists the modes and `/mode <name>` picks one for the agent on screen; Shift+Tab steps to the next one. `default` asks before file edits and commands, `accept-edits` lets file edits run but still asks before commands, `plan` is read-only, and `unrestricted` never asks. Reading files and searching never ask. The status line starts with the current mode. Each switch is written to the agent's record before the next tool call uses it, and agents it spawns start in the same mode. The starting mode comes from `--mode`, then `AURA_TUI_MODE`, then `AURA_DEFAULT_MODE`; `agent` means `default`.
- **Notifications.** The terminal rings the bell when a turn that took at least 10 seconds finishes and when an agent needs approval. Desktop notifications are off unless you set `AURA_TUI_DESKTOP_NOTIFY=1`. See [Terminal long runs](configuration.md#terminal-long-runs) for the settings.

## Project instructions

Terminal agents follow the instructions a project keeps for coding agents. Before each turn the terminal looks for these files in the agent's working folder (its project's folder, or the folder the terminal was started in) and in every folder above it, stopping below your home directory:

- `AGENTS.md` (or `agents.md`)
- `CLAUDE.md`
- `.cursor/rules.md` and the `.md` / `.mdc` files in `.cursor/rules/`

Your own instructions for every project go in `~/.aura/AGENTS.md` (`$AURA_HOME/AGENTS.md` when `AURA_HOME` is set).

- **Precedence.** The files go into the system prompt from the most general (yours, then the folders furthest up) to the most specific (the working folder). The prompt tells the agent that the later file wins when two disagree, and that what you say in the chat beats them all. A `CLAUDE.md` that is a symlink to `AGENTS.md` is read once.
- **Cursor rules.** A rule with `alwaysApply: true`, or with no frontmatter, is always included. A rule with `globs` is included with its globs, and the agent applies it only to matching files. A rule with only a `description` is listed by path and description for the agent to read when it is relevant. A rule with none of these is meant to be named by hand and is skipped.
- **Size limit.** Instructions are capped at 32 KiB in total. The nearest files are kept first; a file that no longer fits is cut at a line end with a note saying so, or left out and listed by path when too little room is left.
- **Changes.** Files are re-read when their size or modification time changes, so an edit applies from the next turn. Unchanged files produce the same bytes every turn, right after the base system prompt, so they stay in the prompt cache.
- **`/context`** lists the files the agent loads, with their size and whether each was cut, left out, or only listed.
- **`/init`** asks the agent, as a normal turn, to study the repository and write an `AGENTS.md` at the project root: what the project is, build, run, lint and test commands, the layout, and conventions and pitfalls. If the project already has an `AGENTS.md`, the agent improves it instead of starting over. A busy agent starts it when its current work ends.

Set `AURA_PROJECT_INSTRUCTIONS=off` to stop loading these files.

## Input in the terminal

- **`@` mentions.** Typing `@` opens a file picker over the agent's workspace, ranked by name and path prefix, then substring, then letters in order. `↑`/`↓` move, Enter inserts `@path`, Tab completes (on a folder it stays open to go into it), and Esc closes it for that word. The picker skips `target/`, `.git/`, `node_modules/` and plain names or `*.ext` patterns from the workspace's root `.gitignore`, and it leaves out paths that contain spaces. When you send the message, each `@path` that names a file or folder inside the workspace is attached: the model gets up to 64 KiB of each file (with a note when it is cut), at most 256 KiB per message, and a listing of up to 200 entries for a folder. Binary files are described, not shown. The chat and the record keep the message as you typed it. Mentions that don't resolve stay plain text.
- **Images.** Ctrl+V pastes an image from the clipboard, or the clipboard's text when it holds no image. Pasting the path of a PNG, JPEG, GIF or WebP file (as terminals do when you drop a file on them) attaches that file. Each image puts an `[image N]` chip in the input; deleting the chip drops the image. The model gets the images as image blocks before the message text, and the chat and the record show the chips. An image may be at most 3.75 MiB (5 MiB once encoded), and a message holds at most 5. A running turn only takes text, so a message with images sent to a busy agent waits for its current work to end. Reading the clipboard uses `wl-paste` or `xclip` on Linux, `osascript` or `pbpaste` on macOS, and PowerShell on Windows, and gives up after 2 seconds.
- **Ctrl+G** opens the input in your editor (`$VISUAL`, then `$EDITOR`, then `vi`; see [Terminal editor](configuration.md#terminal-editor)). When the editor exits, what you saved replaces the input, newlines included, ready to edit further or send with Enter. If the editor fails, the input stays as it was.
- **Pasting.** Pasted text arrives in one piece when the terminal supports bracketed paste: newlines stay in the input (shown as `↵`) instead of sending the message line by line.

## `aura -p`: one prompt without the terminal

`aura -p "prompt"` (or `--print`) runs one prompt to the end, prints the result, and exits. It is meant for scripts, CI and evals.

```sh
aura -p "fix the failing test in src/parser.rs"
git diff | aura -p "review this change" --output-format json
aura -p - < task.md --max-time 30m --max-cost-usd 5
aura -p "now add docs for it" --continue
```

- **Prompt.** The text after `-p`. `-p -`, or `-p` with no text, reads the prompt from stdin. When stdin is piped and a prompt is given too, stdin is appended to the prompt after a blank line. Scripts that pipe nothing should give stdin `/dev/null` so the run does not wait for input.
- **Same agent machinery as the terminal.** The run uses the terminal's session code: long-run settings (no iteration cap, the todo list keeps the agent going), background jobs, history compaction, session checkpoints, the web tools, the post-edit check, project instructions, and the permanent agents for `task`. It works in the current directory (or `--cwd`) and keeps its data in `AURA_DATA_DIR`, else `./aura_data`, like the terminal.
- **Sessions.** Runs belong to a dedicated agent named "Headless" in the data directory's agent list, so they never replace the session of an agent the terminal shows. Each run starts a new session unless `--continue` (the Headless agent's latest session) or `--resume <session_id>` is given. A session id is `<agent id>:<session start ms>`, printed as `session_id` in the `json` and `stream-json` output; `--resume` also accepts ids of terminal agents' sessions, and a bare number names a session of the Headless agent. The terminal can open the Headless agent and `/resume` its sessions too.
- **Background jobs.** When the agent ends its turn while jobs it started are still running, the run waits for them (up to `AURA_HEADLESS_JOB_WAIT`, default 10 minutes, and within `--max-time`) and gives the agent each exit notice as a new turn, as the terminal does. Jobs still running at the end are stopped.
- **Permissions.** Nothing prompts. A tool call the policy would ask about is denied, and so is any tool outside `--allowed-tools` or inside `--disallowed-tools`; filtered tools are also left out of what the model is offered. `todo_write` is always allowed unless disallowed, and `job_output` / `job_kill` come with `run_command`. `--mode plan`, `ask` and `debug` behave as the terminal's modes of those names, with every ask denied. `--mode agent` (the default) lets edits and commands run, where the terminal's `default` mode would ask before them; `AURA_TUI_MODE` only sets the terminal's starting mode. Agents spawned with `task` get the same mode and tool filter.
- **Limits.** `--max-turns` counts model calls across the whole run. `--max-time` and `--max-cost-usd` also apply to the whole run and default to `AURA_TUI_TURN_TIMEOUT` and `AURA_TUI_MAX_TURN_COST_USD`. A limit stops the agent between steps, so the work done so far is kept in the session and `--resume` can continue it. A limit that is reached while the agent writes its final reply does not count as stopping it.
- **Output.** stdout carries only the chosen format; logs go to stderr (warnings only by default, `--verbose` or `RUST_LOG` for more).
- **Store lock.** RocksDB lets one process open a data directory at a time, so `aura -p` cannot share a data directory with a running terminal. Point `AURA_DATA_DIR` elsewhere to run both.

### Flags for `-p`

Each requires `-p`.

| Flag | Default | Description |
|------|---------|-------------|
| `--output-format <text\|json\|stream-json>` | `text` | What is printed; see below. |
| `--resume <SESSION_ID>` | — | Continue the saved session with this id. |
| `-c, --continue` | off | Continue the Headless agent's latest session. |
| `--allowed-tools <TOOLS>` | all | Only these tools may run (comma- or space-separated, repeatable). |
| `--disallowed-tools <TOOLS>` | none | These tools may not run. Wins over `--allowed-tools`. |
| `--max-turns <N>` | no limit | Stop after N model calls. |
| `--max-time <DURATION>` | `AURA_TUI_TURN_TIMEOUT` | Stop after this long: `90s`, `45m`, `2h`, or a bare number of minutes. |
| `--max-cost-usd <USD>` | `AURA_TUI_MAX_TURN_COST_USD` | Stop once model calls cost more than this at API list prices. |
| `-d, --cwd <DIR>` | current directory | Work in this directory. |
| `--model <MODEL>` | the terminal's choice | Model for this run only; the `/model` choice is not changed. A local model whose server is not ready fails the run at once. |
| `--provider <anthropic\|mock>` | `anthropic` | `mock` answers without a model, for tests. |
| `--verbose` | off | Debug logging to stderr. |
| `--mode <agent\|plan\|ask\|debug>` | `AURA_DEFAULT_MODE`, else `agent` | Global flag; the session mode. |

### Output formats

- **`text`** prints the agent's final reply. Errors go to stderr.
- **`json`** prints one object:

  | Field | Meaning |
  |-------|---------|
  | `type` | Always `"result"`. |
  | `exit_reason` | `success`, `max_turns`, `max_time`, `max_cost`, `error` or `interrupted`. |
  | `is_error` | `true` unless `exit_reason` is `success`. |
  | `result` | The agent's final reply. |
  | `session_id` | Pass to `--resume`. |
  | `model` | Model the run used. |
  | `num_turns` | Model calls made by this run. |
  | `agent_turns` | Agent turns: the prompt plus one per background-job notice. |
  | `duration_ms` | Wall time. |
  | `usage` | `calls`, `input_tokens` (neither read from nor written to the cache), `cache_read_tokens`, `cache_write_tokens`, `output_tokens`, `cache_hit_rate` (cache reads over all prompt tokens) and `cost_usd`. |
  | `total_cost_usd` | Estimated cost at API list prices, or `null` when no call had a price. |
  | `compactions` | History summaries made to stay within the context window. |
  | `todos` | The agent's todo list at the end (`id`, `content`, `status`). |
  | `files_changed` | Files the agent wrote, edited or deleted with its file tools. |
  | `error` | What went wrong, when `is_error`. |

  A failure before the run starts (for example an unknown `--resume` id) is reported the same way with `exit_reason: "error"`.
- **`stream-json`** prints one JSON object per line as the run goes, each with a `type`: `init` (session id, agent id, model, working directory, mode, offered tools, whether it resumed), `text_delta`, `thinking_delta`, `tool_use` (a call started: `id`, `name`), `tool_result` (`id`, `name`, `input`, `content`, `is_error`), `stream_reset` (drop text streamed since the last tool result), `todos`, `usage` (one model call's tokens and cost), `compacted`, `warning`, `error`, `notice` (a background-job notice starts another turn), and finally the `result` object above.

### Exit codes

| Code | Meaning |
|------|---------|
| 0 | The agent finished. |
| 1 | The run failed (model error, unknown session, unreadable data directory, …). |
| 2 | Bad command-line arguments. |
| 3 | `--max-turns`, `--max-time` or `--max-cost-usd` stopped the run. |
| 130 | Interrupted with Ctrl+C. |

## Global flags

| Flag | Default | Description |
|------|---------|-------------|
| `--mode <agent\|plan\|ask\|debug>` | `AURA_DEFAULT_MODE`, else `agent` | Session mode for the terminal UI and `aura -p`. The kernel denies any tool call the mode does not allow. In the terminal, `agent` starts in the `default` permission mode, which asks before edits and commands; `/mode` changes it later. `aura -p` cannot ask, so there `agent` lets edits and commands run. Headless `--ui none` ignores the flag with a warning; there, each `POST /v1/run` carries its own optional `mode`. |
| `-p, --print [PROMPT]` | — | Run one prompt without the terminal UI; see [`aura -p`](#aura--p-one-prompt-without-the-terminal). |

## `aura run` flags

| Flag | Default | Description |
|------|---------|-------------|
| `--ui <terminal\|none>` | `terminal` | Terminal UI (ratatui) or headless node. |
| `--theme <name>` | `cyber` | One of `cyber`, `matrix`, `synthwave`, `minimal`. |
| `-d, --dir <path>` | — | Override the working / data directory. |
| `--provider <anthropic\|mock>` | `anthropic` | Model provider for the session. `anthropic` serves `local/*` models from Ollama and everything else from aura-router. |
| `-v, --verbose` | off | Verbose tracing output. |
| `--allow-unrestricted-full-access` | off | Let FullAccess sessions bypass command allowlists (operator opt-in). |

## Plan mode in the terminal

Plan mode lets the agent research a task and write a plan before it touches code.

| Command | What it does |
|---------|--------------|
| `/plan` or Shift+Tab | Switch to plan mode (Shift+Tab switches back to agent mode). The agent can read, search, run commands and write Markdown files, but not edit other files, use the network or spawn agents. |
| `/mode <agent\|plan\|ask\|debug>` | Switch to any session mode. |
| `/build [plan path]` | Switch to agent mode and implement the latest plan, or the given one. |

In plan mode, ask for what you want; the agent may ask a question or two, then saves a plan to `.aura/plans/<name>_<id>.plan.md` in its workspace: YAML frontmatter (`name`, `overview`, `todos`) followed by the plan in Markdown. Keep chatting to revise it. While you build, the agent marks todos `in_progress` and `completed` with `todo_write`, and the strip above the input shows its progress. The mode badge next to the model name shows any mode other than agent.

Every mode switch is written to the agent's record as a `mode_change` entry, and the kernel enforces the new mode from then on. A mode can't be changed while a turn is running.
