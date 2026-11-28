#!/usr/bin/env bash
# Share compiled crates between checkouts with sccache. Run once per machine;
# running it again changes nothing.
#
# Every checkout and worktree builds into its own target/. sccache caches
# each crate's compiler output by its inputs, so a new worktree takes the
# dependencies it shares with the others from the cache instead of compiling
# them again, RocksDB's and aws-lc's C and C++ objects included.
#
# It:
#   1. installs sccache with `cargo install` unless it is on PATH;
#   2. where systemd runs user services, keeps one sccache server up as
#      `sccache.service`, so no server started inside a sandboxed build ever
#      owns the cache;
#   3. sets `build.rustc-wrapper` in ~/.cargo/config.toml, and
#      `SCCACHE_IGNORE_SERVER_IO_ERROR` so cargo compiles without the cache
#      when it can't reach the server (as in a sandbox without network).
#
# SCCACHE_CACHE_SIZE (default 40G) sets the cache's size.
set -euo pipefail

config="${CARGO_HOME:-$HOME/.cargo}/config.toml"

# set_key TABLE KEY VALUE: put `KEY = VALUE` under `[TABLE]` in $config,
# adding the table when there is none. A KEY already set is left alone.
set_key() {
  local table=$1 key=$2 value=$3
  mkdir -p "$(dirname "$config")"
  touch "$config"
  if awk -v t="[$table]" -v k="$key" '
      $0 == t { inside = 1; next }
      /^\[/ { inside = 0 }
      inside && $0 ~ "^[ \t]*" k "[ \t]*=" { found = 1 }
      END { exit !found }' "$config"; then
    echo "$table.$key is already set in $config; left as it is"
    return
  fi
  if grep -qxF "[$table]" "$config"; then
    awk -v t="[$table]" -v line="$key = $value" '{ print } $0 == t { print line }' \
      "$config" > "$config.tmp"
    mv "$config.tmp" "$config"
  else
    printf '\n[%s]\n%s = %s\n' "$table" "$key" "$value" >> "$config"
  fi
  echo "set $table.$key in $config"
}

if ! command -v sccache >/dev/null 2>&1; then
  echo "installing sccache"
  cargo install sccache --locked
fi
sccache=$(command -v sccache)

if command -v systemctl >/dev/null 2>&1 && systemctl --user show-environment >/dev/null 2>&1; then
  unit_dir="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
  mkdir -p "$unit_dir"
  cat > "$unit_dir/sccache.service" <<EOF
[Unit]
Description=sccache, the compiler cache cargo builds share

[Service]
Environment=SCCACHE_NO_DAEMON=1
Environment=SCCACHE_IDLE_TIMEOUT=0
Environment=SCCACHE_CACHE_SIZE=${SCCACHE_CACHE_SIZE:-40G}
ExecStart=$sccache --start-server
Restart=on-failure

[Install]
WantedBy=default.target
EOF
  # A server a build started on demand holds the port the service needs.
  systemctl --user is-active --quiet sccache.service || "$sccache" --stop-server >/dev/null 2>&1 || true
  systemctl --user daemon-reload
  systemctl --user enable --now sccache.service
  echo "sccache.service is running"
else
  echo "no systemd user session: builds start the sccache server when they need it"
fi

set_key build rustc-wrapper '"sccache"'
set_key env SCCACHE_IGNORE_SERVER_IO_ERROR '"1"'

echo "done; \`sccache --show-stats\` shows the cache's hits after a build"
