use super::*;
use std::fs;
use tempfile::TempDir;

/// A home directory with a project two levels below it.
struct Tree {
    _root: TempDir,
    home: PathBuf,
    repo: PathBuf,
}

fn tree() -> Tree {
    let root = TempDir::new().unwrap();
    let home = fs::canonicalize(root.path()).unwrap().join("home");
    let repo = home.join("code").join("repo");
    fs::create_dir_all(&repo).unwrap();
    Tree {
        _root: root,
        home,
        repo,
    }
}

fn write(path: &Path, content: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn settings(tree: &Tree, max_bytes: usize) -> InstructionSettings {
    InstructionSettings {
        enabled: true,
        user_file: Some(tree.home.join(".aura").join("AGENTS.md")),
        home: Some(tree.home.clone()),
        max_bytes,
    }
}

fn position(text: &str, needle: &str) -> usize {
    text.find(needle)
        .unwrap_or_else(|| panic!("{needle:?} missing from:\n{text}"))
}

#[test]
fn files_load_from_general_to_specific_and_stop_below_home() {
    let tree = tree();
    let rules = tree.repo.join(".cursor").join("rules");
    write(&tree.home.join(".aura").join("AGENTS.md"), "USER rule");
    write(&tree.home.join("AGENTS.md"), "HOME rule");
    write(&tree.home.join("code").join("AGENTS.md"), "PARENT rule");
    write(&tree.repo.join("AGENTS.md"), "REPO agents");
    write(&tree.repo.join("CLAUDE.md"), "REPO claude");
    write(&tree.repo.join(".cursor").join("rules.md"), "LEGACY rules");
    write(
        &rules.join("b-scoped.mdc"),
        "---\ndescription: Rust\nglobs: **/*.rs, *.toml\nalwaysApply: false\n---\nSCOPED rule\n",
    );
    write(
        &rules.join("a-always.mdc"),
        "---\nalwaysApply: true\n---\nALWAYS rule\n",
    );
    write(
        &rules.join("c-on-request.mdc"),
        "---\ndescription: Database migrations\nalwaysApply: false\n---\nMIGRATION rule\n",
    );
    write(
        &rules.join("d-manual.mdc"),
        "---\nalwaysApply: false\n---\nMANUAL rule\n",
    );
    write(&rules.join("notes.txt"), "NOT a rule");

    let loaded =
        ProjectInstructions::new(settings(&tree, DEFAULT_MAX_BYTES)).for_workspace(&tree.repo);
    let text = &loaded.text;

    let order = [
        "USER rule",
        "PARENT rule",
        "REPO agents",
        "REPO claude",
        "LEGACY rules",
        "ALWAYS rule",
        "SCOPED rule",
    ]
    .map(|needle| position(text, needle));
    assert!(
        order.windows(2).all(|w| w[0] < w[1]),
        "order {order:?}:\n{text}"
    );
    assert!(text.contains("globs=\"**/*.rs, *.toml\""), "{text}");
    assert!(
        text.contains("c-on-request.mdc: Database migrations"),
        "{text}"
    );
    for absent in ["HOME rule", "MIGRATION rule", "MANUAL rule", "NOT a rule"] {
        assert!(!text.contains(absent), "{absent:?} included:\n{text}");
    }

    let labels: Vec<&str> = loaded.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(labels[0], "~/.aura/AGENTS.md");
    assert_eq!(labels[1], "~/code/AGENTS.md");
    let on_request = loaded
        .files
        .iter()
        .find(|f| f.path.ends_with("c-on-request.mdc"))
        .unwrap();
    assert_eq!(on_request.status, InstructionStatus::OnRequest);
    let scoped = loaded
        .files
        .iter()
        .find(|f| f.path.ends_with("b-scoped.mdc"))
        .unwrap();
    assert_eq!(scoped.globs.as_deref(), Some("**/*.rs, *.toml"));
}

#[test]
fn nothing_found_renders_nothing() {
    let tree = tree();
    let loaded =
        ProjectInstructions::new(settings(&tree, DEFAULT_MAX_BYTES)).for_workspace(&tree.repo);
    assert!(loaded.text.is_empty());
    assert!(loaded.files.is_empty());
}

#[test]
fn disabled_settings_load_nothing() {
    let tree = tree();
    write(&tree.repo.join("AGENTS.md"), "rule");
    let loaded =
        ProjectInstructions::new(InstructionSettings::disabled()).for_workspace(&tree.repo);
    assert!(loaded.text.is_empty());
}

#[cfg(unix)]
#[test]
fn a_symlinked_claude_md_is_loaded_once() {
    let tree = tree();
    write(&tree.repo.join("AGENTS.md"), "SHARED rule");
    std::os::unix::fs::symlink("AGENTS.md", tree.repo.join("CLAUDE.md")).unwrap();

    let loaded =
        ProjectInstructions::new(settings(&tree, DEFAULT_MAX_BYTES)).for_workspace(&tree.repo);
    assert_eq!(loaded.text.matches("SHARED rule").count(), 1);
    assert_eq!(loaded.files.len(), 1);
}

#[test]
fn the_size_limit_cuts_the_most_general_files_first() {
    let tree = tree();
    let far = format!("FAR start\n{}", "far line\n".repeat(400));
    write(&tree.home.join("code").join("AGENTS.md"), &far);
    write(&tree.repo.join("AGENTS.md"), &"near line\n".repeat(50));

    let loaded = ProjectInstructions::new(settings(&tree, 2_000)).for_workspace(&tree.repo);
    assert!(loaded
        .text
        .contains(&"near line\n".repeat(50).trim_end().to_string()));
    assert!(loaded.text.contains("FAR start"));
    assert!(
        loaded.text.contains("[Truncated: only the first"),
        "{}",
        loaded.text
    );
    let far_view = &loaded.files[0];
    assert!(matches!(far_view.status, InstructionStatus::Truncated(n) if n == far.trim().len()));
    assert!(far_view.bytes <= 2_000 - 500);

    // With too little room left for a useful start, the file is listed
    // instead of cut.
    let loaded = ProjectInstructions::new(settings(&tree, 1_200)).for_workspace(&tree.repo);
    assert!(!loaded.text.contains("FAR start"));
    assert!(loaded.text.contains("size limit"));
    assert_eq!(loaded.files[0].status, InstructionStatus::Omitted);
}

#[test]
fn unchanged_files_give_the_same_bytes_and_changes_reload() {
    let tree = tree();
    let agents = tree.repo.join("AGENTS.md");
    write(&agents, "first version");
    let instructions = ProjectInstructions::new(settings(&tree, DEFAULT_MAX_BYTES));

    let first = instructions.for_workspace(&tree.repo);
    let again = instructions.for_workspace(&tree.repo);
    assert!(
        Arc::ptr_eq(&first, &again),
        "unchanged files are not re-read"
    );

    write(&agents, "second, longer version");
    let changed = instructions.for_workspace(&tree.repo);
    assert!(changed.text.contains("second, longer version"));

    write(&agents, "first version");
    let restored = instructions.for_workspace(&tree.repo);
    assert_eq!(
        restored.text, first.text,
        "same files render the same bytes"
    );

    write(&tree.repo.join("CLAUDE.md"), "new file");
    assert!(instructions
        .for_workspace(&tree.repo)
        .text
        .contains("new file"));
}

#[test]
fn rule_frontmatter_forms() {
    let globs = |frontmatter: &str| parse_rule_meta(frontmatter).globs;
    assert_eq!(globs("globs: **/*.rs"), ["**/*.rs"]);
    assert_eq!(globs("globs: \"*.ts\", '*.tsx'"), ["*.ts", "*.tsx"]);
    assert_eq!(globs("globs: [\"a/**\", b/*]"), ["a/**", "b/*"]);
    assert_eq!(
        globs("globs:\n  - \"*.py\"\n  - '*.pyi'\nalwaysApply: false"),
        ["*.py", "*.pyi"]
    );
    assert!(globs("globs:").is_empty());

    assert_eq!(rule_kind(None), Some(Kind::Always));
    assert_eq!(
        rule_kind(Some("alwaysApply: true\nglobs: *.rs")),
        Some(Kind::Always)
    );
    assert_eq!(rule_kind(Some("alwaysApply: false")), None);

    assert_eq!(
        split_frontmatter("---\na: b\n---\nbody"),
        (Some("a: b\n"), "body")
    );
    assert_eq!(
        split_frontmatter("no frontmatter"),
        (None, "no frontmatter")
    );
    assert_eq!(split_frontmatter("---\nunclosed"), (None, "---\nunclosed"));
}

#[test]
fn cuts_land_on_line_ends_and_character_boundaries() {
    assert_eq!(cut_point("aaaa\nbbbb\ncccc", 12), 9);
    assert_eq!(cut_point("ééééé", 3), 2);
    assert_eq!(cut_point("short", 100), 5);
}

#[test]
fn switch_values() {
    for off in ["0", "off", "FALSE", " no "] {
        assert!(is_off(off), "{off}");
    }
    for on in ["1", "on", "true", ""] {
        assert!(!is_off(on), "{on}");
    }
}
