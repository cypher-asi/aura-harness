//! Skills loaded on demand: the system prompt carries only an index of an
//! agent's skills (name, description, path), and the `load_skill` tool
//! returns one skill's full instructions when the agent asks for it.
//!
//! The index costs a line or two per skill however long its SKILL.md is,
//! so an agent with several skills no longer pays for all their bodies on
//! every turn. A skill's delegation target stays in the index: it is a
//! rule the model must see before it picks anything, not detail to fetch.

use std::fmt::Write;

use crate::prompt::{push_target_note, xml_escape, SkillPromptEntry};

/// Name of the tool that returns a skill's full instructions.
pub const LOAD_SKILL_TOOL_NAME: &str = "load_skill";

/// Render the index of `skills`: each one's name, path and description,
/// without its body, and how to load the rest.
///
/// Returns an empty string when no skills are provided.
#[must_use]
pub fn render_skill_index_xml(skills: &[SkillPromptEntry<'_>]) -> String {
    if skills.is_empty() {
        return String::new();
    }
    let mut buf = format!(
        "<agent_skills>\n\
         You have the skills listed below. Each entry gives only what the skill is for. \
         Before you do a task a skill covers, call `{LOAD_SKILL_TOOL_NAME}` with the skill's \
         name to get its full instructions, then follow them. Load a skill once per \
         conversation; its instructions stay in your context.\n\n"
    );
    for s in skills {
        let _ = write!(
            buf,
            "<agent_skill name=\"{}\" skillPath=\"{}\">{}",
            xml_escape(s.name),
            xml_escape(&s.dir_path.display().to_string()),
            xml_escape(s.description),
        );
        push_target_note(&mut buf, s);
        buf.push_str("</agent_skill>\n");
    }
    buf.push_str("</agent_skills>");
    buf
}

/// Append the index of `skills` to `system_prompt` after a blank line.
pub fn inject_skill_index(system_prompt: &mut String, skills: &[SkillPromptEntry<'_>]) {
    let xml = render_skill_index_xml(skills);
    if xml.is_empty() {
        return;
    }
    system_prompt.push_str("\n\n");
    system_prompt.push_str(&xml);
}

/// What `load_skill` returns for `skill` and, when given, one `file` of
/// its folder: the file's text, else the skill's full instructions with
/// the reference files it can load.
///
/// # Errors
/// When `file` leaves the folder or can't be read.
pub fn load_skill_text(skill: &SkillPromptEntry<'_>, file: Option<&str>) -> Result<String, String> {
    let Some(file) = file.map(str::trim).filter(|file| !file.is_empty()) else {
        return Ok(render_loaded_skill(skill));
    };
    let relative = crate::bundle::inside(file)
        .ok_or_else(|| format!("`{file}` is not a path inside the skill's folder"))?;
    crate::bundle::read_text(&skill.dir_path.join(relative))
        .map_err(|why| format!("can't load `{file}` of skill `{}`: {why}", skill.name))
}

/// What `load_skill` returns for `skill`: its full instructions, with
/// its path, delegation target and reference files.
#[must_use]
pub fn render_loaded_skill(skill: &SkillPromptEntry<'_>) -> String {
    let mut buf = format!(
        "<skill name=\"{}\" skillPath=\"{}\">\n{}",
        xml_escape(skill.name),
        xml_escape(&skill.dir_path.display().to_string()),
        skill.description,
    );
    if !skill.body.is_empty() {
        let _ = write!(buf, "\n\n{}", skill.body);
    }
    push_target_note(&mut buf, skill);
    let references = crate::bundle::reference_files(skill.dir_path);
    if !references.is_empty() {
        let _ = write!(
            buf,
            "\n\nReference files, loaded with `{LOAD_SKILL_TOOL_NAME}` and `file`: {}",
            references.join(", ")
        );
    }
    buf.push_str("\n</skill>");
    buf
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn entry<'a>(name: &'a str, body: &'a str, target: Option<&'a str>) -> SkillPromptEntry<'a> {
        SkillPromptEntry {
            name,
            description: "Ship a release",
            body,
            dir_path: Path::new("/skills/deploy"),
            agent_target_id: target,
            agent_target_name: target.map(|_| "Release bot"),
        }
    }

    #[test]
    fn the_index_names_each_skill_and_its_target_but_carries_no_body() {
        let skills = [
            entry("deploy", "Step 1: tag the release.", Some("abc123")),
            entry("review", "Read every diff twice.", None),
        ];
        let index = render_skill_index_xml(&skills);
        for expected in [
            "name=\"deploy\"",
            "name=\"review\"",
            "skillPath=\"/skills/deploy\"",
            "Ship a release",
            "agent_id=\"abc123\"",
            LOAD_SKILL_TOOL_NAME,
        ] {
            assert!(index.contains(expected), "{expected} in {index}");
        }
        assert!(!index.contains("tag the release"));
        assert!(!index.contains("every diff"));
    }

    #[test]
    fn no_skills_add_nothing() {
        let mut prompt = String::from("base");
        inject_skill_index(&mut prompt, &[]);
        assert_eq!(prompt, "base");
    }

    #[test]
    fn a_loaded_skill_carries_its_whole_body_and_target() {
        let loaded = render_loaded_skill(&entry("deploy", "Step 1: tag <v1>.", Some("abc123")));
        assert!(
            loaded.contains("Step 1: tag <v1>."),
            "bodies are not escaped"
        );
        assert!(loaded.contains("Ship a release"));
        assert!(loaded.contains("agent_id=\"abc123\""));
    }
}
