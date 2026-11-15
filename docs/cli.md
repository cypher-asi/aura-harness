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

## Global flags

| Flag | Default | Description |
|------|---------|-------------|
| `--mode <agent\|plan\|ask\|debug>` | `AURA_DEFAULT_MODE`, else `agent` | Session mode for the terminal UI. The kernel denies any tool call the mode does not allow. Headless `--ui none` ignores the flag with a warning; there, each `POST /v1/run` carries its own optional `mode`. |

## `aura run` flags

| Flag | Default | Description |
|------|---------|-------------|
| `--ui <terminal\|none>` | `terminal` | Terminal UI (ratatui) or headless node. |
| `--theme <name>` | `cyber` | One of `cyber`, `matrix`, `synthwave`, `minimal`. |
| `-d, --dir <path>` | — | Override the working / data directory. |
| `--provider <anthropic\|mock>` | `anthropic` | Model provider for the session. `anthropic` serves `local/*` models from Ollama and everything else from aura-router. |
| `-v, --verbose` | off | Verbose tracing output. |
| `--allow-unrestricted-full-access` | off | Let FullAccess sessions bypass command allowlists (operator opt-in). |
