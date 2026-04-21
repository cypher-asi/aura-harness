//! Node configuration.

use std::path::{Path, PathBuf};

/// Errors returned by [`NodeConfig::resolve_allowed_path`].
///
/// The variants map onto distinct HTTP statuses so the file handlers can
/// signal `400`, `403`, and `404` separately instead of collapsing every
/// refusal into a single opaque error (which is what the previous
/// `bool`-returning `is_allowed_path` forced them to do).
#[derive(Debug, thiserror::Error)]
pub enum PathError {
    /// The resolved path does not exist on disk.
    #[error("path not found: {0}")]
    NotFound(PathBuf),
    /// The resolved path's canonical form escapes the workspace root.
    #[error("path escapes workspace: {0}")]
    Escapes(PathBuf),
    /// The workspace root itself is unavailable, or canonicalization
    /// failed for a reason other than `NotFound` (e.g. permission denied).
    #[error("path not permitted: {0}")]
    NotPermitted(String),
}

/// Node configuration.
#[derive(Debug, Clone)]
pub struct NodeConfig {
    /// Data directory for `RocksDB` and workspaces
    pub data_dir: PathBuf,
    /// Base directory for project workspaces on remote VMs.
    /// When set (e.g. `/home/aura`), incoming `project_path` / `workspace_root`
    /// values are remapped to `{project_base}/{slug}` where slug is the last
    /// path component of the incoming path.  When `None` paths pass through
    /// unchanged (local development).
    pub project_base: Option<PathBuf>,
    /// HTTP server bind address
    pub bind_addr: String,
    /// Enable sync writes to `RocksDB`
    pub sync_writes: bool,
    /// Record window size for kernel context
    pub record_window_size: usize,
    /// Enable filesystem tools
    pub enable_fs_tools: bool,
    /// Enable command tools
    pub enable_cmd_tools: bool,
    /// Allowed commands (if cmd tools enabled)
    pub allowed_commands: Vec<String>,
    /// Orbit service URL
    pub orbit_url: String,
    /// Aura Storage service URL
    pub aura_storage_url: String,
    /// Aura Network service URL
    pub aura_network_url: String,
}

impl Default for NodeConfig {
    fn default() -> Self {
        Self {
            data_dir: default_data_dir(),
            project_base: None,
            bind_addr: "127.0.0.1:8080".to_string(),
            sync_writes: false,
            record_window_size: 50,
            enable_fs_tools: true,
            enable_cmd_tools: false,
            allowed_commands: vec![],
            orbit_url: "https://orbit-sfvu.onrender.com".to_string(),
            aura_storage_url: "https://aura-storage.onrender.com".to_string(),
            aura_network_url: "https://aura-network.onrender.com".to_string(),
        }
    }
}

fn default_data_dir() -> PathBuf {
    dirs::data_local_dir().map_or_else(
        || PathBuf::from("./aura_data"),
        |path| path.join("aura").join("node"),
    )
}

impl NodeConfig {
    /// Load configuration from environment variables.
    #[must_use]
    pub fn from_env() -> Self {
        let mut config = Self::default();

        if let Ok(val) = std::env::var("AURA_DATA_DIR").or_else(|_| std::env::var("DATA_DIR")) {
            config.data_dir = PathBuf::from(val);
        }
        if let Ok(val) = std::env::var("AURA_LISTEN_ADDR").or_else(|_| std::env::var("BIND_ADDR")) {
            config.bind_addr = val;
        }
        if let Ok(val) = std::env::var("SYNC_WRITES") {
            config.sync_writes = val == "true" || val == "1";
        }
        if let Ok(val) = std::env::var("RECORD_WINDOW_SIZE") {
            if let Ok(n) = val.parse() {
                config.record_window_size = n;
            }
        }
        if let Ok(val) = std::env::var("ENABLE_FS_TOOLS") {
            config.enable_fs_tools = val != "false" && val != "0";
        }
        if let Ok(val) = std::env::var("ENABLE_CMD_TOOLS") {
            config.enable_cmd_tools = val == "true" || val == "1";
        }
        if let Ok(val) = std::env::var("ALLOWED_COMMANDS") {
            config.allowed_commands = val.split(',').map(String::from).collect();
        }
        if let Ok(val) = std::env::var("ORBIT_URL") {
            config.orbit_url = val;
        }
        if let Ok(val) = std::env::var("AURA_STORAGE_URL") {
            config.aura_storage_url = val;
        }
        if let Ok(val) = std::env::var("AURA_NETWORK_URL") {
            config.aura_network_url = val;
        }
        if let Ok(val) = std::env::var("AURA_PROJECT_BASE") {
            if !val.is_empty() {
                config.project_base = Some(PathBuf::from(val));
            }
        }
        config
    }

    /// Get the `RocksDB` path.
    #[must_use]
    pub fn db_path(&self) -> PathBuf {
        aura_agent::session_bootstrap::resolve_store_path(&self.data_dir)
    }

