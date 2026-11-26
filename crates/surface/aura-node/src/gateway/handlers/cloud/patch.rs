//! The workspace patch route; the only place this module runs `git`.

use super::*;

async fn git(dir: &std::path::Path, args: &[&str]) -> ApiResult<String> {
    git_with_index(dir, args, None).await
}

/// `git args` in `dir`, with `index` as the index file when given.
async fn git_with_index(
    dir: &std::path::Path,
    args: &[&str],
    index: Option<&std::path::Path>,
) -> ApiResult<String> {
    let mut command = tokio::process::Command::new("git");
    command.arg("-C").arg(dir).args(args);
    if let Some(index) = index {
        command.env("GIT_INDEX_FILE", index);
    }
    let output = tokio::time::timeout(GIT_TIMEOUT, command.output())
        .await
        .map_err(|_| {
            (
                StatusCode::GATEWAY_TIMEOUT,
                format!("git {} timed out", args.join(" ")),
            )
        })?
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("git: {e}")))?;
    if !output.status.success() {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!(
                "git {}: {}",
                args.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Paths a patch never carries.
const PATCH_EXCLUDES: &[&str] = &[
    ":(exclude,glob)**/.env",
    ":(exclude,glob)**/.env.*",
    ":(exclude,glob).env",
    ":(exclude,glob).env.*",
];

/// The working tree (tracked changes and the untracked files
/// `.gitignore` lets through, never `.env` files) written as a tree
/// object from a scratch copy of the index, so the agent's own index is
/// never touched.
async fn worktree_tree(dir: &std::path::Path) -> ApiResult<String> {
    let git_dir = git(dir, &["rev-parse", "--absolute-git-dir"]).await?;
    let git_dir = std::path::PathBuf::from(git_dir.trim());
    let scratch = git_dir.join(format!(
        "aura-patch-{}.index",
        uuid::Uuid::new_v4().simple()
    ));
    match tokio::fs::copy(git_dir.join("index"), &scratch).await {
        Ok(_) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("copy the index: {err}"),
            ))
        }
    }
    let mut add_args = vec!["add", "-A", "--", "."];
    add_args.extend(PATCH_EXCLUDES);
    let tree = match git_with_index(dir, &add_args, Some(&scratch)).await {
        Ok(_) => git_with_index(dir, &["write-tree"], Some(&scratch)).await,
        Err(err) => Err(err),
    };
    let _ = tokio::fs::remove_file(&scratch).await;
    Ok(tree?.trim().to_string())
}

/// The commit a patch is taken against: what the hub last had
/// ([`aura_engine_cloud::workspace::BASE_REF`]), else `HEAD`.
async fn patch_base(dir: &std::path::Path) -> ApiResult<String> {
    let base_ref = format!("{}^{{commit}}", aura_engine_cloud::workspace::BASE_REF);
    Ok(
        match git(dir, &["rev-parse", "-q", "--verify", &base_ref]).await {
            Ok(commit) => commit.trim().to_string(),
            Err(_) => git(dir, &["rev-parse", "HEAD"]).await?.trim().to_string(),
        },
    )
}

/// The paths that differ between `base` and `tree`.
async fn changed_files(dir: &std::path::Path, base: &str, tree: &str) -> ApiResult<Vec<String>> {
    let mut name_args = vec!["diff", "--name-only", base, tree, "--", "."];
    name_args.extend(PATCH_EXCLUDES);
    Ok(git(dir, &name_args)
        .await?
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect())
}

/// `GET /v1/workspace/patch`: the working tree's changes (tracked and
/// untracked) since what the hub last had of it
/// ([`aura_engine_cloud::workspace::BASE_REF`]; `HEAD` on a workspace
/// without one), and the tree they were taken from.
pub(in crate::gateway) async fn workspace_patch_handler(
    State(state): State<RouterState>,
) -> ApiResult<impl IntoResponse> {
    if !state.cloud.enabled() {
        return Err(not_cloud());
    }
    let dir = state
        .cloud
        .workspace()
        .ok_or_else(|| {
            (
                StatusCode::CONFLICT,
                "AURA_WORKSPACE is not set".to_string(),
            )
        })?
        .to_path_buf();
    let base = patch_base(&dir).await?;
    let tree = worktree_tree(&dir).await?;
    let mut diff_args = vec!["diff", "--binary", base.as_str(), tree.as_str(), "--", "."];
    diff_args.extend(PATCH_EXCLUDES);
    let patch = git(&dir, &diff_args).await?;
    if patch.len() > MAX_PATCH_BYTES {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("patch is {} bytes, above {MAX_PATCH_BYTES}", patch.len()),
        ));
    }
    let files = changed_files(&dir, &base, &tree).await?;
    Ok(Json(WorkspacePatch {
        base_commit: base,
        patch,
        files,
        tree: Some(tree),
    }))
}
