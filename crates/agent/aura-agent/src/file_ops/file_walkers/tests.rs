use super::*;
use std::collections::HashSet;

fn make_temp_project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("main.rs"), "fn main() {}").unwrap();
    std::fs::write(dir.path().join("lib.rs"), "pub fn hello() {}").unwrap();
    std::fs::write(dir.path().join("readme.txt"), "not included").unwrap();
    std::fs::create_dir_all(dir.path().join(".git")).unwrap();
    std::fs::write(dir.path().join(".git").join("config"), "git stuff").unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src").join("utils.rs"), "pub fn util() {}").unwrap();
    std::fs::write(
        dir.path().join("src").join("helper.ts"),
        "export function helper() {}",
    )
    .unwrap();
    dir
}

#[test]
fn test_walk_and_collect_filtered_skips_gitdir() {
    let dir = make_temp_project();
    let mut output = String::new();
    let mut size = 0;
    let mut included = HashSet::new();
    walk_and_collect_filtered(
        dir.path(),
        dir.path(),
        &mut output,
        &mut size,
        100_000,
        &mut included,
    )
    .unwrap();
    assert!(
        !output.contains("git stuff"),
        ".git/ contents should be skipped"
    );
    assert!(!included.iter().any(|f| f.contains(".git")));
}

#[test]
fn test_walk_and_collect_filtered_respects_extensions() {
    let dir = make_temp_project();
    let mut output = String::new();
    let mut size = 0;
    let mut included = HashSet::new();
    walk_and_collect_filtered(
        dir.path(),
        dir.path(),
        &mut output,
        &mut size,
        100_000,
        &mut included,
    )
    .unwrap();
    assert!(output.contains("main.rs"), "should include .rs files");
    assert!(
        !output.contains("not included"),
        "should not include .txt files"
    );
}

#[test]
fn test_walk_and_collect_filtered_respects_size_limit() {
    let dir = make_temp_project();
    let mut output = String::new();
    let mut size = 0;
    let mut included = HashSet::new();
    walk_and_collect_filtered(
        dir.path(),
        dir.path(),
        &mut output,
        &mut size,
        50,
        &mut included,
    )
    .unwrap();
    assert!(
        output.len() <= 100,
        "output should be limited by max_bytes (with some tolerance for one section)"
    );
}

#[test]
fn test_walk_and_collect_filtered_dedup_via_included() {
    let dir = make_temp_project();
    let mut output = String::new();
    let mut size = 0;
    let mut included = HashSet::new();
    included.insert("main.rs".to_string());
    walk_and_collect_filtered(
        dir.path(),
        dir.path(),
        &mut output,
        &mut size,
        100_000,
        &mut included,
    )
    .unwrap();
    assert!(
        !output.contains("fn main()"),
        "pre-included files should be skipped"
    );
}

#[test]
fn test_format_file_or_signatures_short_file() {
    let content = "fn short() {}";
    let result = format_file_or_signatures("short.rs", content);
    assert!(
        result.contains("fn short() {}"),
        "short file should include full content"
    );
    assert!(result.contains("--- short.rs ---"));
    assert!(!result.contains("[signatures]"));
}

#[test]
fn test_format_file_or_signatures_long_file() {
    let content = (0..500)
        .map(|i| format!("fn func_{i}() {{ /* body */ }}\n"))
        .collect::<String>();
    assert!(content.len() > 8_000);
    let result = format_file_or_signatures("long.rs", &content);
    assert!(result.contains("long.rs"));
}

#[test]
fn test_collect_keyword_matching_files_finds_matches() {
    let dir = make_temp_project();
    let mut results = Vec::new();
    let included = HashSet::new();
    collect_keyword_matching_files(
        dir.path(),
        dir.path(),
        &["utils".to_string()],
        &mut results,
        &included,
    );
    let paths: Vec<&str> = results.iter().map(|(r, _)| r.as_str()).collect();
    assert!(
        paths.iter().any(|p| p.contains("utils")),
        "should find utils.rs"
    );
}

#[test]
fn test_collect_keyword_matching_files_skips_non_matching() {
    let dir = make_temp_project();
    let mut results = Vec::new();
    let included = HashSet::new();
    collect_keyword_matching_files(
        dir.path(),
        dir.path(),
        &["nonexistent".to_string()],
        &mut results,
        &included,
    );
    assert!(results.is_empty(), "no files should match 'nonexistent'");
}

#[test]
fn test_tiered_collector_budget_exhaustion() {
    let dir = make_temp_project();
    let tc = TieredCollector::new(dir.path(), 0);
    assert!(
        tc.budget_exhausted(),
        "zero budget should be immediately exhausted"
    );
    let tc2 = TieredCollector::new(dir.path(), 100_000);
    assert!(
        !tc2.budget_exhausted(),
        "large budget should not be exhausted"
    );
}
