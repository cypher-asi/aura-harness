//! A skill's folder as files: read within caps to send to a cloud node
//! ([`bundle`]), written back there ([`write_folder`]), and its reference
//! files listed and read for `load_skill`.
//!
//! # Invariants
//!
//! * A folder carries its `SKILL.md` and the UTF-8 files under
//!   `references/`, at most [`MAX_FILE_BYTES`] each, [`MAX_FILES`] per
//!   skill and [`MAX_BUNDLE_BYTES`] in all. What goes over is left out
//!   with a warning; a skill's `SKILL.md` always goes first.
//! * Paths are relative, `/`-separated, with no `..`: a folder written
//!   from a bundle stays inside its root.

use std::path::{Component, Path, PathBuf};

use aura_core_protocol::cloud::{SkillBundle, SkillFile, SkillFolder};
use tracing::warn;

use crate::types::Skill;

/// Largest file sent or read.
pub const MAX_FILE_BYTES: u64 = 256 * 1024;
/// Most files sent for one skill.
pub const MAX_FILES: usize = 64;
/// Most bytes sent for all of an agent's skills.
pub const MAX_BUNDLE_BYTES: usize = 2 * 1024 * 1024;
/// The skill's own file.
pub const SKILL_FILE: &str = "SKILL.md";
/// Where a skill's reference files are.
pub const REFERENCES: &str = "references";

/// The bundle of `skills`, within the caps.
#[must_use]
pub fn bundle(skills: &[&Skill]) -> SkillBundle {
    let mut budget = MAX_BUNDLE_BYTES;
    let folders = skills
        .iter()
        .map(|skill| folder_of(skill, &mut budget))
        .collect();
    SkillBundle::new(folders)
}

/// `skill`'s folder: its `SKILL.md`, then its references, while
/// `budget` lasts.
fn folder_of(skill: &Skill, budget: &mut usize) -> SkillFolder {
    let name = skill.frontmatter.name.clone();
    let mut files = Vec::new();
    let mut paths = vec![SKILL_FILE.to_string()];
    paths.extend(reference_files(&skill.dir_path));
    for path in paths {
        if files.len() >= MAX_FILES {
            warn!(skill = %name, %path, "Skill has too many files to send; the rest are left out");
            break;
        }
        match read_text(&skill.dir_path.join(&path)) {
            Ok(text) if text.len() <= *budget => {
                *budget -= text.len();
                files.push(SkillFile { path, text });
            }
            Ok(_) => warn!(skill = %name, %path, "Skills too large to send in all; file left out"),
            Err(why) => warn!(skill = %name, %path, %why, "Skill file left out"),
        }
    }
    SkillFolder { name, files }
}

/// The files under `dir`'s `references/`, as `/`-separated paths
/// relative to `dir`, sorted.
#[must_use]
pub fn reference_files(dir: &Path) -> Vec<String> {
    let mut found = Vec::new();
    let mut pending = vec![dir.join(REFERENCES)];
    while let Some(next) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&next) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => pending.push(path),
                Ok(kind) if kind.is_file() => {
                    if let Ok(relative) = path.strip_prefix(dir) {
                        found.push(slashed(relative));
                    }
                }
                _ => {}
            }
        }
    }
    found.sort();
    found
}

/// `path` with `/` between its parts.
fn slashed(path: &Path) -> String {
    let parts: Vec<String> = path
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect();
    parts.join("/")
}

/// The text of the file at `path`, within [`MAX_FILE_BYTES`].
///
/// # Errors
/// When it can't be read, is too large or isn't UTF-8.
pub fn read_text(path: &Path) -> Result<String, String> {
    let len = std::fs::metadata(path)
        .map_err(|e| format!("can't read it: {e}"))?
        .len();
    if len > MAX_FILE_BYTES {
        return Err(format!("it is larger than {MAX_FILE_BYTES} bytes"));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("can't read it: {e}"))?;
    String::from_utf8(bytes).map_err(|_| "it isn't text".to_string())
}

/// `path` as a path inside a folder: relative, without `..`.
#[must_use]
pub fn inside(path: &str) -> Option<PathBuf> {
    let path = Path::new(path);
    let mut kept = PathBuf::new();
    for part in path.components() {
        match part {
            Component::Normal(part) => kept.push(part),
            Component::CurDir => {}
            _ => return None,
        }
    }
    (!kept.as_os_str().is_empty()).then_some(kept)
}

/// Write `folder` as `root/<name>`, replacing what was there: written to
/// a folder beside it, then renamed over it.
///
/// # Errors
/// When its name or a path leaves the folder, or a write fails.
pub fn write_folder(root: &Path, folder: &SkillFolder) -> std::io::Result<()> {
    let invalid =
        |what: &str| std::io::Error::new(std::io::ErrorKind::InvalidInput, what.to_string());
    let name = inside(&folder.name)
        .filter(|name| name.components().count() == 1)
        .ok_or_else(|| invalid("a skill's name must be one folder name"))?;
    let incoming = root.join(format!(".incoming-{}", name.display()));
    if incoming.exists() {
        std::fs::remove_dir_all(&incoming)?;
    }
    for file in &folder.files {
        let path = inside(&file.path)
            .ok_or_else(|| invalid("a skill file's path must stay inside its folder"))?;
        let target = incoming.join(path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(target, &file.text)?;
    }
    std::fs::create_dir_all(&incoming)?;
    let target = root.join(name);
    if target.exists() {
        std::fs::remove_dir_all(&target)?;
    }
    std::fs::rename(incoming, target)
}

/// What an install made by [`apply`] records as its source, so the next
/// bundle can tell the skills the hub sent from those installed otherwise.
pub const FROM_HUB: &str = "hub";

/// Hold `bundle` for `agent` on a node: write its folders under `root`,
/// reload `manager` so it finds them, install each for the agent, and
/// uninstall the skills an earlier bundle installed that this one
/// drops. Blocks: run it off the async threads.
///
/// `root` is shared by the agents a node hosts: a folder another agent
/// sent under the same name is replaced.
///
/// # Errors
/// When a folder can't be written or an install can't be recorded.
pub fn apply(
    manager: &mut crate::SkillManager,
    root: &Path,
    agent: aura_core_types::AgentId,
    bundle: &SkillBundle,
) -> Result<(), crate::SkillError> {
    std::fs::create_dir_all(root)?;
    for folder in &bundle.skills {
        write_folder(root, folder)?;
    }
    manager.reload();
    let kept: std::collections::HashSet<&str> =
        bundle.skills.iter().map(|s| s.name.as_str()).collect();
    for installed in manager.list_agent_skills(agent)? {
        let from_hub = installed.source_url.as_deref() == Some(FROM_HUB);
        if from_hub && !kept.contains(installed.skill_name.as_str()) {
            manager.uninstall_from_agent(agent, &installed.skill_name)?;
        }
    }
    for name in kept {
        manager.install_for_agent(agent, name, Some(FROM_HUB.to_string()), vec![], vec![])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
