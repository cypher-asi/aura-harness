//! Read-only Git inspection for an authenticated client of a pod workspace.
//! The caller selects a directory inside the configured file sandbox; Git
//! cannot walk to a repository whose root is outside that directory.

use super::super::*;
use crate::gateway::handlers::files::path_error_response;
use std::path::{Component, Path as FsPath, PathBuf};
use std::process::Stdio;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

const MAX_STATUS_FILES: usize = 2_000;
const MAX_GIT_OUTPUT_BYTES: usize = 2 * 1024 * 1024;
const MAX_DIFF_BYTES: usize = 512 * 1024;
const GIT_TIMEOUT: Duration = Duration::from_secs(8);
static GIT_SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(8);

#[derive(Deserialize)]
pub(in crate::gateway) struct GitStatusQuery {
    path: String,
}

#[derive(Deserialize)]
pub(in crate::gateway) struct GitDiffQuery {
    path: String,
    file: String,
    area: DiffArea,
}

#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum DiffArea {
    Staged,
    Worktree,
}

#[derive(Serialize)]
struct ChangedFile {
    path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    original_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    staged_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    worktree_status: Option<String>,
}

#[derive(Serialize)]
struct GitStatus {
    available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    unavailable_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    upstream: Option<String>,
    ahead: u32,
    behind: u32,
    files: Vec<ChangedFile>,
}

impl GitStatus {
    fn unavailable(reason: &str) -> Self {
        Self {
            available: false,
            unavailable_reason: Some(reason.into()),
            branch: None,
            upstream: None,
            ahead: 0,
            behind: 0,
            files: Vec::new(),
        }
    }
}

#[derive(Serialize)]
struct GitDiff {
    path: String,
    area: DiffArea,
    diff: String,
    truncated: bool,
    binary: bool,
}

async fn read_bounded<R: AsyncRead + Unpin>(reader: R, limit: usize) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader
        .take((limit + 1) as u64)
        .read_to_end(&mut bytes)
        .await?;
    Ok(bytes)
}

async fn git(cwd: &FsPath, args: &[&str]) -> Result<Vec<u8>, String> {
    let _permit = GIT_SLOTS
        .try_acquire()
        .map_err(|_| "Git inspection is busy".to_string())?;
    let mut command = Command::new("git");
    command
        .arg("-c")
        .arg("core.fsmonitor=false")
        .args(args)
        .current_dir(cwd)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_LITERAL_PATHSPECS", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command
        .spawn()
        .map_err(|_| "Git is unavailable".to_string())?;
    let stdout = child.stdout.take().ok_or("Git output unavailable")?;
    let stderr = child.stderr.take().ok_or("Git errors unavailable")?;
    tokio::time::timeout(GIT_TIMEOUT, async {
        let (output, errors) = tokio::try_join!(
            read_bounded(stdout, MAX_GIT_OUTPUT_BYTES),
            read_bounded(stderr, 8 * 1024),
        )
        .map_err(|_| "Git inspection failed".to_string())?;
        if output.len() > MAX_GIT_OUTPUT_BYTES || errors.len() > 8 * 1024 {
            let _ = child.kill().await;
            return Err("Git inspection exceeded its output limit".into());
        }
        let status = child.wait().await.map_err(|_| "Git inspection failed")?;
        if !status.success() {
            return Err("Git could not inspect this workspace".into());
        }
        Ok(output)
    })
    .await
    .map_err(|_| "Git inspection timed out".to_string())?
}

async fn repository_root(workspace: &FsPath) -> Result<PathBuf, String> {
    let output = git(workspace, &["rev-parse", "--show-toplevel"]).await?;
    let root = PathBuf::from(String::from_utf8_lossy(&output).trim());
    let root = root
        .canonicalize()
        .map_err(|_| "Git repository is unavailable".to_string())?;
    if !root.starts_with(workspace) {
        return Err("The Git repository is outside this workspace".into());
    }
    Ok(root)
}

async fn changed_files(root: &FsPath) -> Result<GitStatus, String> {
    let output = git(
        root,
        &[
            "status",
            "--porcelain=v2",
            "--branch",
            "-z",
            "--untracked-files=all",
            "--ignore-submodules=all",
        ],
    )
    .await?;
    Ok(parse_status(&output))
}

