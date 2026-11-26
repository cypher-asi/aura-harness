#!/usr/bin/env bash
# check_file_size.sh: no Rust source file may be longer than 500 lines.
#
# Every tracked `.rs` file counts, tests included. Files that were over
# the limit when the gate landed are listed in
# `scripts/file_size_allowlist.txt`; the list may only shrink:
#
#   - a file over the limit that is not listed fails the check;
#   - a listed file that is now within the limit, or no longer exists,
#     also fails, so its entry is removed in the same change that fixed it.
#
# Run from anywhere: `bash scripts/check_file_size.sh`.
# `--update` rewrites the allowlist to the files over the limit today
# (use it only to shrink the list, never to add to it).

set -euo pipefail
export LC_ALL=C  # one sort order everywhere, so the allowlist diffs cleanly

MAX_LINES=500
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ALLOWLIST="$REPO_ROOT/scripts/file_size_allowlist.txt"
cd "$REPO_ROOT"

over_limit() {
    # Tracked and new (untracked, not ignored) files; skip ones deleted
    # from the working tree but not yet from the index.
    git ls-files -z --cached --others --exclude-standard -- '*.rs' |
        while IFS= read -r -d '' file; do
            [[ -f "$file" ]] && printf '%s\0' "$file"
        done |
        sort -zu |
        xargs -0 wc -l |
        awk -v max="$MAX_LINES" '$2 != "total" && $1 > max { print $2 }' |
        sort
}

current="$(over_limit)"

if [[ "${1:-}" == "--update" ]]; then
    {
        grep "^#" "$ALLOWLIST" 2>/dev/null || true
        printf "%s\n" "$current" | sed "/^$/d"
    } > "$ALLOWLIST.tmp"
    mv "$ALLOWLIST.tmp" "$ALLOWLIST"
    echo "wrote $(grep -c . "$ALLOWLIST" || true) entries to $ALLOWLIST"
    exit 0
fi

allowed=""
if [[ -f "$ALLOWLIST" ]]; then
    allowed="$(grep -v '^[[:space:]]*\(#\|$\)' "$ALLOWLIST" | sort)"
fi

failures=0

# Over the limit and not allowlisted.
while IFS= read -r file; do
    [[ -z "$file" ]] && continue
    lines=$(wc -l < "$file")
    echo "::error file=${file}::${file} has ${lines} lines (limit ${MAX_LINES}); split it into submodules"
    failures=$((failures + 1))
done < <(comm -23 <(printf '%s\n' "$current") <(printf '%s\n' "$allowed"))

# Allowlisted but no longer over the limit: shrink the list.
while IFS= read -r file; do
    [[ -z "$file" ]] && continue
    echo "::error file=scripts/file_size_allowlist.txt::${file} is within ${MAX_LINES} lines or gone; remove it from scripts/file_size_allowlist.txt"
    failures=$((failures + 1))
done < <(comm -13 <(printf '%s\n' "$current") <(printf '%s\n' "$allowed"))

if (( failures > 0 )); then
    echo "file size check failed: ${failures} problem(s)" >&2
    exit 1
fi

echo "file size check passed ($(printf '%s\n' "$allowed" | grep -c . || true) files still allowlisted)"
