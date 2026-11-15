# CLI reference

## Binaries

| Binary | Entry point | Purpose |
|--------|-------------|---------|
| `aura` | [`src/main.rs`](../src/main.rs) | The main CLI. Opens the terminal UI by default, runs headless with `aura run --ui none`, and hosts the account, plugin and subagent commands below. The CLI body lives in [`aura-surface-cli`](../crates/aura-surface-cli). |
| `aura-node` | [`crates/aura-runtime/src/main.rs`](../crates/aura-runtime/src/main.rs) | Standalone headless server exposing the [HTTP / WebSocket API](api.md). |

Older specs mention a separate `aura-cli` crate. It never shipped; read `aura-cli/src/...` paths as `src/...` in the root crate.

## Commands

Defined in [`crates/aura-surface-cli/src/cli.rs`](../crates/aura-surface-cli/src/cli.rs).

| Command | Description |
|---------|-------------|
| `aura run` (default) | Run the agent. Flags below. |
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
- **`/compact [focus]`** summarizes the older history now instead of waiting for context pressure, keeping the first message and the recent steps as they are. Text after the command tells the summary what to keep. A busy agent does it when its current work ends. The summary replaces history only in the running session; the record keeps every message.
- **Approvals.** When a tool call needs your approval, the turn pauses and the agent's chat shows what it wants to run. Press `y` to allow it once, `a` to always allow it in this project, or `n` (or Esc) to deny it. "Always" is saved per project folder in `<data_dir>/approvals.json`: for `run_command` it covers the program and its subcommand (`cargo test`, not every `cargo` command) and never matches a shell script that chains or redirects commands; for other tools it covers the tool. Commands you type in the chat are never asked about. A prompt left unanswered is denied after 10 minutes, and Ctrl+C denies it and stops the turn. An agent that is not on screen asks in its own chat, and the terminal tells you which agent is waiting.
- **Modes.** `/mode` lists the modes and `/mode <name>` picks one for the agent on screen; Shift+Tab steps to the next one. `default` asks before file edits and commands, `accept-edits` lets file edits run but still asks before commands, `plan` is read-only, and `unrestricted` never asks. Reading files and searching never ask. The status line starts with the current mode. Each switch is written to the agent's record before the next tool call uses it, and agents it spawns start in the same mode. The starting mode comes from `--mode`, then `AURA_TUI_MODE`, then `AURA_DEFAULT_MODE`; `agent` means `default`.
- **Notifications.** The terminal rings the bell when a turn that took at least 10 seconds finishes and when an agent needs approval. Desktop notifications are off unless you set `AURA_TUI_DESKTOP_NOTIFY=1`. See [Terminal long runs](configuration.md#terminal-long-runs) for the settings.

## Global flags

| Flag | Default | Description |
|------|---------|-------------|
| `--mode <agent\|plan\|ask\|debug>` | `AURA_DEFAULT_MODE`, else `agent` | Session mode for the terminal UI. The kernel denies any tool call the mode does not allow. `agent` starts in the `default` permission mode, which asks before edits and commands; `/mode` changes it later. Headless `--ui none` ignores the flag with a warning; there, each `POST /v1/run` carries its own optional `mode`. |

## `aura run` flags

| Flag | Default | Description |
|------|---------|-------------|
| `--ui <terminal\|none>` | `terminal` | Terminal UI (ratatui) or headless node. |
| `--theme <name>` | `cyber` | One of `cyber`, `matrix`, `synthwave`, `minimal`. |
| `-d, --dir <path>` | — | Override the working / data directory. |
| `--provider <anthropic\|mock>` | `anthropic` | Model provider for the session. `anthropic` serves `local/*` models from Ollama and everything else from aura-router. |
| `-v, --verbose` | off | Verbose tracing output. |
| `--allow-unrestricted-full-access` | off | Let FullAccess sessions bypass command allowlists (operator opt-in). |
