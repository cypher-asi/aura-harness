//! Node configuration.

use std::path::PathBuf;

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
    /// Reasoner gateway URL
    pub reasoner_url: String,
    /// Reasoner timeout in milliseconds
    pub reasoner_timeout_ms: u64,
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
            data_dir: PathBuf::from("./aura_data"),
            project_base: None,
            bind_addr: "127.0.0.1:8080".to_string(),
            sync_writes: false,
            record_window_size: 50,
            reasoner_url: "http://localhost:3000".to_string(),
            reasoner_timeout_ms: 30_000,
            enable_fs_tools: true,
            enable_cmd_tools: false,
            allowed_commands: vec![],
            orbit_url: "https://orbit-sfvu.onrender.com".to_string(),
            aura_storage_url: "https://aura-storage.onrender.com".to_string(),
            aura_network_url: "https://aura-network.onrender.com".to_string(),
        }
    }
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
        if let Ok(val) = std::env::var("REASONER_URL") {
            config.reasoner_url = val;
        }
        if let Ok(val) = std::env::var("REASONER_TIMEOUT_MS") {
            if let Ok(n) = val.parse() {
                config.reasoner_timeout_ms = n;
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
        self.data_dir.join("db")
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
    /// Accepts paths under `project_base` (remote) or `workspaces_path` (local).
    #[must_use]
    pub fn is_allowed_path(&self, path: &std::path::Path) -> bool {
        if let Some(ref base) = self.project_base {
            path.starts_with(base)
        } else {
            path.starts_with(&self.workspaces_path())
        }
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
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Mutex to serialize env var tests (env vars are process-global)
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn clear_node_env_vars() {
        std::env::remove_var("DATA_DIR");
        std::env::remove_var("AURA_DATA_DIR");
        std::env::remove_var("BIND_ADDR");
        std::env::remove_var("AURA_LISTEN_ADDR");
        std::env::remove_var("SYNC_WRITES");
        std::env::remove_var("RECORD_WINDOW_SIZE");
        std::env::remove_var("REASONER_URL");
        std::env::remove_var("REASONER_TIMEOUT_MS");
        std::env::remove_var("ENABLE_FS_TOOLS");
        std::env::remove_var("ENABLE_CMD_TOOLS");
        std::env::remove_var("ALLOWED_COMMANDS");
        std::env::remove_var("ORBIT_URL");
        std::env::remove_var("AURA_STORAGE_URL");
        std::env::remove_var("AURA_NETWORK_URL");
        std::env::remove_var("AURA_PROJECT_BASE");
    }

    #[test]
    fn test_default_config() {
        let config = NodeConfig::default();

        assert_eq!(config.data_dir, PathBuf::from("./aura_data"));
        assert_eq!(config.bind_addr, "127.0.0.1:8080");
        assert!(!config.sync_writes);
        assert_eq!(config.record_window_size, 50);
        assert_eq!(config.reasoner_url, "http://localhost:3000");
        assert_eq!(config.reasoner_timeout_ms, 30_000);
        assert!(config.enable_fs_tools);
        assert!(!config.enable_cmd_tools);
        assert!(config.allowed_commands.is_empty());
        assert_eq!(config.orbit_url, "https://orbit-sfvu.onrender.com");
        assert_eq!(config.aura_storage_url, "https://aura-storage.onrender.com");
        assert_eq!(config.aura_network_url, "https://aura-network.onrender.com");
        assert!(config.project_base.is_none());
    }

    #[test]
    fn test_db_path() {
        let config = NodeConfig::default();
        assert_eq!(config.db_path(), PathBuf::from("./aura_data/db"));
    }

    #[test]
    fn test_workspaces_path() {
        let config = NodeConfig::default();
        assert_eq!(
            config.workspaces_path(),
            PathBuf::from("./aura_data/workspaces")
        );
    }

    #[test]
    fn test_custom_data_dir() {
        let config = NodeConfig {
            data_dir: PathBuf::from("/custom/path"),
            ..NodeConfig::default()
        };

        assert_eq!(config.db_path(), PathBuf::from("/custom/path/db"));
        assert_eq!(
            config.workspaces_path(),
            PathBuf::from("/custom/path/workspaces")
        );
    }

    #[test]
    fn test_from_env_uses_defaults_when_not_set() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_node_env_vars();

        let config = NodeConfig::from_env();
        let default = NodeConfig::default();

        assert_eq!(config.data_dir, default.data_dir);
        assert_eq!(config.bind_addr, default.bind_addr);
        assert_eq!(config.sync_writes, default.sync_writes);
    }

    #[test]
    fn test_sync_writes_parsing() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_node_env_vars();

        // Test "true"
        std::env::set_var("SYNC_WRITES", "true");
        let config = NodeConfig::from_env();
        assert!(config.sync_writes);

        // Test "1"
        std::env::set_var("SYNC_WRITES", "1");
        let config = NodeConfig::from_env();
        assert!(config.sync_writes);

        // Test "false"
        std::env::set_var("SYNC_WRITES", "false");
        let config = NodeConfig::from_env();
        assert!(!config.sync_writes);

        clear_node_env_vars();
    }

    #[test]
    fn test_enable_fs_tools_parsing() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_node_env_vars();

        // Test disabling with "false"
        std::env::set_var("ENABLE_FS_TOOLS", "false");
        let config = NodeConfig::from_env();
        assert!(!config.enable_fs_tools);

        // Test disabling with "0"
        std::env::set_var("ENABLE_FS_TOOLS", "0");
        let config = NodeConfig::from_env();
        assert!(!config.enable_fs_tools);

        // Test keeping enabled with any other value
        std::env::set_var("ENABLE_FS_TOOLS", "yes");
        let config = NodeConfig::from_env();
        assert!(config.enable_fs_tools);

        clear_node_env_vars();
    }

    #[test]
    fn test_allowed_commands_parsing() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_node_env_vars();

        std::env::set_var("ALLOWED_COMMANDS", "ls,cat,echo");
        let config = NodeConfig::from_env();
        assert_eq!(config.allowed_commands, vec!["ls", "cat", "echo"]);

        clear_node_env_vars();
    }

    #[test]
    fn test_record_window_size_parsing() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_node_env_vars();

        std::env::set_var("RECORD_WINDOW_SIZE", "100");
        let config = NodeConfig::from_env();
        assert_eq!(config.record_window_size, 100);

        // Invalid value should keep default
        std::env::set_var("RECORD_WINDOW_SIZE", "invalid");
        let config = NodeConfig::from_env();
        assert_eq!(config.record_window_size, 50); // default

        clear_node_env_vars();
    }

    #[test]
    fn test_reasoner_url_env() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_node_env_vars();

        std::env::set_var("REASONER_URL", "http://custom:5000");
        let config = NodeConfig::from_env();
        assert_eq!(config.reasoner_url, "http://custom:5000");

        clear_node_env_vars();
    }

    #[test]
    fn test_reasoner_timeout_env() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_node_env_vars();

        std::env::set_var("REASONER_TIMEOUT_MS", "60000");
        let config = NodeConfig::from_env();
        assert_eq!(config.reasoner_timeout_ms, 60_000);

        std::env::set_var("REASONER_TIMEOUT_MS", "not_a_number");
        let config = NodeConfig::from_env();
        assert_eq!(config.reasoner_timeout_ms, 30_000);

        clear_node_env_vars();
    }

    #[test]
    fn test_bind_addr_env() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_node_env_vars();

        std::env::set_var("BIND_ADDR", "0.0.0.0:3000");
        let config = NodeConfig::from_env();
        assert_eq!(config.bind_addr, "0.0.0.0:3000");

        clear_node_env_vars();
    }

    #[test]
    fn test_enable_cmd_tools_parsing() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_node_env_vars();

        std::env::set_var("ENABLE_CMD_TOOLS", "true");
        let config = NodeConfig::from_env();
        assert!(config.enable_cmd_tools);

        std::env::set_var("ENABLE_CMD_TOOLS", "1");
        let config = NodeConfig::from_env();
        assert!(config.enable_cmd_tools);

        std::env::set_var("ENABLE_CMD_TOOLS", "false");
        let config = NodeConfig::from_env();
        assert!(!config.enable_cmd_tools);

        std::env::set_var("ENABLE_CMD_TOOLS", "anything_else");
        let config = NodeConfig::from_env();
        assert!(!config.enable_cmd_tools);

        clear_node_env_vars();
    }

    #[test]
    fn test_allowed_commands_empty_string() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_node_env_vars();

        std::env::set_var("ALLOWED_COMMANDS", "");
        let config = NodeConfig::from_env();
        assert_eq!(config.allowed_commands, vec![""]);

        clear_node_env_vars();
    }

    #[test]
    fn test_allowed_commands_single_command() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_node_env_vars();

        std::env::set_var("ALLOWED_COMMANDS", "cargo");
        let config = NodeConfig::from_env();
        assert_eq!(config.allowed_commands, vec!["cargo"]);

        clear_node_env_vars();
    }

    #[test]
    fn test_full_env_override() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_node_env_vars();

        std::env::set_var("DATA_DIR", "/opt/aura");
        std::env::set_var("BIND_ADDR", "0.0.0.0:4000");
        std::env::set_var("SYNC_WRITES", "true");
        std::env::set_var("RECORD_WINDOW_SIZE", "200");
        std::env::set_var("REASONER_URL", "http://reasoner:8080");
        std::env::set_var("REASONER_TIMEOUT_MS", "45000");
        std::env::set_var("ENABLE_FS_TOOLS", "false");
        std::env::set_var("ENABLE_CMD_TOOLS", "true");
        std::env::set_var("ALLOWED_COMMANDS", "git,cargo,npm");
        std::env::set_var("ORBIT_URL", "https://orbit.example.com");
        std::env::set_var("AURA_STORAGE_URL", "https://storage.example.com");
        std::env::set_var("AURA_NETWORK_URL", "https://network.example.com");

        let config = NodeConfig::from_env();

        assert_eq!(config.data_dir, PathBuf::from("/opt/aura"));
        assert_eq!(config.bind_addr, "0.0.0.0:4000");
        assert!(config.sync_writes);
        assert_eq!(config.record_window_size, 200);
        assert_eq!(config.reasoner_url, "http://reasoner:8080");
        assert_eq!(config.reasoner_timeout_ms, 45_000);
        assert!(!config.enable_fs_tools);
        assert!(config.enable_cmd_tools);
        assert_eq!(config.allowed_commands, vec!["git", "cargo", "npm"]);
        assert_eq!(config.orbit_url, "https://orbit.example.com");
        assert_eq!(config.aura_storage_url, "https://storage.example.com");
        assert_eq!(config.aura_network_url, "https://network.example.com");

        clear_node_env_vars();
    }

    #[test]
    fn test_project_base_env() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_node_env_vars();

        std::env::set_var("AURA_PROJECT_BASE", "/home/aura");
        let config = NodeConfig::from_env();
        assert_eq!(config.project_base, Some(PathBuf::from("/home/aura")));

        clear_node_env_vars();
    }

    #[test]
    fn test_project_base_empty_string_ignored() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_node_env_vars();

        std::env::set_var("AURA_PROJECT_BASE", "");
        let config = NodeConfig::from_env();
        assert!(config.project_base.is_none());

        clear_node_env_vars();
    }

    #[test]
    fn test_resolve_project_path_with_base() {
        let config = NodeConfig {
            project_base: Some(PathBuf::from("/home/aura")),
            ..NodeConfig::default()
        };
        let incoming = std::path::Path::new("/state/workspaces/my-app");
        assert_eq!(
            config.resolve_project_path(incoming),
            PathBuf::from("/home/aura/my-app")
        );
    }

    #[test]
    fn test_resolve_project_path_without_base() {
        let config = NodeConfig::default();
        let incoming = std::path::Path::new("/state/workspaces/my-app");
        assert_eq!(
            config.resolve_project_path(incoming),
            PathBuf::from("/state/workspaces/my-app")
        );
    }

    #[test]
    fn test_resolve_project_path_local_absolute() {
        let config = NodeConfig {
            project_base: Some(PathBuf::from("/home/aura")),
            ..NodeConfig::default()
        };
        let incoming = std::path::Path::new("/some/deep/nested/cool-project");
        assert_eq!(
            config.resolve_project_path(incoming),
            PathBuf::from("/home/aura/cool-project")
        );
    }
}
