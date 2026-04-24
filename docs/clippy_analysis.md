# Clippy Analysis Report

**Date:** January 8, 2026  
**Rust Toolchain:** 1.92.0  
**Lint Configuration:** `clippy::all`, `clippy::pedantic`, `clippy::nursery`

This document captures all clippy warnings found after removing all `#![allow(...)]` suppressions from the codebase.

## Summary

| Module | Warnings | Status |
|--------|----------|--------|
| aura-store | 21 | ✅ Fixed |
| aura-reasoner | 25 | ✅ Fixed |
| aura-tools | 5 | ✅ Fixed |
| aura-kernel | 22 | ✅ Fixed |
| aura-runtime | 9 | ✅ Fixed |
| **Total** | **82** | **✅ All Fixed** |

**All warnings resolved and tests passing as of January 8, 2026.**

---

## aura-store (21 warnings)

### `doc_markdown` (14 instances)
Technical terms need backticks in documentation.

| File | Line | Term |
|------|------|------|
| `keys.rs` | 1 | `RocksDB` |
| `keys.rs` | 9 | `agent_id(32)` |
| `keys.rs` | 11 | `agent_id(32)` |
| `keys.rs` | 13 | `agent_id(32)`, `inbox_seq(u64be)` |
| `keys.rs` | 66 | `agent_id(32)` |
| `keys.rs` | 128 | `agent_id(32)` |
| `keys.rs` | 142 | `head_seq` |
| `keys.rs` | 148 | `inbox_head` |
| `keys.rs` | 154 | `inbox_tail` |
| `keys.rs` | 196 | `agent_id(32)`, `inbox_seq(u64be)` |
| `rocks_store.rs` | 1 | `RocksDB` |
| `rocks_store.rs` | 23 | `RocksDB` |
| `rocks_store.rs` | 61 | `sync_writes` |
| `store.rs` | 74 | `WriteBatch` |
| `store.rs` | 76 | `head_seq` |
| `store.rs` | 78 | `inbox_head` |
| `lib.rs` | 27 | `head_seq` |

### `single_match_else` (1 instance)
- `rocks_store.rs:133` - Use `if let` instead of single-arm match

### `derivable_impls` (1 instance)
- `store.rs:37` - `AgentStatus::default()` can use `#[derive(Default)]`

---

## aura-reasoner (25 warnings)

### `doc_markdown` (4 instances)
| File | Line | Term |
|------|------|------|
| `lib.rs` | 17 | `OpenAI` |
| `lib.rs` | 48 | `OpenAI` |
| `types.rs` | 36 | `tool_use` |
| `types.rs` | 402 | `max_tokens` |

### `missing_const_for_fn` (8 instances)
| File | Line | Function |
|------|------|----------|
| `mock.rs` | 141 | `with_latency` |
| `mock.rs` | 148 | `with_failure` |
| `mock.rs` | 255 | `MockReasoner::new` |
| `mock.rs` | 280 | `with_latency` |
| `mock.rs` | 287 | `with_failure` |
| `request.rs` | 67 | `with_limits` |
| `types.rs` | 118 | `json` |
| `types.rs` | 157 | `Message::new` |
| `types.rs` | 363 | `max_tokens` |
| `types.rs` | 370 | `temperature` |
| `types.rs` | 499 | `ModelResponse::new` |

### `uninlined_format_args` (4 instances)
| File | Line |
|------|------|
| `anthropic.rs` | 162 |
| `anthropic.rs` | 172 |
| `anthropic.rs` | 178 |
| `client.rs` | 63 |

### `missing_panics_doc` (2 instances)
| File | Line | Function |
|------|------|----------|
| `mock.rs` | 120 | `with_response` |
| `mock.rs` | 127 | `with_responses` |

### `map_unwrap_or` (1 instance)
- `anthropic.rs:146` - Use `map_or(0, f)` instead of `map(f).unwrap_or(0)`

### `cast_possible_truncation` (1 instance)
- `anthropic.rs:165` - `u128` to `u64` for latency

### `option_if_let_else` (1 instance)
- `client.rs:78` - Use `map_or` instead of match

### `derivable_impls` (1 instance)
- `types.rs:408` - `StopReason::default()` can use `#[derive(Default)]`

---

## aura-tools (5 warnings)

