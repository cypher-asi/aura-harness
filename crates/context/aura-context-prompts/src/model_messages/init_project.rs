//! The prompt the terminal's `/init` command sends: have the agent study
//! the repository and write (or improve) the `AGENTS.md` at its root.

const INVESTIGATE: &str = "Investigate before writing:
- Read the README, contributing guides, build manifests (for example Cargo.toml, package.json, \
pyproject.toml, go.mod, Makefile, justfile) and CI workflows to find the commands the project \
really uses.
- Look at the directory layout and at a few representative source and test files to learn the \
conventions actually in use.
- Check for other agent instructions (CLAUDE.md, .cursor/rules/, .cursorrules, \
.github/copilot-instructions.md) and carry over whatever in them is still accurate.

Cover, concisely:
1. What the project is, in a short paragraph.
2. How to build, run, lint, format and test it, including how to run a single test. Only list \
commands you found evidence for; run the quick ones to confirm them when that is safe.
3. The repository layout: the main directories, packages or crates, and what lives where.
4. Conventions that tooling does not enforce: error handling, logging, naming, module structure, \
testing patterns.
5. Rules and pitfalls: what must or must not be done (generated files, checks to run before \
committing, dependency or layering rules, environment setup).

Guidelines:
- Be specific to this repository. Leave out generic advice such as \"write clean code\" or \
\"add tests\".
- Prefer short bullet points and exact commands in backticks. Aim for well under 200 lines.
- Do not invent commands, paths or policies you found no evidence for.
- Do not change any file other than AGENTS.md.";

/// The `/init` prompt for the project rooted at `root`. `existing` is
/// the path of the project's current `AGENTS.md`, when it has one; the
/// prompt then asks for improvements instead of a new file.
#[must_use]
pub fn init_project_prompt(root: &str, existing: Option<&str>) -> String {
    match existing {
        None => format!(
            "Study this repository and write an AGENTS.md file at its root ({root}/AGENTS.md). \
             Coding agents read this file at the start of every session, so it should hold what \
             an experienced contributor would tell a new one on their first day.\n\n\
             {INVESTIGATE}\n\n\
             When you are done, reply with a short summary of what the file covers."
        ),
        Some(path) => format!(
            "This repository already has an AGENTS.md ({path}). Coding agents read it at the \
             start of every session. Read it first, then improve it rather than rewriting it \
             from scratch: keep what is accurate and specific to this project (and its structure \
             and wording where they work), correct anything that is out of date or wrong, fill \
             the gaps from the list below, and remove generic filler.\n\n\
             {INVESTIGATE}\n\n\
             When you are done, reply with a short summary of what you changed and why."
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_file_prompt_names_the_target() {
        let prompt = init_project_prompt("/repo", None);
        assert!(prompt.contains("/repo/AGENTS.md"));
        assert!(prompt.contains("run a single test"));
        assert!(!prompt.contains("already has"));
    }

    #[test]
    fn existing_file_prompt_asks_to_improve_it() {
        let prompt = init_project_prompt("/repo", Some("/repo/AGENTS.md"));
        assert!(prompt.contains("already has an AGENTS.md (/repo/AGENTS.md)"));
        assert!(prompt.contains("improve it rather than rewriting it"));
    }
}
