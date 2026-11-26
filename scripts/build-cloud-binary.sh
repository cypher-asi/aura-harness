#!/usr/bin/env bash
# Builds the `aura` binary that cloud machines run and installs it where
# the cloud hub looks for it ($AURA_HOME/cloud/bin/aura, default
# ~/.aura/cloud/bin/aura).
#
# Rebuild it after the node side changes: the hub asks the binary for its
# node protocol (`aura node protocol`) and refuses one older than itself,
# both when building the base image and on machines already placed.
#
# The boxd machines run Ubuntu with glibc 2.39, so the binary must not need
# a newer glibc. The script builds natively first; when the host's toolchain
# produces a binary that needs a newer glibc (e.g. on a rolling-release
# distro), it rebuilds inside a Debian bookworm container (glibc 2.36).
#
# Usage: scripts/build-cloud-binary.sh [--docker]
#   --docker   skip the native attempt and build in the container.
#
# Override the install location with AURA_CLOUD_BINARY_OUT=/path/to/aura.
set -euo pipefail

MAX_GLIBC_MINOR=39
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
AURA_HOME="${AURA_HOME:-$HOME/.aura}"
OUT="${AURA_CLOUD_BINARY_OUT:-$AURA_HOME/cloud/bin/aura}"
IMAGE="${AURA_CLOUD_BUILD_IMAGE:-rust:1-bookworm}"

if [ "$(uname -s)" != "Linux" ] || [ "$(uname -m)" != "x86_64" ]; then
  FORCE_DOCKER=1
else
  FORCE_DOCKER=0
fi
if [ "${1:-}" = "--docker" ]; then
  FORCE_DOCKER=1
fi

# Highest GLIBC_2.<minor> the binary asks for (0 when none).
glibc_minor() {
  { grep -aoE 'GLIBC_2\.[0-9]+' "$1" || true; } \
    | sed 's/GLIBC_2\.//' | sort -n | tail -n 1 | sed 's/^$/0/'
}

compatible() {
  local minor
  minor="$(glibc_minor "$1")"
  echo "$1 needs glibc 2.${minor} (cloud machines have 2.${MAX_GLIBC_MINOR})" >&2
  [ "$minor" -le "$MAX_GLIBC_MINOR" ]
}

build_native() {
  echo "==> native release build (target/cloud)" >&2
  (cd "$ROOT" && cargo build --release --locked --no-default-features --bin aura \
    --target-dir target/cloud)
  echo "$ROOT/target/cloud/release/aura"
}

build_docker() {
  command -v docker >/dev/null || {
    echo "docker is needed to build for glibc 2.${MAX_GLIBC_MINOR}" >&2
    exit 1
  }
  echo "==> release build in $IMAGE (target/cloud-docker)" >&2
  docker run --rm --platform linux/amd64 \
    -v "$ROOT":/src -w /src \
    -v aura-cloud-cargo-registry:/usr/local/cargo/registry \
    -e CARGO_TARGET_DIR=/src/target/cloud-docker \
    "$IMAGE" bash -c "
      set -e
      apt-get update -qq
      DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends \
        clang libclang-dev llvm-dev cmake pkg-config libssl-dev libdbus-1-dev >/dev/null
      rustup show active-toolchain >/dev/null
      cargo build --release --locked --no-default-features --bin aura
      strip target/cloud-docker/release/aura
      chown -R $(id -u):$(id -g) target/cloud-docker
    " >&2
  echo "$ROOT/target/cloud-docker/release/aura"
}

BIN=""
if [ "$FORCE_DOCKER" = 0 ]; then
  BIN="$(build_native)"
  if ! compatible "$BIN"; then
    echo "==> native build needs a newer glibc; rebuilding in a container" >&2
    BIN=""
  fi
fi
if [ -z "$BIN" ]; then
  BIN="$(build_docker)"
  compatible "$BIN" || { echo "container build still needs a newer glibc" >&2; exit 1; }
fi

mkdir -p "$(dirname "$OUT")"
install -m 0755 "$BIN" "$OUT.tmp"
mv -f "$OUT.tmp" "$OUT"
echo "installed $OUT"
