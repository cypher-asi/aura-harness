---
name: Benchmark arm with a model per difficulty
overview: Add a third kind of benchmark arm, "by difficulty", where the user picks a model for easy, medium and hard work. Each model can come from any provider or subscription (e.g. easy on `openrouter/deepseek/...`, medium on a Claude plan, hard on a ChatGPT plan). Gate's classifier labels each prompt and subtask, and the arm's table picks the model.
todos:
- id: armspec
  content: Add ArmSpec::ByDifficulty with label/from_label/model_for and serde tests
  status: pending
- id: runnable
  content: Add router::runnable and ModelSelection::runnable_models
  status: pending
- id: resolve
  content: 'Resolve arm models across all providers; parse difficulty: arms; DifficultyRoutes::decide'
  status: pending
- id: prepare
  content: Wire routes into ComparePlan, estimate and arm labels in compare.rs/compare_view.rs
  status: pending
- id: run
  content: Route turns by difficulty in compare_run.rs via ArmRouting::ByDifficulty
  status: pending
- id: subtasks
  content: Classify and route subtasks by the table in subagents.rs/gate_subtask.rs
  status: pending
- id: form
  content: 'BENCH form: by-difficulty choice, per-difficulty rows, typed match, submit'
  status: pending
- id: render
  content: Render the new arm and its rows in renderer/benchmarks.rs
  status: pending
- id: tests
  content: Add tests across core-types, gate, aura-app and TUI; run fmt, clippy, test
  status: pending
- id: docs
  content: Update /compare USAGE and CHANGELOG
  status: pending
---

# Benchmark arm with a model per difficulty

Add a third kind of benchmark arm, "by difficulty", where the user picks a model for easy, medium and hard work. Each model can come from any provider or subscription (e.g. easy on `openrouter/deepseek/...`, medium on a Claude plan, hard on a ChatGPT plan). Gate's classifier labels each prompt and subtask, and the arm's table picks the model.

## Todos

- [ ] Add ArmSpec::ByDifficulty with label/from_label/model_for and serde tests
- [ ] Add router::runnable and ModelSelection::runnable_models
- [ ] Resolve arm models across all providers; parse difficulty: arms; DifficultyRoutes::decide
- [ ] Wire routes into ComparePlan, estimate and arm labels in compare.rs/compare_view.rs
- [ ] Route turns by difficulty in compare_run.rs via ArmRouting::ByDifficulty
- [ ] Classify and route subtasks by the table in subagents.rs/gate_subtask.rs
- [ ] BENCH form: by-difficulty choice, per-difficulty rows, typed match, submit
- [ ] Render the new arm and its rows in renderer/benchmarks.rs
- [ ] Add tests across core-types, gate, aura-app and TUI; run fmt, clippy, test
- [ ] Update /compare USAGE and CHANGELOG

<!-- end of generated header: edit below -->

## What exists today

- An arm is an `ArmSpec` (`crates/core/aura-core-types/src/types/compare.rs`): `Gate { delegate }` or `Model { model }`. It is stored in `ComparePayload.arm_a/arm_b`.
- The BENCH form (`crates/ui/aura-ui-terminal/src/app/benchmarks.rs`) sends each arm as a string (`gate`, `gate:nodelegate`, a model id) in `BenchSpec.a/b`. `compare::prepare` (`crates/surface/aura-app/src/event_loop/compare.rs`) resolves those strings with `aura_agent_gate::resolve_arms`.
- Every compare run already classifies its prompt (`run_prompt` in `compare_run.rs`), so each turn's difficulty is known. Subtasks are classified in `SubtaskGate::label` (`gate_subtask.rs`).
- **Gap:** `resolve_against` only matches against `gate_candidates()`. That list keeps one route per upstream model and drops OpenRouter models that have no curated price. So the form offers `openrouter/deepseek/...` but the estimate rejects it, and a pinned `openrouter/anthropic/...` fails when a Claude plan is linked. This has to be fixed before choosing models across providers can work.

## Approach

### 1. Arm type (`aura-core-types`)
Add a variant, serde tag `by_difficulty`. Old records still parse.
```rust
ArmSpec::ByDifficulty { easy: String, medium: String, hard: String }
```
- Label syntax, also used on the CLI: `difficulty:easy=<id>,medium=<id>,hard=<id>`. Model ids never contain `,` or `=`.
- `label()` returns `Cow<'_, str>`. `from_label` parses the prefix. `model_for(Option<Difficulty>) -> &str` falls back to the medium model when the difficulty is unrated.
- Add the constant `ArmSpec::BY_DIFFICULTY_PREFIX`.

### 2. Resolving models across providers (`aura-agent-gate`)
- `router::runnable(catalog, can_use, dropped) -> Vec<Candidate>`: every usable option, with each route kept and no curated-price filter. Add `ModelSelection::runnable_models()` in `model_selection.rs`.
- `compare::resolve_model(query, runnable, candidates)` tries these in order:
  1. an exact id or label in `runnable`
  2. the existing fuzzy match in `candidates`
  3. a fuzzy match in `runnable`
