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

# Terminal UI (log in once with `cargo run -- login`)
cargo run

# Terminal UI in Plan mode
cargo run -- --mode plan

# Headless node, same binary
cargo run -- run --ui none

# Standalone server binary
cargo run -p aura-runtime --bin aura-node
```

The node listens on `127.0.0.1:8080` by default. Start work with `POST /v1/run` and follow it on `WS /stream/:run_id`; see [api.md](api.md). Environment variables are listed in [configuration.md](configuration.md); copy [`.env.example`](../.env.example) as a starting point.

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
