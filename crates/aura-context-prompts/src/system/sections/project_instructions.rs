//! `<project_instructions>` section: the user's and the project's
//! instruction files (`AGENTS.md`, `CLAUDE.md`, Cursor rules).
//!
//! Render-only: the caller discovers and reads the files, applies the
//! size cap, and hands over borrowed views. The output depends only on
//! its inputs, so unchanged files render to the same bytes on every
//! turn and the system prompt stays cacheable.

/// Opening tag of the section. Tests assert on this string instead of
/// duplicating the literal.
pub const PROJECT_INSTRUCTIONS_TAG: &str = "<project_instructions>";

/// Closing tag of the section.
pub const PROJECT_INSTRUCTIONS_CLOSE: &str = "</project_instructions>";

const PREAMBLE: &str = "The user and this project keep instructions for coding agents in the \
files below. Follow them. They are listed from the most general (user-wide) to the most \
specific (closest to your working folder); when two of them disagree, the one listed later \
wins. A rule that names globs applies only when you work on files matching them. \
Instructions the user gives you in the conversation take precedence over all of these files.";

/// One instruction file included in the prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InstructionFile<'a> {
    /// Path shown to the model (absolute, or `~/`-relative).
    pub path: &'a str,
    /// Globs a scoped rule applies to, as written in its frontmatter.
    pub globs: Option<&'a str>,
    /// File content to include (without frontmatter).
    pub body: &'a str,
    /// Size of the whole body in bytes when `body` is only its start.
    pub truncated_from: Option<usize>,
}

/// A rule the agent should read only when it is relevant: listed by
/// path and description instead of being included.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OnRequestRule<'a> {
    /// Path shown to the model.
    pub path: &'a str,
    /// When the rule applies, in its author's words.
    pub description: &'a str,
}

/// Everything the section shows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProjectInstructionsView<'a> {
    /// Files included, most general first.
    pub files: &'a [InstructionFile<'a>],
    /// Rules listed for the agent to read when relevant.
    pub on_request: &'a [OnRequestRule<'a>],
    /// Files left out because the size limit was reached.
    pub omitted: &'a [&'a str],
}

impl ProjectInstructionsView<'_> {
    /// Whether there is nothing to show.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.files.is_empty() && self.on_request.is_empty() && self.omitted.is_empty()
    }
}

/// Render the section, or `None` when there is nothing to show.
#[must_use]
pub fn render(view: &ProjectInstructionsView<'_>) -> Option<String> {
    if view.is_empty() {
        return None;
    }
    let body_bytes: usize = view.files.iter().map(|f| f.body.len()).sum();
    let mut out = String::with_capacity(body_bytes + 1024);
    out.push_str(PROJECT_INSTRUCTIONS_TAG);
    out.push('\n');
    out.push_str(PREAMBLE);
    for file in view.files {
        out.push_str("\n\n<instructions path=\"");
        out.push_str(&escape_attr(file.path));
        out.push('"');
        if let Some(globs) = file.globs {
            out.push_str(" globs=\"");
            out.push_str(&escape_attr(globs));
            out.push('"');
        }
        out.push_str(">\n");
        out.push_str(file.body.trim_end());
        out.push('\n');
        if let Some(total) = file.truncated_from {
            out.push_str(&format!(
                "[Truncated: only the first {} of {total} bytes are shown. Read the file for \
                 the rest.]\n",
                file.body.len()
            ));
        }
        out.push_str("</instructions>");
    }
    if !view.on_request.is_empty() {
        out.push_str(
            "\n\nThese rules apply only in some situations. Read one when its description \
             matches what you are doing:",
        );
        for rule in view.on_request {
            out.push_str(&format!("\n- {}: {}", rule.path, rule.description.trim()));
        }
    }
    if !view.omitted.is_empty() {
        out.push_str(
            "\n\nThese instruction files were left out because the instructions above reached \
             the size limit. Read them if they look relevant:",
        );
        for path in view.omitted {
            out.push_str(&format!("\n- {path}"));
        }
    }
    out.push('\n');
    out.push_str(PROJECT_INSTRUCTIONS_CLOSE);
    Some(out)
}

fn escape_attr(value: &str) -> String {
    value.replace('&', "&amp;").replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file<'a>(path: &'a str, body: &'a str) -> InstructionFile<'a> {
        InstructionFile {
            path,
            globs: None,
            body,
            truncated_from: None,
        }
    }

    #[test]
    fn empty_view_renders_nothing() {
        assert_eq!(render(&ProjectInstructionsView::default()), None);
    }

    #[test]
    fn files_render_in_order_with_paths_and_globs() {
        let files = [
            file("~/.aura/AGENTS.md", "user rule"),
            InstructionFile {
                globs: Some("**/*.rs"),
                ..file("/repo/.cursor/rules/rust.mdc", "rust rule\n\n")
            },
            file("/repo/AGENTS.md", "project rule"),
        ];
        let text = render(&ProjectInstructionsView {
            files: &files,
            ..ProjectInstructionsView::default()
        })
        .unwrap_or_default();

        assert!(text.starts_with(PROJECT_INSTRUCTIONS_TAG));
        assert!(text.ends_with(PROJECT_INSTRUCTIONS_CLOSE));
        assert!(text.contains(
            "<instructions path=\"/repo/.cursor/rules/rust.mdc\" globs=\"**/*.rs\">\nrust rule\n</instructions>"
        ));
        let user = text.find("user rule").unwrap_or(usize::MAX);
        let rust = text.find("rust rule").unwrap_or(usize::MAX);
        let project = text.find("project rule").unwrap_or(usize::MAX);
        assert!(user < rust && rust < project, "order kept: {text}");
    }

    #[test]
    fn truncation_on_request_and_omitted_files_are_noted() {
        let files = [InstructionFile {
            truncated_from: Some(50_000),
            ..file("/repo/AGENTS.md", "start")
        }];
        let on_request = [OnRequestRule {
            path: "/repo/.cursor/rules/db.mdc",
            description: "Database migrations",
        }];
        let omitted = ["/AGENTS.md"];
        let text = render(&ProjectInstructionsView {
            files: &files,
            on_request: &on_request,
            omitted: &omitted,
        })
        .unwrap_or_default();

        assert!(text.contains("only the first 5 of 50000 bytes"));
        assert!(text.contains("- /repo/.cursor/rules/db.mdc: Database migrations"));
        assert!(text.contains("size limit"));
        assert!(text.contains("\n- /AGENTS.md"));
    }

    #[test]
    fn same_input_renders_same_bytes() {
        let files = [file("/repo/AGENTS.md", "rule")];
        let view = ProjectInstructionsView {
            files: &files,
            ..ProjectInstructionsView::default()
        };
        assert_eq!(render(&view), render(&view));
    }

    #[test]
    fn quotes_in_paths_are_escaped() {
        let files = [file("/a \"b\"/AGENTS.md", "rule")];
        let text = render(&ProjectInstructionsView {
            files: &files,
            ..ProjectInstructionsView::default()
        })
        .unwrap_or_default();
        assert!(text.contains("path=\"/a &quot;b&quot;/AGENTS.md\""));
    }
}
