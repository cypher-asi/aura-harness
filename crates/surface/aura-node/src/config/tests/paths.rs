//! Defaults, data-dir paths, the file root and project-path resolution.

use super::super::{default_data_dir, NodeConfig, PathError};
use std::path::{Path, PathBuf};

#[test]
fn test_default_config() {
    let config = NodeConfig::default();
    let default_data_dir = default_data_dir();

    assert_eq!(config.data_dir, default_data_dir);
    assert_eq!(config.bind_addr, "127.0.0.1:8080");
    assert!(!config.sync_writes);
    assert_eq!(config.record_window_size, 50);
    assert_eq!(config.aura_storage_url, "https://aura-storage.onrender.com");
    assert_eq!(config.aura_network_url, "https://aura-network.onrender.com");
    assert!(
        config.aura_os_server_url.is_none(),
        "aura-os-server override is additive / opt-in; default must be None so HttpDomainApi falls back to aura_storage_url"
    );
    assert!(config.project_base.is_none());
    assert!(!config.allow_unrestricted_full_access);
}

#[test]
fn test_db_path() {
    let config = NodeConfig::default();
    assert_eq!(config.db_path(), default_data_dir().join("db"));
}

#[test]
fn test_workspaces_path() {
    let config = NodeConfig::default();
    assert_eq!(
        config.workspaces_path(),
        default_data_dir().join("workspaces")
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
fn a_cloud_machine_browses_its_project_directory() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(workspace.join("src")).unwrap();
    std::fs::write(workspace.join("src/main.rs"), "fn main() {}").unwrap();
    std::fs::create_dir_all(dir.path().join("data/workspaces")).unwrap();
    let config = NodeConfig {
        data_dir: dir.path().join("data"),
        workspace: Some(workspace.clone()),
        ..NodeConfig::default()
    };

    assert_eq!(config.file_root(), workspace);
    let main = config
        .resolve_allowed_path(&workspace.join("src/main.rs"))
        .unwrap();
    assert!(main.ends_with("workspace/src/main.rs"));
    assert!(config.resolve_allowed_path(Path::new("src")).is_ok());
    assert!(matches!(
        config.resolve_allowed_path(&dir.path().join("data/workspaces")),
        Err(PathError::Escapes(_))
    ));

    let based = NodeConfig {
        project_base: Some(dir.path().join("base")),
        ..config
    };
    assert_eq!(based.file_root(), dir.path().join("base"));
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
