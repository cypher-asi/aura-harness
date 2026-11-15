#!/usr/bin/env bash
# eval-live.sh — run the coding eval suite against a live model and compare
# the result with the checked-in baseline.
#
#   AURA_EVAL_MODEL=<model> bash scripts/eval-live.sh            # compare
#   AURA_EVAL_MODEL=<model> bash scripts/eval-live.sh --write    # refresh baseline
#
# Needs a signed-in `aura` (or AURA_ROUTER_JWT) that can use the model.
# Runs go to a throwaway AURA_HOME unless AURA_HOME is set. Exits non-zero
# when a metric or a task's pass rate regresses beyond AURA_EVAL_TOLERANCE.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
model="${AURA_EVAL_MODEL:?set AURA_EVAL_MODEL to the model id to evaluate}"
repeat="${AURA_EVAL_REPEAT:-3}"
tolerance="${AURA_EVAL_TOLERANCE:-0.1}"
baseline="${AURA_EVAL_BASELINE:-$root/evals/coding/baseline.json}"
export AURA_HOME="${AURA_HOME:-$(mktemp -d)}"
export AURA_TRACE="${AURA_TRACE:-full}"

aura=(cargo run --quiet --release --manifest-path "$root/Cargo.toml" --bin aura --)

"${aura[@]}" eval run "$root/evals/coding" --model "$model" --repeat "$repeat" --arm live

if [[ "${1:-}" == "--write" ]]; then
  "${aura[@]}" eval compare --suite coding --arm live --write-baseline "$baseline"
  exit 0
fi

if [[ ! -f "$baseline" ]]; then
  echo "no baseline at $baseline; run with --write first" >&2
  exit 2
fi
"${aura[@]}" eval compare --suite coding --arm live --baseline "$baseline" \
  --tolerance "$tolerance" --fail-on-regression
