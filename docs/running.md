# Running Aura

## Prerequisites

- A Rust toolchain (pinned by [`rust-toolchain.toml`](../rust-toolchain.toml)).
- LLVM / Clang, for the RocksDB build.
- On Linux, `libdbus-1-dev` and `pkg-config` for the keyring's Secret Service backend:
  - Debian / Ubuntu: `sudo apt install libdbus-1-dev pkg-config`
  - Fedora: `sudo dnf install dbus-devel pkgconf-pkg-config`

macOS and Windows need no extra system packages. Everything else, including the wire-protocol types, lives in this workspace; there is no sibling-repo checkout.

## Build and run

```sh
cargo build --release

# Terminal UI — defaults to a small local model, no account needed
ollama pull qwen3:4b                      # one-time, ~2.5 GB
OLLAMA_CONTEXT_LENGTH=32768 ollama serve  # in another terminal
cargo run

# Terminal UI in Plan mode
cargo run -- --mode plan

# Headless node, same binary
cargo run -- run --ui none

# Standalone server binary
cargo run -p aura-runtime --bin aura-node
```

The node listens on `127.0.0.1:8080` by default. Start work with `POST /v1/run` and follow it on `WS /stream/:run_id`; see [api.md](api.md). Environment variables are listed in [configuration.md](configuration.md); copy [`.env.example`](../.env.example) as a starting point.

## Local model by default, login optional

The terminal UI starts on `local/qwen3:4b`, served by [Ollama](https://ollama.com) (0.14+, which exposes the Anthropic-compatible `/v1/messages`). It runs on CPU and supports tool calling. Nothing is sent to aura-router and no login is needed. If Ollama isn't running or the model isn't pulled, the chat shows setup instructions.

- `/model` opens the model selector (↑/↓, Enter, Esc). `/model <id>` switches directly, for example `/model local/qwen3:8b`. The header shows the active model, backend, and user, such as `qwen3:4b · local · guest`.
- `/login` inside the chat unlocks the aura-router cloud models. Picking a cloud model as a guest starts the login flow and switches to that model once login succeeds. `/logout` returns to the local default.
- The last choice is saved to `AURA_HOME/tui.json`. `AURA_DEFAULT_MODEL` overrides it.
- Other local options: `qwen3:1.7b` for very small machines, or `qwen3:8b` / `qwen2.5-coder:7b` for better quality. Set `AURA_LOCAL_MODEL` to change the default.

The headless node and `aura-node` are router-only.

## Docker

The [`Dockerfile`](../Dockerfile) builds the workspace from the repo root:

```sh
docker build -t aura .
docker run -p 8080:8080 aura
```

The image runs `aura run --ui none` as a non-root user, listens on `:8080`, and sets `AURA_DATA_DIR=/data`.

## Hosted deployment (Render)

Use [`Dockerfile.render`](../Dockerfile.render). It binds to `0.0.0.0:10000`, exposes `/health`, and is published by the `Publish GHCR image` workflow as `ghcr.io/cypher-asi/aura-harness:<tag>`. Non-loopback services should require bearer auth:

```sh
AURA_LISTEN_ADDR=0.0.0.0:10000
AURA_NODE_REQUIRE_AUTH=1
AURA_NODE_AUTH_TOKEN=<shared-secret>
AURA_OS_SERVER_URL=https://YOUR-AURA-OS-SERVICE.onrender.com
```

Pair that with `LOCAL_HARNESS_URL=https://YOUR-HARNESS-SERVICE.onrender.com` and `LOCAL_HARNESS_AUTH_TOKEN=<shared-secret>` on `aura-os`. The shared secret authenticates only the transport hop; user and model auth still travel inside `RuntimeRequest.auth_jwt`.