fn meaningful_status(value: char) -> Option<String> {
    (value != '.').then(|| value.to_string())
}

fn parse_status(output: &[u8]) -> GitStatus {
    let text = String::from_utf8_lossy(output);
    let mut status = GitStatus {
        available: true,
        unavailable_reason: None,
        branch: None,
        upstream: None,
        ahead: 0,
        behind: 0,
        files: Vec::new(),
    };
    let mut records = text.split('\0').peekable();
    while let Some(record) = records.next() {
        if let Some(value) = record.strip_prefix("# branch.head ") {
            status.branch = Some(value.into());
            continue;
        }
        if let Some(value) = record.strip_prefix("# branch.upstream ") {
            status.upstream = Some(value.into());
            continue;
        }
        if let Some(value) = record.strip_prefix("# branch.ab ") {
            for field in value.split_whitespace() {
                if let Some(value) = field.strip_prefix('+') {
                    status.ahead = value.parse().unwrap_or(0);
                } else if let Some(value) = field.strip_prefix('-') {
                    status.behind = value.parse().unwrap_or(0);
                }
            }
            continue;
        }
        let (xy, path, original_path) = if record.starts_with("1 ") {
            let fields: Vec<_> = record.splitn(9, ' ').collect();
            if fields.len() < 9 {
                continue;
            }
            (fields[1], fields[8], None)
        } else if record.starts_with("2 ") {
            let fields: Vec<_> = record.splitn(10, ' ').collect();
            if fields.len() < 10 {
                continue;
            }
            (fields[1], fields[9], records.next())
        } else if record.starts_with("u ") {
            let fields: Vec<_> = record.splitn(11, ' ').collect();
            if fields.len() < 11 {
                continue;
            }
            (fields[1], fields[10], None)
        } else if let Some(path) = record.strip_prefix("? ") {
            status.files.push(ChangedFile {
                path: path.into(),
                original_path: None,
                staged_status: None,
                worktree_status: Some("?".into()),
            });
            if status.files.len() >= MAX_STATUS_FILES {
                break;
            }
            continue;
        } else {
            continue;
        };
        let mut statuses = xy.chars();
        status.files.push(ChangedFile {
            path: path.into(),
            original_path: original_path.map(str::to_string),
            staged_status: statuses.next().and_then(meaningful_status),
            worktree_status: statuses.next().and_then(meaningful_status),
        });
        if status.files.len() >= MAX_STATUS_FILES {
            break;
        }
    }
    status.files.sort_by(|a, b| a.path.cmp(&b.path));
    status
}

fn valid_relative_file(file: &str) -> bool {
    !file.trim().is_empty()
        && !FsPath::new(file).is_absolute()
        && FsPath::new(file)
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn bounded_diff(bytes: &[u8]) -> (String, bool, bool) {
    let binary = bytes.contains(&0);
    let mut diff = String::from_utf8_lossy(bytes).into_owned();
    let truncated = diff.len() > MAX_DIFF_BYTES;
    if truncated {
        let mut boundary = MAX_DIFF_BYTES;
        while !diff.is_char_boundary(boundary) {
            boundary -= 1;
        }
        diff.truncate(boundary);
        diff.push_str("\n\n… diff truncated by Aura …\n");
    }
    (diff, truncated, binary)
}

async fn untracked_diff(root: &FsPath, file: &str) -> Result<(String, bool, bool), String> {
    let path = root.join(file);
    let metadata = tokio::fs::symlink_metadata(&path)
        .await
        .map_err(|_| "Changed file is unavailable".to_string())?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("Changed file is not a regular file".into());
    }
    let canonical = path
        .canonicalize()
        .map_err(|_| "Changed file is unavailable".to_string())?;
    if !canonical.starts_with(root) {
        return Err("Changed file is outside the repository".into());
    }
    let input = tokio::fs::File::open(canonical)
        .await
        .map_err(|_| "Changed file is unavailable".to_string())?;
    let bytes = read_bounded(input, MAX_DIFF_BYTES)
        .await
        .map_err(|_| "Changed file is unavailable".to_string())?;
    if bytes.contains(&0) {
        return Ok((
            format!("Binary file: {file}"),
            bytes.len() > MAX_DIFF_BYTES,
            true,
        ));
    }
    let content = String::from_utf8_lossy(&bytes);
    let line_count = content.lines().count();
    let mut diff = format!(
        "diff --git a/{file} b/{file}\nnew file mode 100644\n--- /dev/null\n+++ b/{file}\n@@ -0,0 +1,{line_count} @@\n"
    );
    for line in content.lines() {
        diff.push('+');
        diff.push_str(line);
        diff.push('\n');
    }
    let (diff, capped, binary) = bounded_diff(diff.as_bytes());
    Ok((diff, capped || bytes.len() > MAX_DIFF_BYTES, binary))
}

