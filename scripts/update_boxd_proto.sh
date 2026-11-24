#!/bin/sh
# Refresh boxd's vendored gRPC proto from the published reference.
# The proto is compiled by crates/engine/aura-engine-cloud/build.rs.
set -eu
dest="crates/engine/aura-engine-cloud/proto/boxd/api/v1/api.proto"
tmp=$(mktemp)
curl -fsSL https://docs.boxd.sh/reference/grpc-proto.md -o "$tmp"
awk '/^```protobuf/{f=1;next} /^```/{if(f){exit}} f' "$tmp" > "$dest.new"
rm -f "$tmp"
grep -q '^service BoxdApi' "$dest.new" || { echo "no BoxdApi service in the download" >&2; rm -f "$dest.new"; exit 1; }
mv "$dest.new" "$dest"
echo "updated $dest ($(wc -l < "$dest") lines); run: cargo test -p aura-engine-cloud"
