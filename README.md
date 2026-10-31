# AURA

AURA is a runtime for AI agents. You give an agent a task; it thinks with a language model, acts through tools (reading and writing files, running commands, calling services), and writes every step it takes to its own permanent log. Afterwards you can read that log to see exactly what the agent saw, decided and did. Many agents can run at the same time without getting in each other's way.

You can use AURA from a terminal UI on your machine, or run it as a server that other apps (such as `aura-os`) drive over HTTP and WebSocket.

## Principles

1. **Every agent keeps its own audit log.** Each agent has one append-only log. Nothing is edited or deleted, so any agent's history can be read back from its log alone.
2. **Every action goes through the agent's kernel.** Model calls, tool calls and spawning helper agents all pass through one checkpoint that decides whether they're allowed and records them.
3. **One writer per agent; separate agents run in parallel.** Only one task advances a given agent at a time, so its log stays in order, while unrelated agents run concurrently.
4. **The wire API is the contract.** Outside apps talk to AURA over HTTP and WebSocket. The Rust crates are internal packaging, not a public library.

Each session also runs in a mode (`agent`, `plan`, `ask` or `debug`) that limits what its tools may do; for example, `plan` can write Markdown plans but not code.

## How to run

You need Rust and LLVM/Clang (Linux also needs `libdbus-1-dev` and `pkg-config`; see [running](docs/running.md)).

```sh
ollama pull qwen3:4b        # one-time; the terminal UI defaults to this local model
cargo run                   # terminal UI (no account needed)
cargo run -- --mode plan    # terminal UI in plan mode
cargo run -- login          # optional: unlock cloud models (or /login in the TUI)
cargo run -- run --ui none  # headless server on 127.0.0.1:8080
```

In the terminal UI, `/model` switches between local and cloud models; see [running](docs/running.md#local-model-by-default-login-optional).

Or with Docker:

```sh
docker build -t aura . && docker run -p 8080:8080 aura
```

## Resources

| Page | What's in it |
|------|--------------|
| [Running and deployment](docs/running.md) | Prerequisites, build commands, Docker, hosted (Render) setup. |
| [API](docs/api.md) | Every HTTP and WebSocket endpoint, starting with `POST /v1/run`. |
| [CLI](docs/cli.md) | The `aura` and `aura-node` binaries, commands and flags. |
| [Configuration](docs/configuration.md) | Environment variables and authentication. |
| [Architecture](docs/architecture.md) | The ten-layer crate map, request flow, mode resolution, tool authorization, per-crate reference. |
| [Invariants](docs/invariants.md) | The 15 guarantees behind the principles and the CI checks that enforce them. |
| [Development](docs/development.md) | Format, lint and test commands, CI gates, how to add crates and tools. |
| [Changelog](CHANGELOG.md) | What changed, phase by phase. |

## Directory structure

```text
aura-harness/
  src/             aura binary entry point
  crates/          39 library crates in 10 layers (see docs/architecture.md)
  tests/           workspace-level integration and boundary tests
  evals/           agent evaluation scenarios
  docs/            the pages listed under Resources
  scripts/         check_invariants.sh (CI invariant gate)
  .github/         CI workflows
  Cargo.toml       workspace root
  Dockerfile       container image (headless, port 8080)
  .env.example     environment variable template
```

## License

MIT (declared in [`Cargo.toml`](Cargo.toml)).
