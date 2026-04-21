//! Build/test fix attempt tracking and error reference extraction.

use std::sync::LazyLock;

use regex::Regex;

use crate::file_ops::ErrorReferences;

// INVARIANT: All patterns below are compile-time constants;
// `lazy_regex_compiles` in this file's test module forces every `LazyLock`
// so a broken pattern fails the test suite rather than at runtime.
static TYPE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"found for (?:struct|enum|trait|union) `(\w+)").expect("static regex")
});
static INIT_TYPE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"in initializer of `(?:\w+::)*(\w+)`").expect("static regex"));
static METHOD_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"no method named `(\w+)` found for (?:\w+ )?`(?:&(?:mut )?)?(\w+)")
        .expect("static regex")
});
static FIELD_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"missing field `(\w+)` in initializer of `(?:\w+::)*(\w+)`").expect("static regex")
});
static NO_FIELD_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"struct `(?:\w+::)*(\w+)` has no field named `(\w+)`").expect("static regex")
});
static LOC_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"-->\s*([\w\\/._-]+):(\d+):\d+").expect("static regex"));
static ARG_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"takes (\d+) arguments? but (\d+)").expect("static regex"));

/// Tracks a single build-fix attempt for retry history prompts.
pub struct BuildFixAttemptRecord {
    pub stderr: String,
    pub error_signature: String,
    pub files_changed: Vec<String>,
    pub changes_summary: String,
}

/// Extract type names, method references, field mismatches, and source
/// locations from compiler error output.
pub fn parse_error_references(stderr: &str) -> ErrorReferences {
    let mut refs = ErrorReferences::default();

    for cap in TYPE_RE.captures_iter(stderr) {
        let name = cap[1].to_string();
        if !refs.types_referenced.contains(&name) {
            refs.types_referenced.push(name);
        }
    }

    for cap in INIT_TYPE_RE.captures_iter(stderr) {
        let name = cap[1].to_string();
        if !refs.types_referenced.contains(&name) {
            refs.types_referenced.push(name);
        }
    }

    for cap in METHOD_RE.captures_iter(stderr) {
        let method = cap[1].to_string();
        let type_name = cap[2].to_string();
        refs.methods_not_found.push((type_name.clone(), method));
        if !refs.types_referenced.contains(&type_name) {
            refs.types_referenced.push(type_name);
        }
    }

    for cap in FIELD_RE.captures_iter(stderr) {
        let field = cap[1].to_string();
        let type_name = cap[2].to_string();
        refs.missing_fields.push((type_name.clone(), field));
        if !refs.types_referenced.contains(&type_name) {
            refs.types_referenced.push(type_name);
        }
    }

    for cap in NO_FIELD_RE.captures_iter(stderr) {
        let type_name = cap[1].to_string();
        let field = cap[2].to_string();
        refs.missing_fields.push((type_name.clone(), field));
        if !refs.types_referenced.contains(&type_name) {
            refs.types_referenced.push(type_name);
        }
    }

    for cap in LOC_RE.captures_iter(stderr) {
        let file = cap[1].to_string();
        let line: u32 = cap[2].parse().unwrap_or(0);
        if !refs
            .source_locations
            .iter()
            .any(|(f, l)| f == &file && *l == line)
        {
            refs.source_locations.push((file, line));
        }
    }

    for cap in ARG_RE.captures_iter(stderr) {
        refs.wrong_arg_counts
            .push(format!("expected {} got {}", &cap[1], &cap[2]));
    }

    refs
}

#[cfg(test)]
mod lazy_regex_guard {
    use super::{ARG_RE, FIELD_RE, INIT_TYPE_RE, LOC_RE, METHOD_RE, NO_FIELD_RE, TYPE_RE};

    #[test]
    fn lazy_regex_compiles() {
        let _ = &*TYPE_RE;
        let _ = &*INIT_TYPE_RE;
        let _ = &*METHOD_RE;
        let _ = &*FIELD_RE;
        let _ = &*NO_FIELD_RE;
        let _ = &*LOC_RE;
        let _ = &*ARG_RE;
    }
}
