use std::path::Path;

use serde::{Deserialize, Serialize};

pub mod error_context;
pub mod source_parser;
pub mod stub_detection;

pub use error_context::resolve_error_context;
pub use stub_detection::*;

#[derive(Debug, thiserror::Error)]
pub enum FileOpsError {
    #[error("path escape attempt: {0}")]
    PathEscape(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Replacement {
    pub search: String,
    pub replace: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum FileOp {
    Create {
        path: String,
        content: String,
    },
    Modify {
        path: String,
        content: String,
    },
    Delete {
        path: String,
    },
    SearchReplace {
        path: String,
        replacements: Vec<Replacement>,
    },
}

pub fn validate_path(base: &Path, target: &Path) -> Result<(), FileOpsError> {
    let norm_base = lexical_normalize(base);
    let norm_target = lexical_normalize(target);

    if !norm_target.starts_with(&norm_base) {
        return Err(FileOpsError::PathEscape(target.display().to_string()));
    }
    Ok(())
}

/// Resolve `.` and `..` components without hitting the filesystem, avoiding
/// Windows `\\?\` extended-path issues that `canonicalize()` introduces.
fn lexical_normalize(path: &Path) -> std::path::PathBuf {
    use std::path::Component;
    let mut out = std::path::PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

pub const SKIP_DIRS: &[&str] = &[
    ".git",
    "target",
    "node_modules",
    "__pycache__",
    ".venv",
    "dist",
];

/// References extracted from compiler error output for targeted context resolution.
#[derive(Debug, Default)]
pub struct ErrorReferences {
    pub types_referenced: Vec<String>,
    pub methods_not_found: Vec<(String, String)>,
    pub missing_fields: Vec<(String, String)>,
    pub source_locations: Vec<(String, u32)>,
    pub wrong_arg_counts: Vec<String>,
}
