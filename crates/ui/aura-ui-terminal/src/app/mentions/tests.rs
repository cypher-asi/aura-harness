
use super::*;
use std::fs;

fn candidate(path: &str) -> MentionCandidate {
    MentionCandidate {
        path: path.to_string(),
        is_dir: path.ends_with('/'),
        member: None,
    }
}

fn ranked(paths: &[&str], query: &str) -> Vec<String> {
    let entries: Vec<_> = paths.iter().map(|p| candidate(p)).collect();
    rank(&entries, query).into_iter().map(|c| c.path).collect()
}

#[test]
fn prefix_matches_beat_substrings_and_subsequences() {
    let paths = [
        "docs/main_notes.md",
        "src/domain.rs",
        "src/main.rs",
        "src/app/remain.rs",
        "Makefile",
    ];
    assert_eq!(
        ranked(&paths, "main"),
        [
            "src/main.rs",
            "docs/main_notes.md",
            "src/domain.rs",
            "src/app/remain.rs",
        ]
    );
    assert_eq!(
        ranked(&paths, "src/"),
        ["src/main.rs", "src/domain.rs", "src/app/remain.rs"]
    );
    assert_eq!(
        ranked(&paths, "smr"),
        ["src/main.rs", "src/domain.rs", "src/app/remain.rs"]
    );
    assert_eq!(ranked(&paths, "MAKE"), ["Makefile"]);
    assert!(ranked(&paths, "zzz").is_empty());
}

#[test]
fn subsequence_span_prefers_tight_matches() {
    assert_eq!(subsequence_span("src/main.rs", "smr"), Some(10));
    assert_eq!(subsequence_span("a_b_ab", "ab"), Some(2));
    assert_eq!(subsequence_span("abc", "abc"), Some(3));
    assert_eq!(subsequence_span("abc", "cab"), None);
}

#[test]
fn index_skips_build_output_and_gitignored_names() {
    let dir = tempfile::tempdir().unwrap();
    for sub in ["src", "target/debug", ".git", "node_modules/x", "dist"] {
        fs::create_dir_all(dir.path().join(sub)).unwrap();
    }
    fs::write(dir.path().join("src/main.rs"), "").unwrap();
    fs::write(dir.path().join("target/debug/app"), "").unwrap();
    fs::write(dir.path().join("debug.log"), "").unwrap();
    fs::write(dir.path().join("my notes.txt"), "").unwrap();
    fs::write(dir.path().join(".gitignore"), "/dist/\n*.log\n").unwrap();

    let index = MentionIndex::build(dir.path());
    let paths: Vec<&str> = index.entries.iter().map(|c| c.path.as_str()).collect();
    assert_eq!(paths, ["src/", ".gitignore", "src/main.rs"]);
}

#[test]
fn matches_are_capped() {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..MAX_MATCHES + 10 {
        fs::write(dir.path().join(format!("f{i:03}")), "").unwrap();
    }
    let index = MentionIndex::build(dir.path());
    assert_eq!(index.entries.len(), MAX_MATCHES + 10);
    assert_eq!(index.rank("").len(), MAX_MATCHES);
    assert_eq!(index.rank("f0").len(), MAX_MATCHES);
    assert_eq!(index.rank("f0")[0].path, "f000");
}

#[test]
fn mention_word_needs_a_blank_before_the_at() {
    assert_eq!(mention_at("see @src/ma", 11), Some((4, 11, "src/ma")));
    assert_eq!(mention_at("@", 1), Some((0, 1, "")));
    assert_eq!(mention_at("mail me@host", 12), None);
    assert_eq!(mention_at("@src/main.rs now", 3), Some((0, 12, "sr")));
    assert_eq!(mention_at("@a b", 3), None);
}
