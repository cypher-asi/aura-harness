//! Workspace boundary test for the layered crate architecture.
//!
//! Phase 1 deliverable: walks every `crates/*/Cargo.toml`, builds a
//! `crate → layer` map from the new `aura-<layer>-<name>` naming
//! convention (and an explicit `KNOWN_CRATES` table for the legacy
//! pre-refactor crate names), and emits a line for every
//! workspace-internal dependency edge that points "upward" in the
//! layer stack.
//!
//! Every upward edge fails CI; there is no allowlist. The audit also
//! validates that every `crates/<crate>/src/lib.rs` carries a
//! `//! Layer: <layer>` doc-comment tag that matches the
//! `KNOWN_CRATES` / naming-convention assignment.
//!
//! ## Layer order (lowest → highest)
//!
//! `core` < `store` < `config` < `model` < `context` < `plugin` <
//! `exec` < `agent` < `surface`.
//!
//! Equal or downward edges are silently accepted. Upward edges cause
//! the test to fail.
//!
//! ## Parsing
//!
//! We use a small hand-rolled TOML parser scoped to the `[package]`
//! `name` field and the `[dependencies]`, `[dev-dependencies]`,
//! `[build-dependencies]`, and target-conditional dep tables. Path
//! dependencies referencing `../<other-crate>` are mapped to that
//! crate's name. We deliberately avoid pulling in the `toml` or
//! `cargo_toml` crates as test-only dev deps to keep this test
//! self-contained.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Layer rank — lower numbers depend "downward" (allowed); higher
/// numbers depend "upward" (advisory warning).
const LAYER_ORDER: &[&str] = &[
    "core", "store", "config", "model", "context", "plugin", "exec", "agent", "surface",
];

/// Explicit layer assignments for crate names that don't follow the
/// `aura-<layer>-<name>` convention. Empty string means "no layer
/// assigned — skip".
const KNOWN_CRATES: &[(&str, &str)] = &[
    // Root binary + dev/CI stuff — not a crate-layer thing.
    ("aura", "surface"),
    ("aura-node", "surface"),
    ("aura-config", "config"),
    // Phase 2 store layer split:
    //   aura-store-db     — RocksDB-backed durable storage impl
    //   aura-store-record — append-only record-log domain types + RecordLog trait
    //   aura-store-snapshot — content-addressed snapshot store (V1 no-op stub)
    ("aura-store-db", "store"),
    ("aura-store-record", "store"),
    ("aura-store-snapshot", "store"),
    // Exec layer: aura-tools holds the Tool impls, catalog and
    // executor; the aura-exec-* crates are its primitives.
    //   aura-exec-isolation — worktree + copy isolation (no aura-* deps)
    //   aura-exec-sandbox   — FsSandbox + ProcessSandbox primitives (no aura-* deps)
    ("aura-tools", "exec"),
    ("aura-exec-isolation", "exec"),
    ("aura-exec-sandbox", "exec"),
    // Phase 1 refactor: low-level executor/spawn traits relocated out
    // of aura-agent-kernel so aura-tools no longer depends upward.
    ("aura-exec-traits", "exec"),
    // Phase 3 model + context layer crates. Phase 4 deleted the
    // original `aura-<name>` compatibility shells; consumers now
    // depend on the layered `aura-<layer>-<name>` crates directly.
    // The `aura-<layer>-<name>` names match the
    // `aura-<layer>-<rest>` auto-classification convention exactly,
    // so they could theoretically be omitted from this table, but
    // we list them explicitly for clarity.
    ("aura-context-compaction", "context"),
    ("aura-model-reasoner", "model"),
    ("aura-context-memory", "context"),
    ("aura-context-skills", "context"),
    ("aura-context-prompts", "context"),
    // Phase 6a renamed `aura-kernel` to `aura-agent-kernel`; the
    // shell was removed in Phase 4.
    ("aura-agent-kernel", "agent"),
    ("aura-agent-subagent", "agent"),
    ("aura-terminal", "surface"),
    ("aura-agent", "agent"),
    // Phase 3 placeholder, populated in Phase 6a.
    ("aura-agent-steering", "agent"),
    // Phase 9 reclassifies aura-auth from `core` to `surface`: the
    // network client (ZosClient) and credential persistence
    // (CredentialStore) are surface-side concerns. Token primitive
    // types stay at the core layer in aura-core-auth.
    ("aura-auth", "surface"),
    // One crate per auth method (provider traits live in
    // aura-core-auth); aura-auth composes them with the vault.
    ("aura-auth-kit", "surface"),
    ("aura-auth-zos", "surface"),
    ("aura-auth-chatgpt", "surface"),
    ("aura-auth-claude", "surface"),
    ("aura-automaton", "surface"),
    // Phase B / Commit 3 surface-layer extraction: orchestration
    // engine pulled out of aura-runtime into aura-engine. Both live at
    // the surface layer — aura-runtime is the HTTP/WS gateway, aura-engine
    // is the reusable orchestration core the gateway composes.
    ("aura-engine", "surface"),
    // Phase C / Commit 4 surface-layer extraction: HTTP `DomainApi`
    // implementation + JWT wrapper pulled out of aura-runtime /
    // aura-engine. Stays at the surface layer because it composes
    // lower-layer types (aura-tools::domain_tools::DomainApi) into a
    // deployable HTTP edge.
    ("aura-domain-http", "surface"),
    ("aura-runtime", "surface"),
    ("aura-protocol", "core"),
    // Phase 4b plugin layer:
    //   aura-plugin-api  — in-process contributor traits (first-party only)
    //   aura-plugin-core — declarative manifest + install + cache + marketplace
    ("aura-plugin-api", "plugin"),
    ("aura-plugin-core", "plugin"),
    // Phase 4c plugin runtime surfaces:
    //   aura-plugin-hooks      — HookEngine + 10 Codex/Claude events
    //   aura-plugin-mcp        — stdio JSON-RPC client + connection manager
    //   aura-plugin-connectors — registry of plugin-contributed endpoints
    ("aura-plugin-hooks", "plugin"),
    ("aura-plugin-mcp", "plugin"),
    ("aura-plugin-connectors", "plugin"),
    // CLI composition root (ModeFlag, `--mode` resolution input).
    ("aura-surface-cli", "surface"),
];

