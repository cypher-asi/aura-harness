//! No push leaves `run_command`: typed ones are refused, and one a script
//! makes can't reach a network remote. Pushes to the target are refused
//! like any other.

use super::*;

/// A context that runs any command, shell scripts included.
fn open_ctx() -> (ToolContext, TempDir) {
    tool_ctx(command_config(crate::CommandPolicy {
        enabled: true,
        allow_shell: true,
        bypass_allowlists: true,
        ..Default::default()
    }))
}

fn git(dir: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[tokio::test]
async fn a_typed_push_is_refused_and_points_at_git_ship() {
    let (ctx, _dir) = open_ctx();
    for input in [
        serde_json::json!({ "program": "git", "args": ["push", "origin", "main"] }),
        serde_json::json!({ "shell_script": "git push origin HEAD:main", "allow_shell": true }),
        serde_json::json!({ "shell_script": "git push -u origin aura/x", "allow_shell": true }),
    ] {
        let err = CmdRunTool
            .execute(&ctx, input.clone())
            .await
            .expect_err("refused");
        match err {
            ToolError::Forbidden(msg) => assert!(msg.contains("git_ship"), "{input}: {msg}"),
            other => panic!("{input}: expected Forbidden, got {other:?}"),
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn a_push_a_script_makes_never_reaches_the_remote() {
    if std::process::Command::new("git")
        .arg("--version")
        .output()
        .is_err()
    {
        return;
    }
    let (ctx, dir) = open_ctx();
    let repo = dir.path();
    git(repo, &["init", "-q", "-b", "aura/x"]);
    std::fs::write(repo.join("a.txt"), "a").unwrap();
    git(repo, &["add", "-A"]);
    git(repo, &["commit", "-qm", "a"]);
    git(
        repo,
        &["remote", "add", "origin", "https://github.com/acme/app.git"],
    );
    // The words of the command don't say `git push`; the script does.
    std::fs::write(
        repo.join("ship.sh"),
        "#!/bin/sh\ngit push origin HEAD:main\n",
    )
    .unwrap();
    let result = CmdRunTool
        .execute(
            &ctx,
            serde_json::json!({ "shell_script": "sh ship.sh", "allow_shell": true }),
        )
        .await
        .expect("it runs");
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(!result.ok, "the push failed: {said}");
    assert!(
        said.contains("aura-blocked-push"),
        "rewritten, never sent: {said}"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn commands_see_every_network_push_url_rewritten() {
    if std::process::Command::new("git")
        .arg("--version")
        .output()
        .is_err()
    {
        return;
    }
    let (ctx, _dir) = open_ctx();
    let result = CmdRunTool
        .execute(
            &ctx,
            serde_json::json!({ "shell_script": "git config --list", "allow_shell": true }),
        )
        .await
        .expect("it runs");
    let config = String::from_utf8_lossy(&result.stdout).to_lowercase();
    for scheme in ["https://", "http://", "ssh://", "git@"] {
        let line = format!("url.aura-blocked-push://use-git_ship/.pushinsteadof={scheme}");
        assert!(config.contains(&line), "{line} in:\n{config}");
    }
}
