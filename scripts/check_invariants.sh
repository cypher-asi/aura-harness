#!/usr/bin/env bash
# check_invariants.sh — rg-band enforcement for architectural invariants
# §1, §2, §3, §9, §10 (see `docs/invariants.md` Principles 1 and 2).
#
# This script is executed from CI (`.github/workflows/invariants.yml`) and
# can be run locally: `bash scripts/check_invariants.sh`. It uses ripgrep
# to detect forbidden code patterns outside their allowed modules and
# fails with exit code 1 on the first violation.
#
# Crate layout:
#   - kernel:   `aura-agent-kernel/`
#   - store:    `aura-store-db/` + `aura-store-record/` (per-agent RecordLog
#               trait) + `aura-store-snapshot/` (replay snapshot trait)
#   - reasoner: `aura-model/`
#   - memory:   `aura-context-memory/`
#
# When a legitimate new call-site needs to land (e.g. a newly-approved
# gateway that routes `.complete(` through the kernel, or an engine
# module that holds a store handle for a kernel-mediated audit write), add
# its path to the corresponding allowlist regex below AND update the
# matching section of `docs/invariants.md` in the same PR.

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
    raw=$(rg -n --hidden --glob '!target/**' --glob '!.git/**' --type rust "$pattern" . || true)
    if [[ -z "$raw" ]]; then
        return 0
    fi

    while IFS= read -r line; do
        # Strip line/column prefix to get the path.
        local path="${line%%:*}"
        # Normalize Windows `rg` output (`.\crates\...`) so allowlists stay
        # POSIX-shaped and match the paths emitted in CI.
        path="${path//\\//}"
        path="${path#./}"
        if [[ "$path" =~ $allow_regex ]]; then
            continue
        fi
        report "$invariant" "$description" "$line"
    done <<<"$raw"
}

# §1/§3 — `.complete(` must only appear inside:
#   - the kernel itself (`aura-agent-kernel/`)
#   - the agent-side recording seams (kernel_gateway.rs, recording_stream.rs)
#   - the reasoner provider internals and their mocks (`aura-model/`)
#   - the automaton runtime (wraps its provider with KernelModelGateway before
#     handing it over)
#   - the memory subsystem, which only ever holds an Arc<KernelModelGateway>
#     (`aura-context-memory/`)
#   - the trace `ReplayProvider` (`aura-agent-trace/src/replay.rs`), a
#     provider decorator handed to `Kernel::new`, so its hand-off to the
#     live provider is still recorded by the kernel
#   - any *test* file (unit, integration, harness shims)
run_band "§1/§3" "direct ModelProvider::complete call outside the recording seam" \
    '\.complete\(' \
    '^(crates/agent/aura-agent-kernel/|crates/agent/aura-agent/src/kernel_gateway\.rs|crates/agent/aura-agent/src/recording_stream\.rs|crates/agent/aura-agent/src/agent_loop/|crates/agent/aura-agent/src/event_sequence_tests\.rs|crates/model/aura-model/|crates/engine/aura-engine-automaton/|crates/context/aura-context-memory/|crates/agent/aura-agent-trace/src/replay\.rs|.*/tests/|.*test.*\.rs|.*tests.*\.rs)'

# §2 — direct store append functions bypass the per-agent kernel's
# seq/context-hash pipeline unless they are inside:
#   - the kernel impl (`aura-agent-kernel/`)
#   - the store impl (`aura-store-db/`)
#   - the abstract per-agent `RecordLog` trait (`aura-store-record/`)
#   - tests
#
# Sanctioned non-kernel/store call-sites (documented as exceptions in
# docs/invariants.md):
#   * `crates/surface/aura-node/src/tool_permissions.rs` — HTTP-driven tool-
#     permissions append guarded by the per-agent scheduler claim (see
#     §12.a). Routed via `aura_agent_kernel::write_system_record`.
#   * `crates/engine/aura-engine-cloud/src/hub/store.rs` — the cloud ledger writer:
#     machine and `Placement` rows on ledger agents no kernel runs. Its
#     record mirror
#     goes through `WriteStore::append_replica_batch`.
# Agent-host fallback records (a root agent whose inbox is gone) also go
# through `aura_agent_kernel::write_system_record`, which calls
# `WriteStore::append_entry_direct` internally — `aura-engine/src/agents/`
# never names an `append_entry_*` symbol, so no allowlist entry is needed.
run_band "§2" "append_entry_* used outside the kernel / store crates / tests" \
    'append_entry_(atomic|dequeued|direct|entries_batch)' \
    '^(crates/agent/aura-agent-kernel/|crates/store/aura-store-db/|crates/store/aura-store-record/|crates/surface/aura-node/src/tool_permissions\.rs|crates/engine/aura-engine-cloud/src/hub/store\.rs|.*/tests/|.*test.*\.rs|.*tests.*\.rs)'

