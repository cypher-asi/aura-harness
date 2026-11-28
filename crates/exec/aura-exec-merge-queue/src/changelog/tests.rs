use super::*;
use std::collections::HashMap;

const LOG: &str = "# Changelog\n\nIntro.\n\n## [Unreleased]\n\n### Older change\n\n##### Fixed\n\n- old\n";

fn fragment(name: &str, text: &str) -> Fragment {
    Fragment {
        name: name.to_string(),
        text: text.to_string(),
    }
}

#[test]
fn fragments_go_at_the_top_of_unreleased_in_the_order_given() {
    let fragments = [
        fragment("b.md", "### Newest\n\n##### Added\n\n- b\n"),
        fragment("a.md", "\n### Second\n\n##### Fixed\n\n- a\n\n"),
    ];
    let assembled = assemble(LOG, &fragments).unwrap();
    assert_eq!(
        assembled,
        "# Changelog\n\nIntro.\n\n## [Unreleased]\n\n\
         ### Newest\n\n##### Added\n\n- b\n\n\
         ### Second\n\n##### Fixed\n\n- a\n\n\
         ### Older change\n\n##### Fixed\n\n- old\n"
    );
}

#[test]
fn an_empty_unreleased_section_takes_the_fragments() {
    let log = "# Changelog\n\n## [Unreleased]\n";
    let assembled = assemble(log, &[fragment("a.md", "### Only\n\n- a")]).unwrap();
    assert_eq!(assembled, "# Changelog\n\n## [Unreleased]\n\n### Only\n\n- a\n");
}

#[test]
fn no_fragments_leave_the_changelog_as_it_is() {
    assert_eq!(assemble(LOG, &[]).unwrap(), LOG);
}

#[test]
fn assembly_fails_on_what_it_cannot_place() {
    let good = [fragment("a.md", "### A\n")];
    assert!(matches!(
        assemble("# Changelog\n\n## [1.0.0]\n", &good),
        Err(ChangelogError::NoUnreleased)
    ));
    assert!(matches!(
        assemble(LOG, &[fragment("e.md", " \n\n")]),
        Err(ChangelogError::Empty(name)) if name == "e.md"
    ));
    assert!(matches!(
        assemble(LOG, &[fragment("t.md", "- no title\n")]),
        Err(ChangelogError::NoTitle(name)) if name == "t.md"
    ));
}

#[test]
fn the_folder_is_read_by_name_without_its_readme() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("b.md"), "### B\n").unwrap();
    std::fs::write(dir.path().join("a.md"), "### A\n").unwrap();
    std::fs::write(dir.path().join(README), "How to write one\n").unwrap();
    std::fs::write(dir.path().join("notes.txt"), "not one\n").unwrap();
    let names: Vec<String> = Fragment::read_dir(dir.path())
        .unwrap()
        .into_iter()
        .map(|fragment| fragment.name)
        .collect();
    assert_eq!(names, ["a.md", "b.md"]);
    assert!(Fragment::read_dir(&dir.path().join("missing"))
        .unwrap()
        .is_empty());
}

#[test]
fn newest_first_puts_uncommitted_fragments_on_top() {
    let fragments = vec![
        fragment("old.md", "### Old"),
        fragment("new.md", "### New"),
        fragment("local.md", "### Local"),
    ];
    let added = HashMap::from([("old.md".to_string(), 100), ("new.md".to_string(), 200)]);
    let names: Vec<String> = newest_first(fragments, &added)
        .into_iter()
        .map(|fragment| fragment.name)
        .collect();
    assert_eq!(names, ["local.md", "new.md", "old.md"]);
}

#[test]
fn a_branch_names_its_fragment() {
    for (branch, name) in [
        ("aura/fewer-merge-conflicts-rus204", "fewer-merge-conflicts-rus204.md"),
        ("fix x", "fix-x.md"),
        ("feature/a.b_c", "a.b_c.md"),
        ("///", "change.md"),
    ] {
        assert_eq!(fragment_name(branch), name, "{branch}");
    }
}

#[test]
fn fragment_paths_are_markdown_files_in_the_folder() {
    assert!(is_fragment_path("changelog.d/fix.md"));
    assert!(!is_fragment_path("changelog.d/README.md"));
    assert!(!is_fragment_path("changelog.d/sub/fix.md"));
    assert!(!is_fragment_path("CHANGELOG.md"));
    assert!(!is_fragment_path("docs/changelog.d/fix.md"));
}
