use std::path::Path;

use serde::{Deserialize, Serialize};

mod apply;
pub(crate) mod error_context;
pub mod file_walkers;
pub(crate) mod source_parser;
pub mod stub_detection;
pub mod task_keywords;
pub mod task_relevance;
pub mod type_resolution;
pub mod validation;
pub mod workspace_map;

pub use apply::{apply_file_ops, compute_file_changes};
pub use error_context::{resolve_error_context, resolve_error_source_files, ERROR_SOURCE_BUDGET};
pub(crate) use source_parser::{extract_definition_block, extract_pub_signatures};
pub use stub_detection::*;
pub use task_relevance::*;
pub use validation::*;
pub use workspace_map::*;

#[derive(Debug, thiserror::Error)]
pub enum FileOpsError {
    #[error("IO error: {0}")]
    Io(String),
    #[error("path escape attempt: {0}")]
    PathEscape(String),
    #[error("parse error: {0}")]
    Parse(String),
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileChangeSummary {
    pub op: String,
    pub path: String,
    #[serde(default)]
    pub lines_added: u32,
    #[serde(default)]
    pub lines_removed: u32,
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

pub(crate) const SKIP_DIRS: &[&str] = &[
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

pub(crate) const INCLUDE_EXTENSIONS: &[&str] = &[
    "rs", "ts", "tsx", "js", "jsx", "json", "toml", "md", "css", "html", "yaml", "yml", "py", "sh",
    "sql", "graphql",
];

pub fn read_relevant_files(linked_folder: &str, max_bytes: usize) -> Result<String, FileOpsError> {
    let base = Path::new(linked_folder);
    let mut output = String::new();
    let mut current_size: usize = 0;
    walk_and_collect(base, base, &mut output, &mut current_size, max_bytes)?;
    Ok(output)
}

fn walk_and_collect(
    base: &Path,
    dir: &Path,
    output: &mut String,
    current_size: &mut usize,
    max_bytes: usize,
) -> Result<(), FileOpsError> {
    let mut included = std::collections::HashSet::new();
    file_walkers::walk_and_collect_filtered(
        base,
        dir,
        output,
        current_size,
        max_bytes,
        &mut included,
    )
}

/// Fuzzy whitespace-insensitive search and replace. Matches lines by trimmed
/// content rather than exact whitespace. Returns `None` if zero or multiple
/// matches are found.
pub(crate) fn fuzzy_search_replace(content: &str, search: &str, replace: &str) -> Option<String> {
    let search_lines: Vec<&str> = search.lines().map(|l| l.trim()).collect();
    if search_lines.is_empty() || search_lines.iter().all(|l| l.is_empty()) {
        return None;
    }

    let content_lines: Vec<&str> = content.lines().collect();
    let mut match_positions: Vec<usize> = Vec::new();

    'outer: for start in 0..content_lines.len() {
        if start + search_lines.len() > content_lines.len() {
            break;
        }
        for (j, search_line) in search_lines.iter().enumerate() {
            if content_lines[start + j].trim() != *search_line {
                continue 'outer;
            }
        }
        match_positions.push(start);
    }

    if match_positions.len() != 1 {
        return None;
    }

    let match_start = match_positions[0];
    let match_end = match_start + search_lines.len();

    let mut result = String::with_capacity(content.len());
    for (i, line) in content_lines.iter().enumerate() {
        if i == match_start {
            result.push_str(replace);
            if !replace.ends_with('\n') {
                result.push('\n');
            }
        } else if i >= match_start && i < match_end {
            continue;
        } else {
            result.push_str(line);
            result.push('\n');
        }
    }
    if !content.ends_with('\n') && result.ends_with('\n') {
        result.pop();
    }

    Some(result)
}
