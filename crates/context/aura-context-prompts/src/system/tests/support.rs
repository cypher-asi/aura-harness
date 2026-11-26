//! Shared fixtures for the system-prompt tests.

use crate::descriptors::ProjectInfo;

pub(super) fn test_project(folder: &str) -> ProjectInfo<'_> {
    ProjectInfo {
        project_id: None,
        name: "TestProj",
        description: "Test project description",
        folder_path: folder,
        build_command: Some("cargo build"),
        test_command: Some("cargo test"),
    }
}

pub(super) fn demo_project(folder: &str) -> ProjectInfo<'_> {
    ProjectInfo {
        project_id: None,
        name: "Demo",
        description: "A demo project.",
        folder_path: folder,
        build_command: Some("cargo build"),
        test_command: Some("cargo test"),
    }
}