### `cast_possible_truncation` (2 instances)
| File | Line | Cast |
|------|------|------|
| `executor.rs` | 64 | `u64` to `usize` |
| `fs_tools.rs` | 56 | `u64` to `usize` |

### `missing_const_for_fn` (1 instance)
- `executor.rs:21` - `ToolExecutor::new`

### `map_unwrap_or` (1 instance)
- `executor.rs:62-65` - Use `map_or` instead

### `uninlined_format_args` (1 instance)
- `executor.rs:108`

---

## aura-kernel (22 warnings)

### `missing_panics_doc` (4 instances)
| File | Line | Function |
|------|------|----------|
| `policy.rs` | 180 | `is_session_approved` |
| `policy.rs` | 185 | `approve_for_session` |
| `policy.rs` | 193 | `revoke_session_approval` |
| `policy.rs` | 198 | `clear_session_approvals` |

### `uninlined_format_args` (4 instances)
| File | Line |
|------|------|
| `policy.rs` | 277 |
| `policy.rs` | 292 |
| `policy.rs` | 298 |
| `turn_processor.rs` | 378 |

### `missing_errors_doc` (2 instances)
| File | Line | Function |
|------|------|----------|
| `kernel.rs` | 91 | `process` |
| `turn_processor.rs` | 217 | `process_turn` |

### `cast_possible_truncation` (2 instances)
| File | Line | Cast |
|------|------|------|
| `kernel.rs` | 191 | `usize` to `u32` |
| `turn_processor.rs` | 333 | `usize` to `u32` |

### `doc_markdown` (2 instances)
| File | Line | Term |
|------|------|------|
| `policy.rs` | 132 | `AskOnce` |
| `turn_processor.rs` | 442 | `RecordEntry` |

### `needless_raw_string_hashes` (1 instance)
- `turn_processor.rs:92` - `r#"..."#` can be `r"..."`

### `field_reassign_with_default` (1 instance)
- `policy.rs:107-108` - Use struct init instead of reassign

### `match_same_arms` (1 instance)
- `policy.rs:209-210` - Merge identical match arms

### `missing_const_for_fn` (1 instance)
- `policy.rs:305` - `max_proposals`

### `unused_self` (1 instance)
- `turn_processor.rs:206` - `build_initial_messages` doesn't use `&self`

### `unnecessary_wraps` (1 instance)
- `turn_processor.rs:206` - Return doesn't need `Result` wrapper

### `too_many_lines` (1 instance)
- `turn_processor.rs:217` - `process_turn` has 102 lines (limit: 100)

### `used_underscore_binding` (1 instance)
- `turn_processor.rs:221` - `_next_seq` is used but underscore-prefixed

---

## aura-runtime (9 warnings)

### `doc_markdown` (3 instances)
| File | Line | Term |
|------|------|------|
| `config.rs` | 8 | `RocksDB` |
| `config.rs` | 12 | `RocksDB` |
| `config.rs` | 85 | `RocksDB` |

### `missing_const_for_fn` (3 instances)
| File | Line | Function |
|------|------|----------|
| `router.rs` | 200 | `default_from_seq` |
| `router.rs` | 204 | `default_limit` |
| `swarm.rs` | 24 | `Swarm::new` |

### `cast_possible_truncation` (1 instance)
- `router.rs:125` - `u128` to `u64` for timestamp

### `option_if_let_else` (1 instance)
- `scheduler.rs:91` - Use `map_or` instead

### `dead_code` (1 instance)
- `scheduler.rs:90` - `is_agent_busy` never used internally

---

## Fix Strategy

### Priority 1: Documentation fixes
- Add backticks to technical terms in doc comments
- Add `# Panics` sections where methods may panic
- Add `# Errors` sections for `Result`-returning functions

### Priority 2: Code style improvements
- Convert eligible functions to `const fn`
- Use `map_or` instead of `map().unwrap_or()`
- Inline format arguments (`{e}` instead of `{}, e`)
- Use `if let` instead of single-arm match

### Priority 3: Type safety
- Handle truncation casts with `try_from` or explicit bounds
- Use `#[derive(Default)]` where applicable

### Priority 4: Structural changes
- Merge identical match arms
- Remove unnecessary `Result` wrappers
- Refactor long functions
