//! Skill folders the hub sends a cloud node, so the node can list an
//! agent's skills and serve `load_skill` itself instead of the hub pasting
//! every skill's text into each turn (`PUT /v1/cloud/skills/{agent_id}`).

use serde::{Deserialize, Serialize};

/// One file of a skill folder: its path inside the folder and its text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillFile {
    /// Relative, `/`-separated (`SKILL.md`, `references/api.md`)
    pub path: String,
    pub text: String,
}

/// One skill's folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillFolder {
    /// The skill's name, also the folder's name on the node
    pub name: String,
    pub files: Vec<SkillFile>,
}

/// Every skill an agent has, with a digest of them all: the hub resends
/// only when the digest changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillBundle {
    pub digest: String,
    pub skills: Vec<SkillFolder>,
}

impl SkillBundle {
    /// The bundle of `skills`, sorted by name and path, with its digest.
    #[must_use]
    pub fn new(mut skills: Vec<SkillFolder>) -> Self {
        skills.sort_by(|a, b| a.name.cmp(&b.name));
        for skill in &mut skills {
            skill.files.sort_by(|a, b| a.path.cmp(&b.path));
        }
        let mut hasher = blake3::Hasher::new();
        for skill in &skills {
            hash_part(&mut hasher, &skill.name);
            for file in &skill.files {
                hash_part(&mut hasher, &file.path);
                hash_part(&mut hasher, &file.text);
            }
        }
        Self {
            digest: hasher.finalize().to_hex().to_string(),
            skills,
        }
    }
}

/// Hash `part` with its length first, so parts can't run into each other.
fn hash_part(hasher: &mut blake3::Hasher, part: &str) {
    hasher.update(&(part.len() as u64).to_le_bytes());
    hasher.update(part.as_bytes());
}

/// What the node answers once it holds a bundle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillsSynced {
    pub digest: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str, text: &str) -> SkillFolder {
        SkillFolder {
            name: name.into(),
            files: vec![
                SkillFile {
                    path: "references/b.md".into(),
                    text: "ref".into(),
                },
                SkillFile {
                    path: "SKILL.md".into(),
                    text: text.into(),
                },
            ],
        }
    }

    #[test]
    fn the_digest_ignores_order_and_follows_every_file() {
        let one = SkillBundle::new(vec![folder("b", "x"), folder("a", "y")]);
        let same = SkillBundle::new(vec![folder("a", "y"), folder("b", "x")]);
        assert_eq!(one.digest, same.digest);
        assert_eq!(one.skills[0].name, "a");
        assert_eq!(one.skills[0].files[0].path, "SKILL.md");
        let changed = SkillBundle::new(vec![folder("a", "y!"), folder("b", "x")]);
        assert_ne!(one.digest, changed.digest);
    }
}
