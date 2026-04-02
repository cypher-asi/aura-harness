//! In-memory registry of loaded skills with precedence-based deduplication.

use crate::error::SkillError;
use crate::loader::SkillLoader;
use crate::types::{Skill, SkillMeta};
use std::collections::HashMap;
use tracing::{debug, warn};

/// In-memory registry mapping skill names to resolved [`Skill`] instances.
///
/// When multiple sources provide a skill with the same name, the one with the
/// highest [`SkillSource::precedence`](crate::types::SkillSource::precedence)
/// wins.
#[derive(Debug, Default)]
pub struct SkillRegistry {
    skills: HashMap<String, Skill>,
}

impl SkillRegistry {
    /// Create an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Reload the registry from the given loader, replacing all entries.
    pub fn reload(&mut self, loader: &SkillLoader) {
        self.skills.clear();

        for result in loader.load_all() {
            match result {
                Ok(skill) => {
                    let name = skill.frontmatter.name.clone();
                    let new_precedence = skill.source.precedence();

                    if let Some(existing) = self.skills.get(&name) {
                        if new_precedence <= existing.source.precedence() {
                            debug!(
                                "skipping {name} from {} (existing from {} has equal or higher precedence)",
                                skill.source, existing.source
                            );
                            continue;
                        }
                        debug!(
                            "overriding {name}: {} -> {}",
                            existing.source, skill.source
                        );
                    }

                    self.skills.insert(name, skill);
                }
                Err(e) => {
                    warn!("failed to load skill: {e}");
                }
            }
        }
    }

    /// Get a skill by name.
    ///
    /// # Errors
    ///
    /// Returns [`SkillError::NotFound`] if no skill with the given name is registered.
    pub fn get(&self, name: &str) -> Result<&Skill, SkillError> {
        self.skills
            .get(name)
            .ok_or_else(|| SkillError::NotFound(name.to_string()))
    }

    /// Return metadata for all skills where model invocation is **not** disabled.
    #[must_use]
    pub fn model_invocable_metadata(&self) -> Vec<SkillMeta> {
        self.skills
            .values()
            .filter(|s| !s.frontmatter.disable_model_invocation.unwrap_or(false))
            .map(skill_to_meta)
            .collect()
    }

    /// Return metadata for all user-invocable skills.
    #[must_use]
    pub fn user_invocable_metadata(&self) -> Vec<SkillMeta> {
        self.skills
            .values()
            .filter(|s| s.frontmatter.user_invocable.unwrap_or(false))
            .map(skill_to_meta)
            .collect()
    }

    /// Return metadata for all registered skills.
    #[must_use]
    pub fn all_metadata(&self) -> Vec<SkillMeta> {
        self.skills.values().map(skill_to_meta).collect()
    }

    /// Return skills whose `paths` globs match any of the given file paths.
    ///
    /// This is a simple prefix/contains check — full glob matching can be added
    /// later.
    #[must_use]
    pub fn skills_for_paths(&self, paths: &[String]) -> Vec<&Skill> {
        self.skills
            .values()
            .filter(|s| {
                s.frontmatter.paths.as_ref().is_some_and(|skill_paths| {
                    skill_paths.iter().any(|pattern| {
                        paths
                            .iter()
                            .any(|p| p.contains(pattern) || pattern.contains(p))
                    })
                })
            })
            .collect()
    }

    /// Number of skills in the registry.
    #[must_use]
    pub fn len(&self) -> usize {
        self.skills.len()
    }

    /// Whether the registry is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
    }
}

