use super::*;
use std::fs;

fn names(tree: &SandboxTree) -> Vec<String> {
    tree.rows()
        .iter()
        .map(|r| format!("{}{}", "  ".repeat(r.depth), r.name))
        .collect()
}

fn sample() -> (tempfile::TempDir, SandboxTree) {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src/nested")).unwrap();
    fs::write(dir.path().join("src/main.rs"), "").unwrap();
    fs::write(dir.path().join("src/nested/deep.rs"), "").unwrap();
    fs::create_dir(dir.path().join("Docs")).unwrap();
    fs::write(dir.path().join("README.md"), "").unwrap();
    fs::write(dir.path().join(".env"), "").unwrap();
    let mut tree = SandboxTree::default();
    tree.set_root(dir.path().to_path_buf());
    (dir, tree)
}

#[test]
fn lists_directories_first_case_insensitive() {
    let (_dir, tree) = sample();
    assert_eq!(names(&tree), ["Docs", "src", ".env", "README.md"]);
    assert_eq!(tree.rows()[0].kind, SandboxEntryKind::Dir);
    assert_eq!(tree.rows()[3].kind, SandboxEntryKind::File);
}

#[test]
fn expand_and_collapse() {
    let (_dir, mut tree) = sample();
    tree.move_by(1);
    tree.toggle_selected();
    assert_eq!(
        names(&tree),
        ["Docs", "src", "  nested", "  main.rs", ".env", "README.md"]
    );
    assert!(tree.rows()[1].expanded);

    tree.toggle_selected();
    assert_eq!(names(&tree), ["Docs", "src", ".env", "README.md"]);
}

#[test]
fn toggle_on_file_does_nothing() {
    let (_dir, mut tree) = sample();
    tree.select_last();
    tree.toggle_selected();
    assert_eq!(names(&tree).len(), 4);
}

#[test]
fn left_collapses_then_jumps_to_parent() {
    let (_dir, mut tree) = sample();
    tree.move_by(1);
    tree.toggle_selected();
    tree.move_by(1);
    tree.toggle_selected();
    tree.move_by(1);
    assert_eq!(tree.rows()[tree.selected()].name, "deep.rs");

    tree.collapse_or_parent();
    assert_eq!(tree.rows()[tree.selected()].name, "nested");
    tree.collapse_or_parent();
    assert!(!tree.rows()[tree.selected()].expanded);
    tree.collapse_or_parent();
    assert_eq!(tree.rows()[tree.selected()].name, "src");
}

#[test]
fn refresh_keeps_selection_and_picks_up_new_files() {
    let (dir, mut tree) = sample();
    tree.select_last();
    fs::write(dir.path().join("a.txt"), "").unwrap();
    tree.refresh();
    assert_eq!(tree.rows()[tree.selected()].name, "README.md");
    assert!(names(&tree).contains(&"a.txt".to_string()));
}

#[test]
fn new_root_resets_expansion() {
    let (_dir, mut tree) = sample();
    tree.move_by(1);
    tree.toggle_selected();
    let other = tempfile::tempdir().unwrap();
    tree.set_root(other.path().to_path_buf());
    assert!(tree.rows().is_empty());
    assert_eq!(tree.selected(), 0);
}

#[cfg(unix)]
#[test]
fn symlinked_directory_is_not_expanded() {
    let (dir, mut tree) = sample();
    std::os::unix::fs::symlink(dir.path().join("src"), dir.path().join("link")).unwrap();
    tree.refresh();
    let link = tree.rows().iter().position(|r| r.name == "link").unwrap();
    assert_eq!(tree.rows()[link].kind, SandboxEntryKind::Symlink);
    tree.move_by(isize::try_from(link).unwrap());
    tree.toggle_selected();
    assert!(!tree.rows()[link].expanded);
    assert_eq!(names(&tree).len(), 5);
}

fn entry(name: &str, is_dir: bool) -> RemoteEntry {
    RemoteEntry {
        name: name.to_string(),
        is_dir,
    }
}

#[test]
fn a_remote_tree_shows_what_it_is_sent_and_asks_for_what_it_opens() {
    let mut tree = SandboxTree::default();
    let root = PathBuf::from("/workspace");
    tree.set_remote_root(root.clone());
    assert!(tree.is_remote());
    assert_eq!(tree.unlisted(), Some(None));
    assert_eq!(tree.wanted(), std::slice::from_ref(&root));
    tree.asked(root.clone());
    assert!(tree.wanted().is_empty(), "asked about once");

    tree.set_listing(
        root.clone(),
        vec![
            entry("README.md", false),
            entry("src", true),
            entry("Docs", true),
        ],
    );
    assert_eq!(names(&tree), ["Docs", "src", "README.md"]);
    assert_eq!(tree.unlisted(), None);

    tree.move_by(1);
    tree.toggle_selected();
    let src = root.join("src");
    assert_eq!(tree.wanted(), std::slice::from_ref(&src));
    assert_eq!(names(&tree), ["Docs", "src", "README.md"]);
    tree.asked(src.clone());
    tree.set_listing(src.clone(), vec![entry("main.rs", false)]);
    assert_eq!(names(&tree), ["Docs", "src", "  main.rs", "README.md"]);
    assert_eq!(tree.rows()[2].path, src.join("main.rs"));

    tree.refresh();
    let mut again = tree.wanted();
    again.sort();
    assert_eq!(again, [root, src]);
    assert_eq!(
        names(&tree),
        ["Docs", "src", "  main.rs", "README.md"],
        "what shows stays until listed again"
    );
}

#[test]
fn a_remote_root_that_could_not_be_listed_says_why_until_asked_again() {
    let mut tree = SandboxTree::default();
    let root = PathBuf::from("/workspace");
    tree.set_remote_root(root.clone());
    tree.asked(root.clone());
    tree.listing_failed(root.clone(), "asleep".to_string());
    assert_eq!(tree.unlisted(), Some(Some("asleep")));
    assert!(tree.wanted().is_empty(), "not asked again on its own");

    tree.refresh();
    assert_eq!(tree.wanted(), std::slice::from_ref(&root));
    tree.set_listing(root, Vec::new());
    assert_eq!(tree.unlisted(), None);
}

#[test]
fn a_local_root_after_a_remote_one_reads_the_disk() {
    let (dir, mut tree) = sample();
    tree.set_remote_root(PathBuf::from("/workspace"));
    assert!(tree.rows().is_empty());
    tree.set_root(dir.path().to_path_buf());
    assert!(!tree.is_remote());
    assert_eq!(names(&tree), ["Docs", "src", ".env", "README.md"]);
    assert!(tree.wanted().is_empty());
}

#[test]
fn caps_large_directories() {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..MAX_ENTRIES_PER_DIR + 3 {
        fs::write(dir.path().join(format!("f{i:04}")), "").unwrap();
    }
    let mut tree = SandboxTree::default();
    tree.set_root(dir.path().to_path_buf());
    assert_eq!(tree.rows().len(), MAX_ENTRIES_PER_DIR + 1);
    assert_eq!(tree.rows().last().unwrap().kind, SandboxEntryKind::More(3));
}