    /// Get the workspaces base path.
    #[must_use]
    pub fn workspaces_path(&self) -> PathBuf {
        self.data_dir.join("workspaces")
    }

    /// Remap an incoming project path through `project_base` when configured.
    ///
    /// Extracts the last path component (the project slug) and returns
    /// `{project_base}/{slug}`. When `project_base` is `None` the path passes
    /// through unchanged.
    #[must_use]
    pub fn resolve_project_path(&self, incoming: &std::path::Path) -> PathBuf {
        if let Some(ref base) = self.project_base {
            let slug = incoming
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("default");
            base.join(slug)
        } else {
            incoming.to_path_buf()
        }
    }

    /// Resolve the canonical workspace directory for a project by name.
    ///
    /// This is the single source of truth for where a project's files live.
    /// - Remote VMs (`project_base` set): `{project_base}/{slug}` e.g. `/home/aura/testaaa`
    /// - Local dev (`project_base` unset): `{data_dir}/workspaces/{slug}`
    #[must_use]
    pub fn resolve_workspace_for_project(&self, project_name: &str) -> PathBuf {
        let slug = slugify(project_name);
        if let Some(ref base) = self.project_base {
            base.join(&slug)
        } else {
            self.workspaces_path().join(&slug)
        }
    }

    /// Check whether a path is allowed for file operations.
    ///
    /// Thin wrapper around [`Self::resolve_allowed_path`] retained for
    /// callers that only care whether a path is legal and don't need the
    /// canonical form. New code should prefer `resolve_allowed_path` so
    /// traversal attempts can be distinguished from missing files.
    #[must_use]
    pub fn is_allowed_path(&self, path: &Path) -> bool {
        self.resolve_allowed_path(path).is_ok()
    }

    /// Resolve `input` to a canonical path inside the workspace root.
    ///
    /// Replaces the previous `Path::starts_with` check against the raw
    /// input, which was bypassable with `../` sequences that only
    /// normalised after canonicalisation. The new implementation:
    ///
    /// 1. Canonicalises the workspace root (so symlinks / junctions
    ///    anywhere in the root's ancestry resolve to their real target).
    /// 2. Joins relative `input`s onto the root before canonicalising.
    /// 3. Canonicalises the candidate path, which follows symlinks to
    ///    their real target.
    /// 4. Verifies the canonical candidate lives under the canonical
    ///    root via `starts_with`. Any traversal, symlink, or junction
    ///    that lands outside fails here.
    ///
    /// Relative paths, absolute paths, `.`, and empty inputs are all
    /// accepted — empty / `.` inputs resolve to the root itself.
    ///
    /// # Errors
    ///
    /// * [`PathError::NotFound`] — the candidate path does not exist.
    /// * [`PathError::Escapes`] — the candidate's canonical form is not
    ///   a descendant of the canonical workspace root.
    /// * [`PathError::NotPermitted`] — the workspace root itself
    ///   cannot be canonicalised (missing / permission denied), or the
    ///   candidate's canonicalisation failed for a non-NotFound reason.
    pub fn resolve_allowed_path(&self, input: &Path) -> Result<PathBuf, PathError> {
        let root = self.file_root();
        let canonical_root = std::fs::canonicalize(&root).map_err(|e| {
            PathError::NotPermitted(format!(
                "workspace root unavailable ({}): {e}",
                root.display()
            ))
        })?;
        let canonical_root = strip_unc_prefix(&canonical_root);

        let candidate = if input.as_os_str().is_empty() || input == Path::new(".") {
            root.clone()
        } else if input.is_absolute() {
            input.to_path_buf()
        } else {
            root.join(input)
        };

        let canonical_candidate = match std::fs::canonicalize(&candidate) {
            Ok(p) => strip_unc_prefix(&p),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(PathError::NotFound(candidate));
            }
            Err(e) => {
                return Err(PathError::NotPermitted(format!(
                    "canonicalize({}): {e}",
                    candidate.display()
                )));
            }
        };

        if !canonical_candidate.starts_with(&canonical_root) {
            return Err(PathError::Escapes(canonical_candidate));
        }

        Ok(canonical_candidate)
    }

    /// Return the root directory for file browsing (project_base or workspaces).
    #[must_use]
    pub fn file_root(&self) -> PathBuf {
        if let Some(ref base) = self.project_base {
            base.clone()
        } else {
            self.workspaces_path()
        }
    }
}

/// Strip the `\\?\` verbatim prefix that Windows `canonicalize()` adds.
/// On non-Windows this is a no-op.
fn strip_unc_prefix(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    s.strip_prefix(r"\\?\")
        .map_or_else(|| path.to_path_buf(), PathBuf::from)
}

fn slugify(name: &str) -> String {
    let s = name
        .trim()
        .to_lowercase()
        .replace(char::is_whitespace, "-")
        .replace(|c: char| !c.is_ascii_alphanumeric() && c != '-', "");
    if s.is_empty() {
        "unnamed-project".to_string()
    } else {
        s
    }
}

#[cfg(test)]
mod tests;
