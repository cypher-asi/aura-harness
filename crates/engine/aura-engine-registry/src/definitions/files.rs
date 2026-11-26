//! The definition directory's files: the seed list, retired seeds,
//! reading a definition, and creating and writing files.

use std::fs;
use std::path::Path;

use super::{
    AgentDefinition, AgentDefinitionError, LEGACY_SEEDS, MAX_FILE_BYTES, SEEDED_FILE,
    SEED_DEFINITIONS,
};

/// The seeds `dir` has already had. A directory without the list was
/// created by a version that seeded only [`LEGACY_SEEDS`].
pub(super) fn read_seeded(dir: &Path) -> Vec<String> {
    match fs::read_to_string(dir.join(SEEDED_FILE)) {
        Ok(list) => list
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect(),
        Err(_) => LEGACY_SEEDS.iter().map(|s| (*s).to_string()).collect(),
    }
}

/// Write the current built-in over each seed file in `dir` that is byte
/// for byte one of the `retired` versions (name and blake3 hash), and
/// return the names written. A file that can't be read is left alone.
pub(super) fn refresh_retired(
    dir: &Path,
    retired: &[(&str, &str)],
) -> Result<Vec<&'static str>, AgentDefinitionError> {
    let mut refreshed = Vec::new();
    for (name, content) in SEED_DEFINITIONS {
        if !retired.iter().any(|(retired, _)| retired == name) {
            continue;
        }
        let path = dir.join(format!("{name}.md"));
        let Ok(bytes) = fs::read(&path) else {
            continue;
        };
        let hash = blake3::hash(&bytes).to_hex();
        if !retired.contains(&(*name, hash.as_str())) {
            continue;
        }
        write_file(&path, content)?;
        refreshed.push(*name);
    }
    Ok(refreshed)
}

pub(super) fn read_definition(path: &Path) -> Result<AgentDefinition, AgentDefinitionError> {
    let size = fs::metadata(path)?.len();
    if size > MAX_FILE_BYTES {
        return Err(AgentDefinitionError::TooLarge {
            max: MAX_FILE_BYTES,
        });
    }
    let content = fs::read_to_string(path)?;
    AgentDefinition::parse(&content, path)
}

/// Create `dir` and its parents.
pub(super) fn create_dir(dir: &Path) -> Result<(), AgentDefinitionError> {
    fs::create_dir_all(dir).map_err(|source| AgentDefinitionError::CreateDir {
        dir: dir.to_path_buf(),
        source,
    })
}

/// Write `contents` to the file at `path`.
pub(super) fn write_file(
    path: &Path,
    contents: impl AsRef<[u8]>,
) -> Result<(), AgentDefinitionError> {
    fs::write(path, contents).map_err(|source| AgentDefinitionError::WriteFile {
        path: path.to_path_buf(),
        source,
    })
}
