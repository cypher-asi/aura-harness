#!/bin/sh
# Copies the Gate post and its charts into the website repo.
# Run from anywhere: sh .aura/out/gate-post/apply.sh [path/to/cypher-website-vercel]
set -eu
here=$(cd "$(dirname "$0")" && pwd)
site=${1:-"$here/../../../../cypher-website-vercel"}
site=$(cd "$site" && pwd)

mkdir -p "$site/content/posts"
cp "$here/content/posts/gate-router-design.md" "$site/content/posts/"
cp "$here/content/posts/gate-router-design.research.md" "$site/content/posts/"
for f in data.ts gate-data.ts PostChart.tsx PostChart.module.css gate.test.tsx; do
  cp "$here/src/features/terminal/charts/$f" "$site/src/features/terminal/charts/$f"
done
echo "Copied into $site. Next: cd $site && npm run typecheck && npm test"
