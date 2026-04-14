# ── Builder ──────────────────────────────────────────────────────────────────
FROM rust:1.86-bookworm AS builder

RUN apt-get update && apt-get install -y --no-install-recommends \
        libclang-dev llvm-dev cmake pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Build from the parent workspace so local path dependencies in ../aura-os
# resolve inside the container the same way they do on the host.
COPY aura-harness/Cargo.toml aura-harness/Cargo.lock aura-harness/rust-toolchain.toml ./
COPY aura-harness/src              src/
COPY aura-harness/crates/aura-core         crates/aura-core/
COPY aura-harness/crates/aura-store        crates/aura-store/
COPY aura-harness/crates/aura-tools        crates/aura-tools/
COPY aura-harness/crates/aura-reasoner     crates/aura-reasoner/
COPY aura-harness/crates/aura-kernel       crates/aura-kernel/
COPY aura-harness/crates/aura-node         crates/aura-node/
COPY aura-harness/crates/aura-memory       crates/aura-memory/
COPY aura-harness/crates/aura-terminal     crates/aura-terminal/
COPY aura-harness/crates/aura-cli          crates/aura-cli/
COPY aura-harness/crates/aura-agent        crates/aura-agent/
COPY aura-harness/crates/aura-auth         crates/aura-auth/
COPY aura-harness/crates/aura-automaton    crates/aura-automaton/
COPY aura-harness/crates/aura-skills       crates/aura-skills/
COPY aura-os/crates/aura-protocol          /aura-os/crates/aura-protocol/

RUN cargo build --release --bin aura \
    && strip target/release/aura

# ── Runtime ─────────────────────────────────────────────────────────────────
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
        libssl3 ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*

RUN groupadd -g 1000 aura \
    && useradd -u 1000 -g aura -m aura \
    && mkdir -p /data && chown aura:aura /data

COPY --from=builder /build/target/release/aura /usr/local/bin/aura

ENV AURA_LISTEN_ADDR=0.0.0.0:8080 \
    AURA_DATA_DIR=/data \
    RUST_LOG=info

EXPOSE 8080

HEALTHCHECK --interval=10s --timeout=3s --start-period=5s --retries=3 \
    CMD curl -f http://localhost:8080/health || exit 1

USER aura

ENTRYPOINT ["aura", "run", "--ui", "none"]