# §1 — raw `git` processes must live in a kernel-mediated executor or in a
# declared infrastructure exception.
#
# Permitted locations for `Command::new("git")`:
#
#   * `crates/exec/aura-exec-tools/src/git_tool/` — the `GitExecutor` and its shared
#     helpers (`git_commit_impl`, `git_push_impl`, `git_commit_push_impl`).
#     Every mutating `git` subprocess in the tree must funnel through here.
#   * `crates/agent/aura-agent/src/git.rs` — read-only helpers (`git log` for
#     unpushed-commit telemetry, and `read_only`, which runs only the
#     subcommands in its `READ_ONLY` list, for the REPO tab). Declared
#     exception in docs/invariants.md.
#   * `crates/surface/aura-app/src/isolation/mod.rs` — `git worktree add` /
#     `worktree remove` for the workspace copies `/compare` runs in.
#     Isolation infrastructure, not a mutating commit/push. Declared
#     exception in docs/invariants.md.
#   * `crates/surface/aura-app/src/isolation/changes/mod.rs` — local-only `diff` /
#     `apply` / `ls-files` / `add` / `write-tree` / `diff-index` / `init`
#     to carry uncommitted work into an isolated copy, diff it against
#     its baseline, and apply a chosen copy's patch back (`/compare`);
#     plus `rev-parse` / `diff-tree` / `read-tree` / `checkout-index` on a
#     scratch `GIT_INDEX_FILE` to snapshot a workspace before an escalated
#     subtask and restore it before the retry. No commit, fetch or push.
#     Declared exception in docs/invariants.md; its subcommands are pinned
#     by the check after this band.
#   * `crates/surface/aura-node/src/gateway/handlers/safe_workspace/git.rs` —
#     local-only worktree, imported-project baseline, checkpoint, restore, and
#     apply lifecycle for explicitly selected hosted Safe Workspaces. A closed
#     subcommand allowlist in that module rejects network Git operations.
#     Declared exception in docs/invariants.md.
#   * Cloud agents (declared exception in docs/invariants.md):
#     `crates/engine/aura-engine-cloud/src/delta.rs` reads the project
#     (`rev-parse`, `rev-list`, `diff`, `ls-files`, `config`), runs
#     `bundle create` of its commits for a machine (snapshots build
#     through it), and fetches an agent's branch from the bundle its
#     machine made into the project's branch of the same name,
#     fast-forward only;
#     `crates/engine/aura-engine-cloud/src/hub/pull.rs` runs `git apply --3way`
#     of a cloud agent's patch into its own project;
#     `crates/surface/aura-node/src/gateway/handlers/cloud/patch.rs` diffs the
#     machine's workspace for that patch, staging it in a scratch
#     `GIT_INDEX_FILE` (`add`, `write-tree`). No commit, push, or call to
#     a remote.
#   * `crates/exec/aura-exec-merge-queue/src/git.rs` — the merge queue
#     landing pushes in a worktree of its own: fetch into private refs,
#     rebase, non-forced push of the checked result to the target branch.
#     Closed subcommand allowlist; declared exception in docs/invariants.md.
#   * Test files.
run_band "§1" "Command::new(\"git\") outside the GitExecutor / declared exceptions" \
    'Command::new\("git"\)' \
    '^(crates/exec/aura-exec-tools/src/git_tool/|crates/exec/aura-exec-merge-queue/src/git\.rs|crates/agent/aura-agent/src/git\.rs|crates/surface/aura-app/src/isolation/mod\.rs|crates/surface/aura-app/src/isolation/changes/mod\.rs|crates/surface/aura-node/src/gateway/handlers/safe_workspace/git\.rs|crates/engine/aura-engine-cloud/src/delta\.rs|crates/engine/aura-engine-cloud/src/hub/pull\.rs|crates/surface/aura-node/src/gateway/handlers/cloud/patch\.rs|.*/tests/|.*test.*\.rs|.*tests.*\.rs)'

