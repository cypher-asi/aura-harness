//! `GET /v1/workspace/patch`: the agent's work past the hub's base.

use super::support::{call, claim, cloud_state_in, NODE_TOKEN};
use axum::http::StatusCode;

fn git_in(dir: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A committed workspace with an uncommitted hub edit marked as the base;
/// returns the base commit.
fn workspace_with_hub_base(ws: &std::path::Path) -> String {
    std::fs::create_dir(ws).expect("mkdir");
    git_in(ws, &["init", "-q"]);
    std::fs::write(ws.join("a.rs"), "one").expect("write");
    git_in(ws, &["add", "-A"]);
    git_in(
        ws,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-q",
            "-m",
            "c",
        ],
    );
    // What the hub sent: an edit, left uncommitted, marked as the base.
    std::fs::write(ws.join("a.rs"), "one, from the hub").expect("write");
    let tree = {
        git_in(ws, &["add", "-A"]);
        let tree = git_in(ws, &["write-tree"]);
        git_in(ws, &["reset", "-q"]);
        tree
    };
    let base = git_in(
        ws,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit-tree",
            &tree,
            "-p",
            "HEAD",
            "-m",
            "base",
        ],
    );
    git_in(
        ws,
        &["update-ref", aura_engine_cloud::workspace::BASE_REF, &base],
    );
    base
}

#[tokio::test]
async fn the_patch_holds_only_work_past_the_base_and_leaves_the_index_alone() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ws = dir.path().join("workspace");
    let base = workspace_with_hub_base(&ws);
    // The agent's work: an untracked file and a secret it must not send.
    std::fs::write(ws.join("new.rs"), "agent").expect("write");
    std::fs::write(ws.join(".env"), "TOKEN=1").expect("write");
    let index_before = std::fs::read(ws.join(".git/index")).expect("index");

    let state = cloud_state_in(dir.path(), Some(ws.clone()));
    claim(&state);
    let (status, body) = call(&state, "GET", "/v1/workspace/patch", Some(NODE_TOKEN), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["files"], serde_json::json!(["new.rs"]), "{body}");
    assert_eq!(body["base_commit"], base.as_str());
    let patch = body["patch"].as_str().unwrap_or_default();
    assert!(
        !patch.contains("from the hub") && !patch.contains("TOKEN"),
        "{patch}"
    );
    let tree = body["tree"].as_str().expect("tree").to_string();
    assert_eq!(git_in(&ws, &["cat-file", "-t", &tree]), "tree");
    assert_eq!(
        std::fs::read(ws.join(".git/index")).expect("index"),
        index_before,
        "the agent's index is untouched"
    );
    assert!(
        std::fs::read_dir(ws.join(".git"))
            .expect("git dir")
            .filter_map(Result::ok)
            .all(|e| !e.file_name().to_string_lossy().starts_with("aura-patch-")),
        "the scratch index is gone"
    );
}
