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
