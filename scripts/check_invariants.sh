#!/usr/bin/env bash
# check_invariants.sh — rg-band enforcement for architectural invariants §1, §2, §3, §9.
#
# This script is executed from CI (and can be run locally: `bash scripts/check_invariants.sh`).
# It uses ripgrep to detect forbidden code patterns outside their allowed modules and fails
# with exit code 1 on the first violation. See `docs/invariants.md` for the full contracts.
#
# When a legitimate new call-site needs to land (e.g. a newly-approved gateway that
# routes `.complete(` through the kernel), add its path to the corresponding allowlist
# regex below. Each addition should be justified in the PR description alongside the
# invariant it respects.

set -euo pipefail

if ! command -v rg >/dev/null 2>&1; then
    echo "error: ripgrep (rg) is required to run the invariant check." >&2
    exit 2
fi

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

violations=0

# Emit a violation message and mark the run as failed without aborting
# the rest of the checks — we surface every band in one run.
report() {
    local invariant="$1"; shift
    local description="$1"; shift
    local match_file="$1"; shift
    echo "::error file=${match_file}::Invariant ${invariant} violation (${description}): ${match_file}"
    violations=$((violations + 1))
}

# Run `rg` against the repo, stream each match through an allowlist
# regex, and report anything left as a violation. Using a PCRE-ish
# allowlist keeps the matrix readable and avoids --glob explosions.
run_band() {
    local invariant="$1"; shift
    local description="$1"; shift
    local pattern="$1"; shift
    local allow_regex="$1"; shift

    # `|| true` so rg's own exit code (1 on zero matches) doesn't trip `set -e`.
    local raw
    raw=$(rg -n --hidden --glob '!target/**' --glob '!.git/**' --type rust "$pattern" || true)
    if [[ -z "$raw" ]]; then
        return 0
    fi

    while IFS= read -r line; do
        # Strip line/column prefix to get the path.
        local path="${line%%:*}"
        if [[ "$path" =~ $allow_regex ]]; then
            continue
        fi
        report "$invariant" "$description" "$line"
    done <<<"$raw"
}

# §1/§3 — `.complete(` must only appear inside:
#   - the kernel itself
#   - the agent-side recording seams (kernel_gateway.rs, recording_stream.rs)
#   - the reasoner provider internals and their mocks
#   - the automaton runtime (wraps its provider with KernelModelGateway in
#     aura-node before handing it over)
#   - the memory subsystem, which only ever holds an Arc<KernelModelGateway>
#   - any *test* file (unit, integration, harness shims)
run_band "§1/§3" "direct ModelProvider::complete call outside the recording seam" \
    '\.complete\(' \
    '^(crates/aura-kernel/|crates/aura-agent/src/kernel_gateway\.rs|crates/aura-agent/src/recording_stream\.rs|crates/aura-agent/src/agent_loop/|crates/aura-agent/src/event_sequence_tests\.rs|crates/aura-reasoner/|crates/aura-automaton/|crates/aura-memory/|.*/tests/|.*test.*\.rs|.*tests.*\.rs)'

# §2 — direct store append functions bypass the kernel's seq/context-hash
# pipeline unless they are inside aura-kernel, aura-store, or tests.
run_band "§2" "append_entry_* used outside aura-kernel / aura-store / tests" \
    'append_entry_(atomic|dequeued|direct|entries_batch)' \
    '^(crates/aura-kernel/|crates/aura-store/|.*/tests/|.*test.*\.rs|.*tests.*\.rs)'

# §1 — raw `git` processes must live in a kernel-mediated executor. Phase 2
# will introduce the `GitExecutor` and tighten this band. For now the
# allowlist pins the current call-sites so any *new* location triggers a
# failure.
run_band "§1" "Command::new(\"git\") outside the approved call-sites (phase2 TODO: move to GitExecutor)" \
    'Command::new\("git"\)' \
    '^(crates/aura-agent/src/git\.rs|crates/aura-automaton/src/builtins/dev_loop/tick\.rs|crates/aura-tools/src/domain_tools/orbit\.rs|.*/tests/|.*test.*\.rs|.*tests.*\.rs)'

# §9 — the agent loop must not reach into aura-store directly. Any code
# that needs persistence goes through the kernel. Test files in the same
# tree are exempt since they assemble scaffolding.
store_hits=$(rg -n --hidden --glob '!target/**' --glob '!.git/**' --type rust \
    --glob 'crates/aura-agent/src/agent_loop/**' \
    --glob '!**/*test*.rs' --glob '!**/*tests*.rs' \
    'use aura_store::' || true)
if [[ -n "$store_hits" ]]; then
    while IFS= read -r line; do
        report "§9" "aura-agent/agent_loop must not depend on aura-store" "$line"
    done <<<"$store_hits"
fi

if (( violations > 0 )); then
    echo ""
    echo "Invariant band check failed with ${violations} violation(s)." >&2
    echo "See docs/invariants.md and scripts/check_invariants.sh for the allowed call-sites." >&2
    exit 1
fi

echo "Invariant band check passed."