- `resolve_arm` and `resolve_arms` take `runnable` too. They parse the `difficulty:` prefix, need all three entries, and name the missing or unknown one in the error. Pinned `Model` arms use the same resolution, which fixes the gap above.
- `DifficultyRoutes { easy, medium, hard: Candidate }` with `decide(&Work) -> Decision`:
  - picks the model for the work's difficulty
  - sets the effort with `effort_on`, so a comparison with `gate` only measures the choice of model
  - uses `reason: RouteReason::Compare`, `explored: false`, `propensity: 1.0`

### 3. Running the arm (`aura-app`)
- `ComparePlan` gets `routes: [Option<DifficultyRoutes>; 2]`, filled in by `prepare`. The estimate prices the arm on its medium model. The confirmation reads `easy → deepseek-v3.2 (openrouter) · medium → … · hard → …`, through `compare_view::arm_label`.
- `ArmRouting::ByDifficulty(DifficultyRoutes)` in `subagents.rs`:
  - `run_turn` / `run_prompt`: `routes.decide(&work)`, then `model_limits().route(...)`. The route is recorded with the turn's category and difficulty, as pinned arms are now. There is no Gate follow-up, judge review or escalation (`gate_arm` stays false).
  - `subtask_gate`: a new `SubtaskGate.table` field. `label()` classifies the subtask and routes it by the table, with no `below_lead` and no exploring.
  - `leads()`: no enforced lead, the same as `Pinned`.
- `arm_name` gives `B · by difficulty`. `gate_log.rs` treats the arm as not routed by Gate.
- Add the `difficulty:` form to `/compare` `USAGE`.

### 4. BENCH form (`aura-ui-terminal`)
- `bench_arm_choices` adds `by difficulty` after `gate:nodelegate`.
- `BenchForm` gets `a_models` and `b_models: [String; 3]`. `FormField` adds `AEasy/AMedium/AHard` and `BEasy/BMedium/BHard`. These rows show, and `move_form_field` stops on them, only while their arm is by difficulty.
- Difficulty rows:
  - ←/→ cycle through every available model, from every provider (the same filter as today).
  - Typed text + Enter picks the first model whose id or label contains it, so `deepseek` finds it among hundreds of OpenRouter models. If nothing matches, the user gets a warning.
  - Backspace clears the row.
- `submit_bench_form` builds the `difficulty:` string. If any difficulty has no model, it warns and focuses that row.
- Renderer (`renderer/benchmarks.rs`): `arm_reads` shows `by difficulty · a model for each`. Each row shows the model id and its provider group from `ModelOptionView.group`.

## Decisions (easy to change)
- **No escalation** on this arm. It is a fixed table, so the results show what the table itself does. A later option could move a failed attempt up to the next difficulty's model.
- **Effort follows Gate's rules** for each difficulty, not each model's defaults.
- **All three difficulties are required**. Unrated work uses the medium model.

## Files to change

| File | Change |
|---|---|
| `crates/core/aura-core-types/src/types/compare.rs` | `ByDifficulty` variant, label, parsing, `model_for` |
| `crates/agent/aura-agent-gate/src/router.rs` | `runnable()` |
| `crates/agent/aura-agent-gate/src/compare.rs` | `resolve_model`, `difficulty:` parsing, `DifficultyRoutes`, `USAGE` |
| `crates/surface/aura-app/src/model_selection.rs` | `runnable_models()` |
| `crates/surface/aura-app/src/event_loop/compare.rs` | pass `runnable`, `routes`, estimate text |
| `crates/surface/aura-app/src/event_loop/compare_run.rs` | turn routing for the new arm, `arm_name` |
| `crates/surface/aura-app/src/event_loop/subagents.rs`, `gate_subtask.rs` | `ArmRouting::ByDifficulty`, routing subtasks by the table |
| `crates/surface/aura-app/src/event_loop/compare_view.rs`, `gate_log.rs` | labels, exhaustive matches |
| `crates/ui/aura-ui-terminal/src/app/benchmarks.rs` | form rows, choices, typed match, submit |
| `crates/ui/aura-ui-terminal/src/renderer/benchmarks.rs` | how the rows read |
| `CHANGELOG.md` | entry |

## Risks
- `label()` changing to `Cow` touches several call sites (`RunMeta::new`, `turn_summary`, `format!` uses). The compiler will find them all.
- The UI crate cannot depend on `aura-core-types`, so it formats the `difficulty:` string itself. An `aura-app` test checks that the string the form builds parses back into the same `ArmSpec`.
- A model with no price (some OpenRouter ids before the live list loads) shows as unpriced in the estimate, as pinned models already do.

## Tests
- core-types: serde round-trip of `ByDifficulty`, `label` / `from_label` round-trip, old payloads still parse.
- gate: `runnable` keeps every route and OpenRouter models with no curated price. `resolve_arm` with an exact `openrouter/deepseek/...` id, and errors for a missing or unknown entry. `DifficultyRoutes::decide` for each difficulty, `None` going to medium, and the effort.
- aura-app (`tests/compare_runs.rs`): `prepare` with a by-difficulty arm across Claude plan and OpenRouter models. An easy prompt runs on the easy model and a hard subtask on the hard model.
- TUI: the rows show only for a by-difficulty arm, a typed match picks the model, submit builds the string, and a missing model warns.
- Gates: `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --all --all-features`.