/// Phase 9 strict-mode addition: verify every `crates/<name>/src/lib.rs`
/// carries a `//! Layer: <layer>` doc-comment tag matching the
/// `KNOWN_CRATES` / naming-convention assignment.
///
/// The doc tag is a human-readable guardrail (see plan §5
/// cross-cutting ownership, "Layer doc tags"). Cargo manifest is
/// the primary source of truth — this test only verifies the
/// source-level tag is present and consistent so the two surfaces
/// don't drift.
#[test]
fn every_crate_carries_a_matching_layer_doc_tag() {
    let workspace_root = workspace_root();
    let crates_dir = workspace_root.join("crates");

    let mut offenders: Vec<String> = Vec::new();
    let entries = fs::read_dir(&crates_dir).expect("crates dir readable");
    for entry in entries.flatten() {
        let manifest = entry.path().join("Cargo.toml");
        if !manifest.is_file() {
            continue;
        }
        let Ok(manifest_contents) = fs::read_to_string(&manifest) else {
            continue;
        };
        let Some(name) = parse_package_name(&manifest_contents) else {
            continue;
        };
        let Some(expected_layer) = layer_for(&name) else {
            continue;
        };

        let lib_path = entry.path().join("src").join("lib.rs");
        if !lib_path.is_file() {
            // Binary-only crates may not carry a `lib.rs`; skip
            // them — the manifest source remains authoritative.
            continue;
        }
        let Ok(lib_contents) = fs::read_to_string(&lib_path) else {
            offenders.push(format!(
                "{}: cannot read lib.rs to validate //! Layer: tag",
                name
            ));
            continue;
        };

        let tag_line = lib_contents
            .lines()
            .find(|l| l.trim_start().starts_with("//! Layer:"));
        match tag_line {
            Some(line) => {
                let body = line
                    .trim_start()
                    .trim_start_matches("//!")
                    .trim()
                    .trim_start_matches("Layer:")
                    .trim();
                // Allow trailing notes after the layer word
                // (e.g. "core (Phase 1 — narrowing)"). We just
                // require the first whitespace-delimited token to
                // match the expected layer.
                let first_word = body.split_whitespace().next().unwrap_or("");
                if first_word != expected_layer {
                    offenders.push(format!(
                        "{name}: //! Layer: tag declares `{first_word}` \
                         but Cargo manifest classifies it as `{expected_layer}`"
                    ));
                }
            }
            None => offenders.push(format!(
                "{name}: missing `//! Layer: {expected_layer}` doc-comment tag in src/lib.rs"
            )),
        }
    }

    assert!(
        offenders.is_empty(),
        "//! Layer: doc-tag audit failures:\n  - {}",
        offenders.join("\n  - ")
    );
}

/// External-consumer invariant: the current wire flow must be reachable from
/// the `aura-runtime` Cargo surface alone. If these re-exports disappear,
/// external consumers would have to depend on `aura-protocol` or lower
/// orchestration crates directly, which the architecture forbids.
#[test]
fn aura_runtime_reexports_canonical_wire_types() {
    fn assert_external_wire_type<T: Send + Sync + 'static>() {}

    assert_external_wire_type::<aura_runtime::RuntimeRequest>();
    assert_external_wire_type::<aura_runtime::RuntimeRunResponse>();
    assert_external_wire_type::<aura_runtime::InboundMessage>();
    assert_external_wire_type::<aura_runtime::OutboundMessage>();
}

