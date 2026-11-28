use super::*;
use crate::types::{SkillFrontmatter, SkillSource};

/// A skill named `name` in `dir`, with a SKILL.md and the given
/// reference files.
fn skill_in(dir: &Path, name: &str, references: &[(&str, &[u8])]) -> Skill {
    let folder = dir.join(name);
    std::fs::create_dir_all(folder.join(REFERENCES)).unwrap();
    std::fs::write(
        folder.join(SKILL_FILE),
        format!("---\nname: {name}\n---\nSteps."),
    )
    .unwrap();
    for (path, bytes) in references {
        let target = folder.join(path);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(target, bytes).unwrap();
    }
    let frontmatter: SkillFrontmatter =
        serde_yaml::from_str(&format!("name: {name}\ndescription: d")).unwrap();
    Skill {
        frontmatter,
        body: "Steps.".into(),
        source: SkillSource::Workspace,
        dir_path: folder,
    }
}

#[test]
fn a_bundle_carries_skill_md_and_text_references_and_leaves_the_rest_out() {
    let dir = tempfile::tempdir().unwrap();
    let big = vec![b'a'; usize::try_from(MAX_FILE_BYTES).unwrap() + 1];
    let skill = skill_in(
        dir.path(),
        "deploy",
        &[
            ("references/api.md", b"GET /v1"),
            ("references/deep/notes.md", b"notes"),
            ("references/logo.png", &[0xff, 0xfe, 0x00]),
            ("references/huge.md", &big),
        ],
    );
    let bundle = bundle(&[&skill]);
    let paths: Vec<&str> = bundle.skills[0]
        .files
        .iter()
        .map(|f| f.path.as_str())
        .collect();
    assert_eq!(
        paths,
        ["SKILL.md", "references/api.md", "references/deep/notes.md"]
    );
    assert_eq!(bundle.skills[0].name, "deploy");
}

#[test]
fn the_digest_changes_when_one_file_does() {
    let dir = tempfile::tempdir().unwrap();
    let skill = skill_in(dir.path(), "deploy", &[("references/api.md", b"v1")]);
    let first = bundle(&[&skill]).digest;
    assert_eq!(bundle(&[&skill]).digest, first, "stable");
    std::fs::write(skill.dir_path.join("references/api.md"), "v2").unwrap();
    assert_ne!(bundle(&[&skill]).digest, first);
}

#[test]
fn a_folder_is_written_in_place_of_the_old_one_and_stays_inside_its_root() {
    let root = tempfile::tempdir().unwrap();
    let folder = |text: &str, path: &str| SkillFolder {
        name: "deploy".into(),
        files: vec![SkillFile {
            path: path.into(),
            text: text.into(),
        }],
    };
    write_folder(root.path(), &folder("old", "references/old.md")).unwrap();
    write_folder(root.path(), &folder("new", "references/new.md")).unwrap();
    assert!(
        !root.path().join("deploy/references/old.md").exists(),
        "replaced"
    );
    let new = std::fs::read_to_string(root.path().join("deploy/references/new.md")).unwrap();
    assert_eq!(new, "new");
    for bad in ["../escape.md", "/etc/passwd", "references/../../x"] {
        assert!(
            write_folder(root.path(), &folder("x", bad)).is_err(),
            "{bad}"
        );
    }
    let named = SkillFolder {
        name: "../up".into(),
        files: Vec::new(),
    };
    assert!(write_folder(root.path(), &named).is_err());
}

#[test]
fn load_skill_text_returns_a_reference_and_lists_them_with_the_skill() {
    let dir = tempfile::tempdir().unwrap();
    let skill = skill_in(dir.path(), "deploy", &[("references/api.md", b"GET /v1")]);
    let entry = crate::prompt::SkillPromptEntry {
        name: "deploy",
        description: "d",
        body: &skill.body,
        dir_path: &skill.dir_path,
        agent_target_id: None,
        agent_target_name: None,
    };
    let whole = crate::on_demand::load_skill_text(&entry, None).unwrap();
    assert!(whole.contains("Steps.") && whole.contains("references/api.md"));
    let reference = crate::on_demand::load_skill_text(&entry, Some("references/api.md")).unwrap();
    assert_eq!(reference, "GET /v1");
    assert!(crate::on_demand::load_skill_text(&entry, Some("../deploy/SKILL.md")).is_err());
    assert!(crate::on_demand::load_skill_text(&entry, Some("references/none.md")).is_err());
}
