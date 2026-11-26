# Test Coverage Gaps and Recommendations

**Date:** Phase 10 Review  
**Status:** 20 files reviewed (CLI surface, tests, CI, invariants)

## Overview

This document summarizes findings from a code review of the CLI, test suite, and CI pipeline. It identifies gaps in test coverage, configuration safety issues, and misalignments between documented rules and CI enforcement.

---

## Critical Issues (Fix Before Merge)

### 1. Config File Loss on Crash

**File:** `crates/surface/aura-cli/src/runner.rs` — `update_plugin_enable()`

**Risk:** Data loss. Atomic write is broken.

```rust
let tmp = config_path.with_extension("toml.tmp");
fs::write(&tmp, serialised)?;
if config_path.exists() {
    let _ = fs::remove_file(&config_path);  // ← Errors ignored
}
fs::rename(&tmp, &config_path)?;             // ← Crash here loses config
```

**What can go wrong:**
- Crash between `remove_file` and `rename` leaves no config.
- Two concurrent `aura plugins` commands collide on the same `*.tmp` filename.
- On Windows, the `remove_file` is necessary; on Unix, `rename` already overwrites. This should be `#[cfg(windows)]`.

**Fix:**
- Use `tempfile::NamedTempFile` with an auto-generated name and `persist()`.

---

### 2. Unsafe Shell Execution in Evals

**File:** `crates/surface/aura-cli/src/evals.rs` — `verify()` function

**Risk:** Unbounded blocking, platform dependency, path control.

```rust
pub fn verify(cmd: &str, dir: &Path, source: &str) -> anyhow::Result<(bool, Reward)> {
    std::process::Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .current_dir(dir)
        // ↑ No timeout
        // ↑ Hard-coded "sh" (fails on Windows)
        // ↑ dir comes from trace metadata (tampered runs control path)
        .output()?
```

**What can go wrong:**
- No timeout: blocks the async runtime indefinitely if verify hangs.
- Hard-coded `sh`: fails on Windows. The repo claims Windows support.
- Path comes from trace metadata: a user with run-write access can point a verify to any directory.

**Fix:**
- Apply timeout with `tokio::time::timeout()`.
- Use conditional compilation to pick `sh` vs `cmd.exe`.
- Validate that `dir` is under the workspace root.

---

### 3. Invalid Argument Values Not Caught

**File:** `crates/surface/aura-cli/src/cli.rs` — `--tolerance` and `--budget-usd`

**Risk:** Broken comparison logic.

```rust
#[arg(long, default_value_t = 0.1)]
pub tolerance: f64,                    // ← No validation; accepts NaN or negative

#[arg(long)]
pub budget_usd: Option<f64>,           // ← No validation
```

**What can go wrong:**
- `--tolerance=-1` or `--tolerance=NaN`: reverses regressions or breaks the comparison.
- `--budget-usd=NaN`: the check `spent >= budget` is always false (NaN comparisons are false), so the cap is ignored.

**Fix:**
- Use `value_parser` to validate at parse time.

---

### 4. Clippy Warnings Don't Fail CI (resolved)

**File:** `.github/workflows/ci.yml`

CI now runs `cargo clippy --workspace --all-targets --all-features -- -D warnings`,
as `.cursor/rules.md` §1.4 asks, so a clippy warning fails the build.

---

## High-Priority Issues (Test/Document)

### 5. Logout Partial Failure

**File:** `crates/surface/aura-cli/src/auth_commands.rs` — `cmd_logout()`

**Risk:** Silent partial unlink.

```rust
if args.all {
    for link in auth.links() {
        auth.unlink(link.provider).await?;  // ← Stops on first error
    }
    return logout_zos(&auth).await;
}
```

If `unlink` fails partway, the user is told "logged out" but some links remain.

**Fix:**
- Collect errors and report them all before returning.

---

### 6. Headless Tracing Filter Can Be Overridden

**File:** `crates/surface/aura-cli/src/runner.rs` — `run_headless()`

**Risk:** User's `RUST_LOG` is overridden.

```rust
let filter = if args.verbose {
    EnvFilter::from_default_env().add_directive("aura=debug".parse()?)
} else {
    EnvFilter::from_default_env().add_directive("aura=info".parse()?)
};
```

If the user set `RUST_LOG=aura=trace`, the `add_directive()` replaces it with `aura=debug`.

**Fix:**
- Only add a directive if `RUST_LOG` is not set.

---

### 7. Desktop Feature Error Message Is Misleading

**File:** `src/main.rs` and `crates/surface/aura-cli/src/runner.rs` — `run_cli()`

