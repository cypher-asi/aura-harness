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

## Desktop app

`aura-desktop` is the terminal UI in a native window: the same agent session, the same screen and the same keys, drawn on the GPU (wgpu, through [iced](https://iced.rs)) on a cell grid instead of through a terminal emulator, with a CPU fallback (tiny-skia) when no GPU is available. Box-drawing lines, block characters, braille spinners and status symbols are drawn as shapes, so borders join without gaps and symbols missing from the font still render. It needs Rust 1.88 or later.

```sh
cargo run --release -p aura-desktop                        # agent in the current directory
cargo run --release -p aura-desktop -- --workspace ~/proj  # agent in another folder
cargo run --release -p aura-desktop -- --mode plan
cargo run --release -p aura-desktop -- --demo              # scripted session, no model needed
```

It takes the terminal's `--dir`, `--provider`, `--mode`, `--theme`, `--verbose` and `--allow-unrestricted-full-access`. Every terminal key works the same way, including Ctrl+C (copy a selection, interrupt the agent, or quit when idle) and Ctrl+D. Drag to select text in a panel; it's copied when you let go. On top of that, the window handles what a terminal emulator would:

| Keys (Cmd on macOS) | Action |
|---|---|
| Ctrl+Shift+C | Copy the selection |
| Ctrl+V, Ctrl+Shift+V, Shift+Insert | Paste into the input, as in the terminal: an image, or a path to one, is attached |
| Ctrl+= / Ctrl+- / Ctrl+0 | Larger / smaller / default text |

Turn alerts follow the terminal's settings; where the terminal rings the bell, the window asks for attention instead.

Text is set in Geist Mono, bundled (SIL Open Font License, [`assets/fonts/OFL.txt`](../crates/aura-desktop/assets/fonts/OFL.txt)).

| Variable | Effect |
|---|---|
| `AURA_DESKTOP_MONO_FONT` | Another installed monospace family, e.g. `JetBrains Mono` |
| `AURA_DESKTOP_FONT_FILES` | Extra font files to load, separated like `PATH` |
| `AURA_DESKTOP_FONT_SIZE` | Starting text size in pixels (default 14) |
| `AURA_DESKTOP_PERF` | Log startup time and frame / update / view timings to stderr |

On Linux the window opens on Wayland, or on X11 when `WAYLAND_DISPLAY` is unset; nothing beyond the packages above is needed to build it. `WGPU_BACKEND=gl` forces OpenGL when Vulkan drivers misbehave, and `ICED_BACKEND=tiny-skia` forces the CPU renderer. Editing a permanent agent opens `$VISUAL` / `$EDITOR`; editors that need a terminal (vim, nano, helix…) can't run in a window, so the file opens in the system's default app instead and is reloaded the first time you save it.

The node listens on `127.0.0.1:8080` by default. Start work with `POST /v1/run` and follow it on `WS /stream/:run_id`; see [api.md](api.md). Environment variables are listed in [configuration.md](configuration.md); copy [`.env.example`](../.env.example) as a starting point.

## Local model by default, login optional

The terminal UI starts on `gate`, which with nothing signed in sends every turn to `local/qwen3:4b`, served by [Ollama](https://ollama.com) (0.14+, which exposes the Anthropic-compatible `/v1/messages`). It runs on CPU and supports tool calling. Nothing is sent to aura-router and no login is needed. If Ollama isn't running or the model isn't pulled, the chat shows setup instructions.

- `/model` opens the model selector (↑/↓, Enter, Esc). `/model <id>` switches directly, for example `/model local/qwen3:8b`. The header shows the active model, backend, and user, such as `qwen3:4b · local · guest`.
- `/login` inside the chat unlocks the aura-router cloud models. Picking a cloud model as a guest starts the login flow and switches to that model once login succeeds. `/logout` switches a cloud model back to `gate`.
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