/// Convert a [`Skill`] to lightweight [`SkillMeta`].
fn skill_to_meta(skill: &Skill) -> SkillMeta {
    SkillMeta {
        name: skill.frontmatter.name.clone(),
        description: skill.frontmatter.description.clone(),
        source: skill.source.clone(),
        model_invocable: !skill.frontmatter.disable_model_invocation.unwrap_or(false),
        user_invocable: skill.frontmatter.user_invocable.unwrap_or(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader::{SkillLoaderConfig};
    use tempfile::TempDir;

    fn make_skill_dir(base: &std::path::Path, name: &str, desc: &str) {
        let dir = base.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {desc}\n---\nBody for {name}."),
        )
        .unwrap();
    }

    #[test]
    fn precedence_override() {
        let tmp = TempDir::new().unwrap();
        let workspace_skills = tmp.path().join("ws").join("skills");
        let personal_skills = tmp.path().join("personal");

        make_skill_dir(&workspace_skills, "deploy", "workspace version");
        make_skill_dir(&personal_skills, "deploy", "personal version");

        let loader = SkillLoader::new(SkillLoaderConfig {
            workspace_root: Some(tmp.path().join("ws")),
            personal_dir: Some(personal_skills),
            ..SkillLoaderConfig::default()
        });

        let mut reg = SkillRegistry::new();
        reg.reload(&loader);

        let skill = reg.get("deploy").unwrap();
        assert_eq!(skill.frontmatter.description, "workspace version");
    }

    #[test]
    fn get_not_found() {
        let reg = SkillRegistry::new();
        assert!(reg.get("nonexistent").is_err());
    }

    #[test]
    fn model_invocable_metadata_filters() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path().join("ws").join("skills");

        let dir = ws.join("visible");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            "---\nname: visible\ndescription: shown\n---\nBody.",
        ).unwrap();

        let dir2 = ws.join("hidden");
        std::fs::create_dir_all(&dir2).unwrap();
        std::fs::write(
            dir2.join("SKILL.md"),
            "---\nname: hidden\ndescription: not shown\ndisable-model-invocation: true\n---\nBody.",
        ).unwrap();

        let loader = SkillLoader::new(SkillLoaderConfig {
            workspace_root: Some(tmp.path().join("ws")),
            ..SkillLoaderConfig::default()
        });
        let mut reg = SkillRegistry::new();
        reg.reload(&loader);

        let meta = reg.model_invocable_metadata();
        assert!(meta.iter().any(|m| m.name == "visible"));
        assert!(!meta.iter().any(|m| m.name == "hidden"));
    }

    #[test]
    fn user_invocable_metadata_filters() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path().join("ws").join("skills");

        let dir = ws.join("user-skill");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            "---\nname: user-skill\ndescription: user can invoke\nuser-invocable: true\n---\nBody.",
        ).unwrap();

        let dir2 = ws.join("model-only");
        std::fs::create_dir_all(&dir2).unwrap();
        std::fs::write(
            dir2.join("SKILL.md"),
            "---\nname: model-only\ndescription: not user invocable\n---\nBody.",
        ).unwrap();

        let loader = SkillLoader::new(SkillLoaderConfig {
            workspace_root: Some(tmp.path().join("ws")),
            ..SkillLoaderConfig::default()
        });
        let mut reg = SkillRegistry::new();
        reg.reload(&loader);

        let meta = reg.user_invocable_metadata();
        assert!(meta.iter().any(|m| m.name == "user-skill"));
        assert!(!meta.iter().any(|m| m.name == "model-only"));
    }

    #[test]
    fn skills_for_paths_empty_paths() {
        let reg = SkillRegistry::new();
        let result = reg.skills_for_paths(&[]);
        assert!(result.is_empty());
    }

    #[test]
    fn skills_for_paths_no_match() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path().join("ws").join("skills");
        let dir = ws.join("path-skill");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            "---\nname: path-skill\ndescription: test\npaths:\n  - src/components\n---\nBody.",
        ).unwrap();

        let loader = SkillLoader::new(SkillLoaderConfig {
            workspace_root: Some(tmp.path().join("ws")),
            ..SkillLoaderConfig::default()
        });
        let mut reg = SkillRegistry::new();
        reg.reload(&loader);

        let result = reg.skills_for_paths(&["tests/unit".to_string()]);
        assert!(result.is_empty());
    }
}