**Risk:** Wrong rebuild hint.

The root `Cargo.toml` has `features default = ["desktop"]`. If the user built with `--no-default-features`, they should drop that flag, not add `--features desktop`.

**Fix:**
- Tell the user to rebuild without `--no-default-features`.

---

## Test Coverage Gaps

### 8. No Tests for Plugin Config Mutations

**File:** `crates/surface/aura-cli/src/runner.rs`

**Missing tests:**
- `cmd_plugin_enable` and `cmd_plugin_disable` modify `config.toml`.
- No integration test covers the full flow: install → enable → check config → disable.

**Add an integration test** that:
1. Create a temporary cache with a test plugin.
2. Call `cmd_plugin_enable(...)`.
3. Assert `[plugins.<id>].enabled = true` is in `config.toml`.
4. Call `cmd_plugin_enable` again; assert no crash.

---

### 9. No Unit Tests for Auth Commands

**File:** `crates/surface/aura-cli/src/auth_commands.rs`

**Missing:**
- `parse_provider("local")` rejects the local provider. Test that.
- `describe_link()` formats link status. Test all three `LinkStatus` variants.
- `owner_label()` formats the owner. Test both `Owner` variants.

---

### 10. No Tests for Baseline Comparison Logic

**File:** `crates/surface/aura-cli/src/evals.rs`

**Missing:**
- `Baseline::compare()` computes regressions. No unit test for the logic.
- `compare()` (non-baseline) also untested.
- `rate()` is a pure function; trivial to test.

---

### 11. Config Boundary Test Is Fragile

**File:** `tests/config_boundary.rs`

**Risk:** Text-based scanning misses changes.

The test uses grep-like checks: `contents.contains("std::env::var(\"AURA_HOME\")")`. This breaks if:
- The env-var name appears in a comment.
- It is split across lines.

**Improvement:**
- Use `syn` to parse Rust and check the AST for `env::var()` calls.

---

### 12. Invariant Check Uses Loose Grep Patterns

**File:** `scripts/check_invariants.sh`

**Risk:** False negatives.

```bash
run_band "§1" "Command::new(\"git\")" ... '^(crates/exec/...|.*test.*\.rs|...)'
```

The exemption `.*test.*\.rs` matches any file with `test` in the name, so a production file named `latest_state_check.rs` would be exempt.

**Fix:**
- Use more specific patterns: `(tests/|_test\.rs$|_tests\.rs$)`.

---

## CI Improvements

### 13. Test Job Doesn't Use MSRV

**File:** `.github/workflows/ci.yml`

**Risk:** Tests pass on stable but might fail on MSRV.

The `msrv` job checks compilation but not tests at MSRV. If a test uses a feature new in Rust 1.86, it would not be caught.

**Add:**
```yaml
- uses: dtolnay/rust-toolchain@1.85.0
- name: cargo test (MSRV)
  run: cargo test --workspace --all-features --locked
```

---

### 14. MSRV Test Excludes Desktop but Not Fully

**File:** `.github/workflows/ci.yml`

The check should explicitly verify that `aura --no-default-features` works end-to-end by building and running it, not just compiling.

---

## Summary Table

| Issue | Severity | Type | File |
|-------|----------|------|------|
| Config loss on crash | Critical | Safety | runner.rs |
| Unbounded shell execution | Critical | Timeout/Path | evals.rs |
| Invalid args (tolerance, budget) | Critical | Logic | cli.rs |
| Clippy warnings don't fail CI (resolved) | Critical | CI | ci.yml |
| Logout partial failure | High | Error handling | auth_commands.rs |
| Tracing filter override | High | UX | runner.rs |
| Desktop error message | Medium | UX | runner.rs |
| Plugin config tests | Medium | Test gap | runner.rs |
| Auth command tests | Medium | Test gap | auth_commands.rs |
| Baseline tests | Medium | Test gap | evals.rs |
| Config boundary fragile | Medium | Test fragility | config_boundary.rs |
| Invariant grep patterns | Medium | Precision | check_invariants.sh |
| Test at MSRV | Low | Coverage | ci.yml |
| MSRV desktop check | Low | Coverage | ci.yml |

---

## Next Steps

1. **Fix critical issues** (1–4) before merging.
2. **Add unit tests** for auth, baseline, and plugin config logic.
3. **Fix CI** to test at MSRV.
4. **Review `aura-exec-tools`** for path traversal and timeout enforcement (§1 invariants).
5. **Review `aura-agent-kernel`** for the commit/seq/context-hash pipeline.

