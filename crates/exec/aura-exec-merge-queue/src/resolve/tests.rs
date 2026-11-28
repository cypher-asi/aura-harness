use super::*;

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| (*item).to_string()).collect()
}

#[test]
fn generated_files_are_known_by_name_at_any_depth() {
    for (path, kind) in [
        ("CHANGELOG.md", Some(Generated::Changelog)),
        ("crates/x/CHANGELOG.md", Some(Generated::Changelog)),
        ("Cargo.lock", Some(Generated::CargoLock)),
        ("web/package-lock.json", Some(Generated::NpmLock)),
        ("Cargo.toml", None),
        ("src/CHANGELOG.md.rs", None),
        ("changelog.md", None),
    ] {
        assert_eq!(Generated::of(path), kind, "{path}");
    }
}

#[test]
fn conflicted_files_split_into_real_and_generated() {
    let (real, generated) = split(&strings(&["src/a.rs", "CHANGELOG.md", "Cargo.lock", "b.rs"]));
    assert_eq!(real, strings(&["src/a.rs", "b.rs"]));
    assert_eq!(generated, strings(&["CHANGELOG.md", "Cargo.lock"]));
}

#[test]
fn a_conflict_names_the_real_files_first() {
    let reason = conflict_reason(
        "main",
        "0123456789",
        &strings(&["a.rs", "b.rs"]),
        &strings(&["CHANGELOG.md"]),
        "",
    );
    assert!(
        reason.starts_with("conflicts with main at 01234567 in a.rs, b.rs;"),
        "{reason}"
    );
    assert!(
        reason.contains("CHANGELOG.md conflicted too; the queue resolves those itself"),
        "{reason}"
    );
    let plain = conflict_reason("main", "0123456789", &strings(&["a.rs"]), &[], "");
    assert!(!plain.contains("conflicted too"), "{plain}");
}

#[test]
fn a_merge_that_failed_without_conflicts_says_what_git_said() {
    let reason = conflict_reason("main", "0123456789", &[], &[], "warning\nfatal: no");
    assert_eq!(reason, "merging with main at 01234567 failed: fatal: no");
}

#[test]
fn the_temp_files_of_each_stage_are_read() {
    assert_eq!(
        temp_names(".merge_file_a .merge_file_b .merge_file_c\tCHANGELOG.md"),
        [
            Some(".merge_file_a".to_string()),
            Some(".merge_file_b".to_string()),
            Some(".merge_file_c".to_string()),
        ]
    );
    assert_eq!(
        temp_names(". .merge_file_b .merge_file_c\tCHANGELOG.md"),
        [
            None,
            Some(".merge_file_b".to_string()),
            Some(".merge_file_c".to_string()),
        ]
    );
    assert_eq!(
        temp_names(".merge_file_a .merge_file_b .\tCHANGELOG.md")[2],
        None
    );
}
