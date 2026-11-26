#!/usr/bin/env bash
# check_file_size.sh: no Rust source file may be longer than 500 lines.
#
# Every tracked `.rs` file counts, tests included, except the eval tasks
# under `evals/`: their workspaces and checks are benchmark files kept
# exactly as published.
#
# Run from anywhere: `bash scripts/check_file_size.sh`.

set -euo pipefail
export LC_ALL=C  # one sort order everywhere, so the report reads the same

MAX_LINES=500
cd "$(dirname "$0")/.."

over_limit() {
    # Tracked and new (untracked, not ignored) files; skip ones deleted
    # from the working tree but not yet from the index.
    git ls-files -z --cached --others --exclude-standard -- '*.rs' ':!:evals/' |
        while IFS= read -r -d '' file; do
            [[ -f "$file" ]] && printf '%s\0' "$file"
        done |
        sort -zu |
        xargs -0 -r wc -l |
        awk -v max="$MAX_LINES" '{ n = $1; sub(/^ *[0-9]+ /, "") } $0 != "total" && n > max { print n "\t" $0 }'
}

failures=0
while IFS=$'\t' read -r lines file; do
    echo "::error file=${file}::${file} has ${lines} lines (limit ${MAX_LINES}); split it into submodules"
    failures=$((failures + 1))
done < <(over_limit)

if (( failures > 0 )); then
    echo "file size check failed: ${failures} file(s) over ${MAX_LINES} lines" >&2
    exit 1
fi

echo "file size check passed"
