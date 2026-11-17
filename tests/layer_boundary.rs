//! Workspace boundary test for the layered crate architecture.
//!
//! Walks every `crates/<layer>/<crate>/Cargo.toml`, classifies each
//! crate by its `aura-<layer>[-<part>]` name (or the `KNOWN_CRATES`
//! table for the surface crates, which are named after what you run),
//! and fails on any workspace-internal dependency edge that points
//! "upward" in the layer stack. There is no allowlist.
//!
//! It also checks that every crate lives in the folder of its layer and
//! that its `src/lib.rs` carries a matching `//! Layer: <layer>` tag.
//!
//! ## Layer order (lowest → highest)
//!
//! `core` < `config` < `store` < `model` < `context` < `plugin` <
//! `exec` < `agent` < `auth` < `engine` < `ui` < `surface`.
//!
//! Equal or downward edges are accepted.
//!
//! ## Parsing
//!
//! We use a small hand-rolled TOML parser scoped to the `[package]`
//! `name` field and the `[dependencies]`, `[build-dependencies]`, and
//! target-conditional dep tables. We deliberately avoid pulling in the
//! `toml` or `cargo_toml` crates as test-only dev deps to keep this
//! test self-contained.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Layer rank, lowest first. A crate may depend on its own layer or any
/// layer before it.
const LAYER_ORDER: &[&str] = &[
    "core", "config", "store", "model", "context", "plugin", "exec", "agent", "auth", "engine",
    "ui", "surface",
];

/// Surface crates are named after the binary or host they provide, so
/// they don't carry an `aura-<layer>-` prefix.
const KNOWN_CRATES: &[(&str, &str)] = &[
    ("aura", "surface"),
    ("aura-app", "surface"),
    ("aura-cli", "surface"),
    ("aura-node", "surface"),
    ("aura-desktop", "surface"),
];

/// Every crate sits in `crates/<layer>/` for its own layer.
#[test]
fn every_crate_lives_in_its_layer_folder() {
    let mut offenders: Vec<String> = Vec::new();
    for (folder, dir) in crate_dirs() {
        let Some(name) = fs::read_to_string(dir.join("Cargo.toml"))
            .ok()
            .and_then(|contents| parse_package_name(&contents))
        else {
            continue;
        };
        match layer_for(&name) {
            Some(layer) if layer == folder => {}
            Some(layer) => offenders.push(format!(
                "{name} is in crates/{folder}/ but its layer is `{layer}`"
            )),
            None => offenders.push(format!(
                "{name} in crates/{folder}/ has no layer; name it aura-<layer>[-<part>]"
            )),
        }
    }
    assert!(
        offenders.is_empty(),
        "crate folder audit failures:\n  - {}",
        offenders.join("\n  - ")
    );
}

/// Every `src/lib.rs` carries a `//! Layer: <layer>` doc-comment tag
/// matching the crate's layer, so the source and the manifest don't
/// drift.
#[test]
fn every_crate_carries_a_matching_layer_doc_tag() {
    let mut offenders: Vec<String> = Vec::new();
    for (_, dir) in crate_dirs() {
        let Ok(manifest_contents) = fs::read_to_string(dir.join("Cargo.toml")) else {
            continue;
        };
        let Some(name) = parse_package_name(&manifest_contents) else {
            continue;
        };
        let Some(expected_layer) = layer_for(&name) else {
            continue;
        };

        let lib_path = dir.join("src").join("lib.rs");
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
/// the `aura-node` Cargo surface alone. If these re-exports disappear,
/// external consumers would have to depend on `aura-core-protocol` or lower
/// orchestration crates directly, which the architecture forbids.
#[test]
fn aura_node_reexports_canonical_wire_types() {
    fn assert_external_wire_type<T: Send + Sync + 'static>() {}

    assert_external_wire_type::<aura_node::RuntimeRequest>();
    assert_external_wire_type::<aura_node::RuntimeRunResponse>();
    assert_external_wire_type::<aura_node::InboundMessage>();
    assert_external_wire_type::<aura_node::OutboundMessage>();
}

#[test]
fn no_upward_layer_dependencies() {
    let mut layer_map: BTreeMap<String, &'static str> = BTreeMap::new();
    let mut manifests: Vec<(String, PathBuf)> = Vec::new();

    for (_, dir) in crate_dirs() {
        let manifest = dir.join("Cargo.toml");
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

/// `(layer folder, crate dir)` for every `crates/<layer>/<crate>/` that
/// has a `Cargo.toml`.
fn crate_dirs() -> Vec<(String, PathBuf)> {
    let crates_dir = workspace_root().join("crates");
    let mut out = Vec::new();
    for layer in fs::read_dir(&crates_dir)
        .expect("crates dir readable")
        .flatten()
    {
        let Ok(entries) = fs::read_dir(layer.path()) else {
            continue;
        };
        let folder = layer.file_name().to_string_lossy().into_owned();
        for entry in entries.flatten() {
            if entry.path().join("Cargo.toml").is_file() {
                out.push((folder.clone(), entry.path()));
            }
        }
    }
    assert!(!out.is_empty(), "no crates found under crates/<layer>/");
    out
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