pub(in crate::gateway) async fn git_status_handler(
    State(state): State<RouterState>,
    Query(query): Query<GitStatusQuery>,
) -> impl IntoResponse {
    let workspace = match state.config.resolve_allowed_path(FsPath::new(&query.path)) {
        Ok(path) if path.is_dir() => path,
        Ok(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": "workspace is not a directory" })),
            )
                .into_response()
        }
        Err(error) => return path_error_response(&error).into_response(),
    };
    let root = match repository_root(&workspace).await {
        Ok(root) => root,
        Err(reason) => return Json(GitStatus::unavailable(&reason)).into_response(),
    };
    let status = match changed_files(&root).await {
        Ok(status) => status,
        Err(reason) => return Json(GitStatus::unavailable(&reason)).into_response(),
    };
    Json(status).into_response()
}

pub(in crate::gateway) async fn git_diff_handler(
    State(state): State<RouterState>,
    Query(query): Query<GitDiffQuery>,
) -> impl IntoResponse {
    if !valid_relative_file(&query.file) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "invalid changed file path" })),
        )
            .into_response();
    }
    let workspace = match state.config.resolve_allowed_path(FsPath::new(&query.path)) {
        Ok(path) if path.is_dir() => path,
        Ok(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": "workspace is not a directory" })),
            )
                .into_response()
        }
        Err(error) => return path_error_response(&error).into_response(),
    };
    let root = match repository_root(&workspace).await {
        Ok(root) => root,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": "Git repository unavailable" })),
            )
                .into_response()
        }
    };
    let status = match changed_files(&root).await {
        Ok(status) => status,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": "Git diff unavailable" })),
            )
                .into_response()
        }
    };
    let changed = status.files.iter().find(|entry| entry.path == query.file);
    let available = changed.is_some_and(|entry| match query.area {
        DiffArea::Staged => entry.staged_status.is_some(),
        DiffArea::Worktree => entry.worktree_status.is_some(),
    });
    if !available {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "changed file not found" })),
        )
            .into_response();
    }
    let result = if query.area == DiffArea::Worktree
        && changed.is_some_and(|entry| entry.worktree_status.as_deref() == Some("?"))
    {
        untracked_diff(&root, &query.file).await
    } else {
        let args: Vec<&str> = if query.area == DiffArea::Staged {
            vec![
                "diff",
                "--cached",
                "--no-ext-diff",
                "--no-textconv",
                "--no-color",
                "--",
                &query.file,
            ]
        } else {
            vec![
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "--no-color",
                "--",
                &query.file,
            ]
        };
        git(&root, &args).await.map(|bytes| bounded_diff(&bytes))
    };
    let (diff, truncated, binary) = match result {
        Ok(result) => result,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": "Git diff unavailable" })),
            )
                .into_response()
        }
    };
    Json(GitDiff {
        path: query.file,
        area: query.area,
        diff,
        truncated,
        binary,
    })
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unsafe_diff_paths() {
        assert!(valid_relative_file("src/main.rs"));
        assert!(!valid_relative_file("../secrets"));
        assert!(!valid_relative_file("/etc/passwd"));
        assert!(!valid_relative_file("./src/main.rs"));
    }

    #[test]
    fn parses_changed_files_and_branch() {
        let status = parse_status(b"# branch.head main\0# branch.ab +2 -1\0? new file.rs\0");
        assert_eq!(status.branch.as_deref(), Some("main"));
        assert_eq!((status.ahead, status.behind), (2, 1));
        assert_eq!(status.files[0].path, "new file.rs");
    }
}
