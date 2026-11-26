
use super::*;
use std::io::Read;

#[test]
fn env_files_and_build_dirs_are_denied() {
    for denied in [
        ".env",
        ".env.local",
        "sub/.env",
        "sub/.env.production",
        ".git/config",
        "target/debug/aura",
        "aura_data/db/LOG",
        "../outside",
        "/etc/passwd",
    ] {
        assert!(is_denied(Path::new(denied)), "{denied}");
    }
    for allowed in [
        ".env.example",
        "src/main.rs",
        "docs/env.md",
        "my.envrc",
        "a/target.rs",
    ] {
        assert!(!is_denied(Path::new(allowed)), "{allowed}");
    }
}

fn sh(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .status()
        .expect("git");
    assert!(status.success(), "git {args:?}");
}

fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    sh(dir.path(), &["init", "-q"]);
    sh(dir.path(), &["config", "user.email", "t@t"]);
    sh(dir.path(), &["config", "user.name", "t"]);
    std::fs::write(dir.path().join("a.rs"), "fn a() {}").expect("write");
    std::fs::write(dir.path().join("gone.rs"), "x").expect("write");
    sh(dir.path(), &["add", "-A"]);
    sh(dir.path(), &["commit", "-q", "-m", "base"]);
    dir
}

#[test]
fn the_delta_never_holds_env_files_even_without_gitignore() {
    let dir = repo();
    let base = head_commit(dir.path()).expect("head");
    std::fs::write(dir.path().join("a.rs"), "fn a() { 1; }").expect("write");
    std::fs::write(dir.path().join("new.rs"), "fn n() {}").expect("write");
    std::fs::write(dir.path().join(".env"), "BOXD_API_KEY=bxd_secret").expect("write");
    std::fs::write(dir.path().join(".env.local"), "X=1").expect("write");
    std::fs::remove_file(dir.path().join("gone.rs")).expect("rm");

    let delta = collect(dir.path(), &base).expect("delta");
    assert_eq!(
        delta.files,
        vec![PathBuf::from("a.rs"), PathBuf::from("new.rs")]
    );
    assert_eq!(delta.deleted, vec![PathBuf::from("gone.rs")]);

    let packed = pack(dir.path(), &delta).expect("pack");
    let mut text = Vec::new();
    flate2::read::GzDecoder::new(packed.as_slice())
        .read_to_end(&mut text)
        .expect("gunzip");
    let text = String::from_utf8_lossy(&text);
    assert!(!text.contains("bxd_secret"));
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(packed.as_slice()));
    let names: Vec<String> = archive
        .entries()
        .expect("entries")
        .map(|e| {
            e.expect("entry")
                .path()
                .expect("path")
                .display()
                .to_string()
        })
        .collect();
    assert_eq!(names, vec!["a.rs", "new.rs", ".aura-deleted"]);
}

#[test]
fn a_clean_tree_has_an_empty_delta() {
    let dir = repo();
    let base = head_commit(dir.path()).expect("head");
    let delta = collect(dir.path(), &base).expect("delta");
    assert!(delta.is_empty());
    assert!(!delta.wants_rebuild());
    assert!(has_commit(dir.path(), &base));
    assert!(!has_commit(
        dir.path(),
        "e91dfbe90eb7b51f7b0459ce0c829d439a150396"
    ));
}

#[test]
fn history_is_bundled_whole_then_by_what_is_new() {
    let dir = repo();
    let first = head_commit(dir.path()).expect("head");
    let history = bundle_head(dir.path()).expect("bundle");
    assert_eq!(history.commit, first);
    assert_eq!(history.upstream, None, "no origin here");
    assert!(history.bytes.starts_with(b"# v2 git bundle"));
    assert!(!format!("{history:?}").contains("git bundle"));
    assert_eq!(bundle_since(dir.path(), &first).expect("since"), None);

    std::fs::write(dir.path().join("b.rs"), "fn b() {}").expect("write");
    sh(dir.path(), &["add", "-A"]);
    sh(dir.path(), &["commit", "-q", "-m", "next"]);
    let newer = bundle_since(dir.path(), &first)
        .expect("since")
        .expect("one commit");
    assert!(newer.len() < history.bytes.len() + 4096);
    assert_eq!(origin_url(dir.path()), None);
}

#[test]
fn a_shallow_clone_is_refused_with_what_to_do() {
    let dir = repo();
    let shallow = tempfile::tempdir().expect("tempdir");
    sh(
        dir.path(),
        &["commit", "-q", "--allow-empty", "-m", "second"],
    );
    let status = Command::new("git")
        .args(["clone", "-q", "--depth", "1"])
        .arg(format!("file://{}", dir.path().display()))
        .arg(shallow.path())
        .status()
        .expect("git clone");
    assert!(status.success());
    let err = bundle_head(shallow.path()).expect_err("shallow");
    assert!(err.to_string().contains("git fetch --unshallow"), "{err}");
}
