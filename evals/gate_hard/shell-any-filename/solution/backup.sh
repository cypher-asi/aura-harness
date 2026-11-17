#!/bin/sh
# Copy every regular .txt file directly inside SRC into DEST.
src=$1
dest=$2
mkdir -p -- "$dest"
for f in "$src"/*.txt; do
  [ -f "$f" ] || continue
  cp -- "$f" "$dest"/
done