# §1 — the git subcommands `changes/mod.rs` runs outside its tests (the first
# literal of each `.arg(` / `.args([`) stay in the local-only set its row in
# docs/invariants.md lists.
changes_file='crates/surface/aura-app/src/isolation/changes/mod.rs'
changes_allowed='^(rev-parse|diff|apply|ls-files|init|add|write-tree|diff-index|diff-tree|read-tree|checkout-index)$'
changes_subcommands=$(sed '/^#\[cfg(test)\]/,$d' "$changes_file" \
    | rg -U -o -r '$1' '\.args?\(\[?\s*"([^"]+)"' || true)
if [[ -z "$changes_subcommands" ]]; then
    report "§1" "no git subcommands found to check in ${changes_file}" "$changes_file"
fi
while IFS= read -r subcommand; do
    [[ -z "$subcommand" ]] && continue
    if [[ ! "$subcommand" =~ $changes_allowed ]]; then
        report "§1" "git ${subcommand} is not in the changes.rs subcommand allowlist" "$changes_file"
    fi
done <<<"$changes_subcommands"

# §1 — the `gh` CLI runs only for the REPO tab's PR reads, in
# `crates/surface/aura-app/src/event_loop/repo/github.rs`, and only as
# `gh pr list` and `gh pr diff` (the first two literals of each `gh(&[`
# call there outside its tests). Declared exception in docs/invariants.md.
run_band "§1" "Command::new(\"gh\") outside the REPO tab's PR reads" \
    'Command::new\("gh"\)' \
    '^(crates/surface/aura-app/src/event_loop/repo/github\.rs|.*/tests/|.*test.*\.rs|.*tests.*\.rs)'
github_file='crates/surface/aura-app/src/event_loop/repo/github.rs'
gh_subcommands=$(sed '/^#\[cfg(test)\]/,$d' "$github_file" \
    | rg -U -o -r '$1 $2' '\bgh\(&\[\s*"([^"]+)",\s*"([^"]+)"' || true)
if [[ -z "$gh_subcommands" ]]; then
    report "§1" "no gh subcommands found to check in ${github_file}" "$github_file"
fi
while IFS= read -r subcommand; do
    [[ -z "$subcommand" ]] && continue
    if [[ ! "$subcommand" =~ ^pr\ (list|diff)$ ]]; then
        report "§1" "gh ${subcommand} is not a read-only PR subcommand" "$github_file"
    fi
done <<<"$gh_subcommands"

