//! System-prompt injection — formats skill metadata as compact XML.

use std::fmt::Write;

use crate::types::SkillMeta;

/// Render a list of skill metadata entries as an `<available_skills>` XML block
/// suitable for injection into a system prompt.
///
/// Returns an empty string when no skills are provided.
#[must_use]
pub fn render_skills_xml(skills: &[SkillMeta]) -> String {
    if skills.is_empty() {
        return String::new();
    }

    let mut buf = String::from("<available_skills>\n");
    for s in skills {
        let _ = writeln!(
            buf,
            "<skill name=\"{}\" description=\"{}\" location=\"{}\"/>",
            xml_escape(&s.name),
            xml_escape(&s.description),
            s.source,
        );
    }
    buf.push_str("</available_skills>");
    buf
}

/// Inject skill metadata into a system prompt string.
///
/// Appends the rendered XML block after a blank line separator.
pub fn inject_into_prompt(system_prompt: &mut String, skills: &[SkillMeta]) {
    let xml = render_skills_xml(skills);
    if xml.is_empty() {
        return;
    }
    system_prompt.push_str("\n\n");
    system_prompt.push_str(&xml);
}

/// Minimal XML escaping for attribute values.
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::SkillSource;

    #[test]
    fn empty_skills_produce_empty_string() {
        assert_eq!(render_skills_xml(&[]), "");
    }

    #[test]
    fn single_skill_xml() {
        let meta = vec![SkillMeta {
            name: "deploy".to_string(),
            description: "Deploy the app".to_string(),
            source: SkillSource::Workspace,
            model_invocable: true,
            user_invocable: true,
        }];
        let xml = render_skills_xml(&meta);
        assert!(xml.contains("<available_skills>"));
        assert!(xml.contains("name=\"deploy\""));
        assert!(xml.contains("location=\"workspace\""));
    }

    #[test]
    fn inject_appends_to_prompt() {
        let mut prompt = "You are an assistant.".to_string();
        let meta = vec![SkillMeta {
            name: "test".to_string(),
            description: "A test skill".to_string(),
            source: SkillSource::Personal,
            model_invocable: true,
            user_invocable: false,
        }];
        inject_into_prompt(&mut prompt, &meta);
        assert!(prompt.starts_with("You are an assistant."));
        assert!(prompt.contains("<available_skills>"));
    }

    #[test]
    fn xml_escaping() {
        let meta = vec![SkillMeta {
            name: "test".to_string(),
            description: "Use <special> & \"chars\"".to_string(),
            source: SkillSource::Bundled,
            model_invocable: true,
            user_invocable: false,
        }];
        let xml = render_skills_xml(&meta);
        assert!(xml.contains("&lt;special&gt;"));
        assert!(xml.contains("&amp;"));
        assert!(xml.contains("&quot;chars&quot;"));
    }
}
