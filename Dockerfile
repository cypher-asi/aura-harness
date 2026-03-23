# ── Builder ──────────────────────────────────────────────────────────────────
FROM rust:1.86-bookworm AS builder

RUN apt-get update && apt-get install -y --no-install-recommends \
        libclang-dev llvm-dev cmake pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY src              src/
COPY crates/aura-core         crates/aura-core/
COPY crates/aura-store        crates/aura-store/
COPY crates/aura-executor     crates/aura-executor/
COPY crates/aura-tools        crates/aura-tools/
COPY crates/aura-reasoner     crates/aura-reasoner/
COPY crates/aura-kernel       crates/aura-kernel/
COPY crates/aura-runtime      crates/aura-runtime/
COPY crates/aura-node         crates/aura-node/
COPY crates/aura-terminal     crates/aura-terminal/
COPY crates/aura-cli          crates/aura-cli/
COPY crates/aura-agent        crates/aura-agent/
COPY crates/aura-agent-fileops crates/aura-agent-fileops/
COPY crates/aura-agent-verify crates/aura-agent-verify/
COPY crates/aura-auth         crates/aura-auth/
COPY crates/aura-automaton    crates/aura-automaton/
COPY crates/aura-session      crates/aura-session/

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

ENTRYPOINT ["aura", "--ui", "none"]