# §10 — non-kernel, non-store crates must bind to `Arc<dyn ReadStore>`.
#
# `Arc<dyn Store>` exposes the sealed `WriteStore` surface. It is only
# legitimate in:
#
#   * The kernel + store impl crates (`aura-agent-kernel`,
#     `aura-store-db`, `aura-store-record`).
#   * Test scaffolding (`mod tests`, `tests/`, `*_tests.rs`).
#   * A bounded set of production sites that must hand a store handle to a
#     per-agent `Kernel::new` (which still takes `Arc<dyn Store>`). Each is
#     listed explicitly below. Once the kernel accepts a
#     `(ReadStore, WriteHook)` pair, this allowlist collapses to just the
#     kernel/store crates.
#
# Pattern also accepts `aura_store_db::Store` (the active impl crate's
# explicit path) so post-Phase-2 callsites stay covered.
#
# Production holders:
#   - crates/surface/aura-node/src/gateway/state.rs   — RouterState field piped into WsContext
#   - crates/surface/aura-node/src/gateway/session/mod.rs — WsContext handed to Kernel::new
#   - crates/surface/aura-node/src/gateway/session/cross_agent_hook/mod.rs — cross-agent chat/spawn hook
#   - crates/surface/aura-node/src/gateway/session/child_kernel.rs — session-scoped child-kernel
#                                                  factory captures the same store handle as
#                                                  WsContext so nested child runs route through
#                                                  AgentHost / Scheduler.
#   - crates/engine/aura-engine/src/scheduler/mod.rs    — Scheduler builds per-agent kernels (§12.a claim seam)
#   - crates/engine/aura-engine/src/automaton/          — AutomatonBridge builds per-agent automaton kernels
#                                                  (mod.rs + build.rs + dispatch.rs + event_channel.rs)
#   - crates/engine/aura-engine/src/agents/host/mod.rs  — AgentHost runs spawned agents through the
#                                                  scheduler and writes inbox fallback records
#   - crates/surface/aura-node/src/node/mod.rs        — boots the process-wide store and wires runtime surfaces
#   - crates/surface/aura-node/src/tool_permissions.rs — HTTP-driven tool-permissions
#                                                  append serialized via the §12.a claim
#   - crates/surface/aura-app/src/harness/               — interactive session host constructs a Kernel
#   - crates/surface/aura-app/src/session/mod.rs, event_loop/mod.rs, event_loop/cloud/mod.rs
#                                                — hand the main store to the cloud hub
#   - crates/engine/aura-engine-cloud/src/hub/         — the cloud hub's ledger writer and record mirror
#                                                (declared exception: cloud record mirror)
#
# Test-only holders (filenames that don't match `*test*.rs` but whose hits
# are inside `#[cfg(test)] mod tests`):
#   - crates/agent/aura-agent/src/kernel_gateway.rs
#   - crates/agent/aura-agent/src/kernel_domain_gateway/
#   - crates/agent/aura-agent/src/recording_stream.rs
#   - crates/engine/aura-engine/src/worker.rs
run_band "§10" "Arc<dyn Store> outside the kernel / store crates" \
    'Arc<dyn (aura_store(_db)?::)?Store>' \
    '^(crates/agent/aura-agent-kernel/|crates/store/aura-store-db/|crates/store/aura-store-record/|crates/surface/aura-node/src/gateway/state\.rs|crates/surface/aura-node/src/gateway/session/mod\.rs|crates/surface/aura-node/src/gateway/session/cross_agent_hook/mod\.rs|crates/surface/aura-node/src/gateway/session/child_kernel\.rs|crates/engine/aura-engine/src/scheduler/mod\.rs|crates/engine/aura-engine/src/automaton/|crates/engine/aura-engine/src/agents/host/mod\.rs|crates/engine/aura-engine/src/worker\.rs|crates/surface/aura-node/src/node/mod\.rs|crates/surface/aura-node/src/tool_permissions\.rs|crates/surface/aura-app/src/harness/|crates/surface/aura-app/src/session/mod\.rs|crates/surface/aura-app/src/event_loop/(mod|cloud/mod)\.rs|crates/engine/aura-engine-cloud/src/hub/|crates/agent/aura-agent/src/kernel_gateway\.rs|crates/agent/aura-agent/src/kernel_domain_gateway/|crates/agent/aura-agent/src/recording_stream\.rs|crates/agent/aura-agent/src/agent_loop/|.*/tests/|.*test.*\.rs|.*tests.*\.rs)'

# §9 — the agent loop must not reach into the store crates directly. Any
# code that needs persistence goes through the per-agent kernel. Test files
# in the same tree are exempt since they assemble scaffolding.
#
# The pattern catches `use aura_store_db::` and the legacy `use aura_store::`.
store_hits=$(rg -n --hidden --glob '!target/**' --glob '!.git/**' --type rust \
    --glob 'crates/agent/aura-agent/src/agent_loop/**' \
    --glob '!**/*test*.rs' --glob '!**/*tests*.rs' \
    'use aura_store(_db)?::' . || true)
if [[ -n "$store_hits" ]]; then
    while IFS= read -r line; do
        report "§9" "aura-agent/agent_loop must not depend on the store crates" "$line"
    done <<<"$store_hits"
fi

# Cloud agents, multi-user: raw user identities never reach boxd. The
# cloud engine sees users only as owner tags (`names::owner_tag`); a
# `user_id`, `primary_zid` or `display_name` read there could end up in a
# machine label, secret name, ledger row or log. Literal test values
# (`user_id: "..."`) and quoted JSON keys are not reads.
owner_hits=$(rg -n -P --glob '!target/**' --type rust \
    '(?<!")\b(user_id|primary_zid|display_name)\b(?!: ")' \
    crates/engine/aura-engine-cloud/src || true)
if [[ -n "$owner_hits" ]]; then
    while IFS= read -r line; do
        report "§cloud" "raw user identity in the cloud engine (use names::owner_tag)" "$line"
    done <<<"$owner_hits"
fi

if (( violations > 0 )); then
    echo ""
    echo "Invariant band check failed with ${violations} violation(s)." >&2
    echo "See docs/invariants.md and scripts/check_invariants.sh for the allowed call-sites." >&2
    exit 1
fi

echo "Invariant band check passed."