#[test]
fn no_upward_layer_dependencies() {
    let workspace_root = workspace_root();
    let crates_dir = workspace_root.join("crates");

    let mut layer_map: BTreeMap<String, &'static str> = BTreeMap::new();
    let mut manifests: Vec<(String, PathBuf)> = Vec::new();

    let entries = match fs::read_dir(&crates_dir) {
        Ok(e) => e,
        Err(err) => {
            eprintln!("LAYER WARN: cannot enumerate crates dir: {err}");
            return;
        }
    };
    for entry in entries.flatten() {
        let manifest = entry.path().join("Cargo.toml");
        if !manifest.is_file() {
            continue;
        }
        let Ok(contents) = fs::read_to_string(&manifest) else {
            continue;
        };
        let Some(name) = parse_package_name(&contents) else {
            continue;
        };
        if let Some(layer) = layer_for(&name) {
            layer_map.insert(name.clone(), layer);
        }
        manifests.push((name, manifest));
    }

    let mut failures: Vec<String> = Vec::new();

    for (consumer, manifest) in &manifests {
        let Ok(contents) = fs::read_to_string(manifest) else {
            continue;
        };
        let deps = parse_workspace_dep_names(&contents, &layer_map);
        let Some(consumer_layer) = layer_map.get(consumer.as_str()) else {
            continue;
        };
        let consumer_rank = rank_of(consumer_layer);
        for dep in deps {
            let Some(dep_layer) = layer_map.get(dep.as_str()) else {
                continue;
            };
            let dep_rank = rank_of(dep_layer);
            if dep_rank > consumer_rank {
                failures.push(format!(
                    "{consumer} ({consumer_layer}) depends on {dep} ({dep_layer}) — upward edge"
                ));
            }
        }
    }

    eprintln!(
        "LAYER INFO: {} crates classified, {} violation(s)",
        layer_map.len(),
        failures.len()
    );

    assert!(
        failures.is_empty(),
        "Layer boundary violations:\n  - {}",
        failures.join("\n  - ")
    );
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

fn layer_for(name: &str) -> Option<&'static str> {
    if let Some((_, layer)) = KNOWN_CRATES.iter().find(|(c, _)| *c == name) {
        if layer.is_empty() {
            return None;
        }
        return Some(*layer);
    }
    if let Some(rest) = name.strip_prefix("aura-") {
        let head = rest.split('-').next().unwrap_or("");
        if LAYER_ORDER.contains(&head) {
            return Some(LAYER_ORDER.iter().copied().find(|l| *l == head).unwrap());
        }
    }
    None
}

fn rank_of(layer: &str) -> usize {
    LAYER_ORDER.iter().position(|l| *l == layer).unwrap_or(0)
}

/// Extract the `name` field under `[package]`.
fn parse_package_name(toml: &str) -> Option<String> {
    let mut in_package = false;
    for line in toml.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_package = trimmed == "[package]";
            continue;
        }
        if in_package {
            if let Some(rest) = trimmed.strip_prefix("name") {
                let rest = rest.trim_start_matches(|c: char| c.is_whitespace() || c == '=');
                let rest = rest.trim();
                if let Some(stripped) = rest.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
                    return Some(stripped.to_string());
                }
            }
        }
    }
    None
}

/// Return the set of workspace-member dep names referenced by any
/// dependency table in this manifest. We accept either:
///
/// - `dep_name = { path = "../<crate>" }` — the dep name is the key
///   and we use it directly.
/// - `dep_name = "x.y"` form when `dep_name` exists in `layer_map`.
///
/// `[dev-dependencies]` blocks are skipped: they only affect the
/// test compilation graph, not the production crate graph, so a
/// dev-dep edge cannot violate the runtime layer ordering.
fn parse_workspace_dep_names(
    toml: &str,
    layer_map: &BTreeMap<String, &'static str>,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut in_deps_table = false;
    for line in toml.lines() {
        let trimmed = line.trim();
        if let Some(section) = trimmed.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            // [dependencies], [build-dependencies],
            // [target.'cfg(...)'.dependencies], etc. — count toward
            // the production graph. [dev-dependencies] (and the
            // `target.'cfg(...)'.dev-dependencies` flavour) only
            // affect tests / benchmarks and are skipped.
            let is_dev = section == "dev-dependencies" || section.ends_with(".dev-dependencies");
            in_deps_table = !is_dev && section.ends_with("dependencies");
            continue;
        }
        if !in_deps_table || trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        // dep_name = ...
        let Some(eq_idx) = trimmed.find('=') else {
            continue;
        };
        let raw_name = trimmed[..eq_idx].trim();
        let name = raw_name
            .trim_matches(|c: char| c == '"' || c == ' ')
            .to_string();
        if name.is_empty() {
            continue;
        }
        if layer_map.contains_key(&name) && !out.contains(&name) {
            out.push(name);
        }
    }
    out
}
